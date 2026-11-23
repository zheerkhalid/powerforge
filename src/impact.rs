//! Estimated per-app power impact from CPU, GPU and disk use. Clearly an
//! estimate: Windows does not measure per-app power.

use std::collections::HashMap;
use std::time::Instant;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    VeryLow,
    Low,
    Moderate,
    High,
    VeryHigh,
}

impl Level {
    pub fn label(self) -> &'static str {
        match self {
            Level::VeryLow => "Very low",
            Level::Low => "Low",
            Level::Moderate => "Moderate",
            Level::High => "High",
            Level::VeryHigh => "Very high",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct AppImpact {
    /// Executable name, e.g. "chrome.exe".
    pub exe: String,
    pub processes: u32,
    /// Share of total CPU capacity, 0..=100.
    pub cpu_pct: f32,
    /// GPU engine utilisation, 0..=100 (None when unavailable).
    pub gpu_pct: Option<f32>,
    /// Disk bytes per second (read + write).
    pub disk_bps: f64,
    /// Estimated watts attributable to the app.
    pub est_watts: f32,
    pub level: Level,
}

/// Typical whole-system draw of a laptop under light load, used when the
/// battery's own rate is not available (plugged in, desktops).
pub const TYPICAL_WATTS: f32 = 15.0;

/// Estimated watts from usage shares. `system_watts` is the measured
/// discharge rate when on battery, else [`TYPICAL_WATTS`]. The processor is
/// taken as about 60 % of the variable draw, the GPU 30 %, and storage
/// about 2 W per 100 MB/s (capped at 4 W).
pub fn estimate_watts(cpu_pct: f32, gpu_pct: f32, disk_bps: f64, system_watts: f32) -> f32 {
    let sys = system_watts.clamp(3.0, 200.0);
    let cpu = cpu_pct.clamp(0.0, 100.0) / 100.0 * sys * 0.6;
    let gpu = gpu_pct.clamp(0.0, 100.0) / 100.0 * sys * 0.3;
    let disk = ((disk_bps / 100_000_000.0) * 2.0).min(4.0) as f32;
    cpu + gpu + disk
}

/// Level of an estimated wattage.
pub fn level(watts: f32) -> Level {
    if watts < 0.3 {
        Level::VeryLow
    } else if watts < 1.0 {
        Level::Low
    } else if watts < 3.0 {
        Level::Moderate
    } else if watts < 6.0 {
        Level::High
    } else {
        Level::VeryHigh
    }
}

/// One process's usage over the sample interval.
#[derive(Clone, Debug, PartialEq)]
pub struct ProcUsage {
    pub exe: String,
    pub cpu_pct: f32,
    pub gpu_pct: Option<f32>,
    pub disk_bps: f64,
}

/// Groups processes by exe name (case-insensitive; the first spelling is
/// kept), estimates watts and sorts by them, largest first. Apps using
/// nothing measurable are left out.
pub fn group(procs: &[ProcUsage], system_watts: f32) -> Vec<AppImpact> {
    let mut map: HashMap<String, AppImpact> = HashMap::new();
    for p in procs {
        let key = p.exe.to_lowercase();
        let e = map.entry(key).or_insert_with(|| AppImpact {
            exe: p.exe.clone(),
            processes: 0,
            cpu_pct: 0.0,
            gpu_pct: None,
            disk_bps: 0.0,
            est_watts: 0.0,
            level: Level::VeryLow,
        });
        e.processes += 1;
        e.cpu_pct += p.cpu_pct;
        if let Some(g) = p.gpu_pct {
            e.gpu_pct = Some((e.gpu_pct.unwrap_or(0.0) + g).min(100.0));
        }
        e.disk_bps += p.disk_bps;
    }
    let mut out: Vec<AppImpact> = map
        .into_values()
        .map(|mut a| {
            a.cpu_pct = a.cpu_pct.min(100.0);
            a.est_watts = estimate_watts(
                a.cpu_pct,
                a.gpu_pct.unwrap_or(0.0),
                a.disk_bps,
                system_watts,
            );
            a.level = level(a.est_watts);
            a
        })
        .filter(|a| a.est_watts >= 0.01)
        .collect();
    out.sort_by(|a, b| b.est_watts.total_cmp(&a.est_watts).then(a.exe.cmp(&b.exe)));
    out
}

#[derive(Clone, Copy)]
struct Counters {
    cpu_100ns: u64,
    io_bytes: u64,
}

/// Keeps the previous counters so each call measures the time since the last.
#[derive(Default)]
pub struct Sampler {
    last: HashMap<u32, (String, Counters)>,
    at: Option<Instant>,
    gpu: Option<gpu::GpuQuery>,
    gpu_tried: bool,
}

impl Sampler {
    pub fn new() -> Sampler {
        Sampler::default()
    }

    /// Apps sorted by estimated watts, largest first. The first call
    /// returns an empty list (it only primes the counters).
    pub fn sample(&mut self, system_watts: f32) -> Vec<AppImpact> {
        if !self.gpu_tried {
            self.gpu_tried = true;
            self.gpu = gpu::GpuQuery::open();
        }
        let gpu_by_pid = self.gpu.as_mut().and_then(|g| g.read());
        let now = Instant::now();
        let current = read_processes();
        let cpus = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1) as f64;
        let mut procs = Vec::new();
        if let Some(at) = self.at {
            let wall = now.duration_since(at).as_secs_f64().max(0.05);
            for (pid, (exe, c)) in &current {
                let Some((_, prev)) = self.last.get(pid) else {
                    continue;
                };
                let cpu_secs = c.cpu_100ns.saturating_sub(prev.cpu_100ns) as f64 / 1e7;
                procs.push(ProcUsage {
                    exe: exe.clone(),
                    cpu_pct: (cpu_secs / wall / cpus * 100.0) as f32,
                    gpu_pct: gpu_by_pid
                        .as_ref()
                        .map(|m| m.get(pid).copied().unwrap_or(0.0)),
                    disk_bps: c.io_bytes.saturating_sub(prev.io_bytes) as f64 / wall,
                });
            }
        }
        self.last = current;
        self.at = Some(now);
        group(&procs, system_watts)
    }
}

fn read_processes() -> HashMap<u32, (String, Counters)> {
    use windows::Win32::Foundation::{CloseHandle, FILETIME};
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };
    use windows::Win32::System::Threading::{
        GetProcessIoCounters, GetProcessTimes, OpenProcess, IO_COUNTERS,
        PROCESS_QUERY_LIMITED_INFORMATION,
    };
    let mut out = HashMap::new();
    let ft = |f: FILETIME| ((f.dwHighDateTime as u64) << 32) | f.dwLowDateTime as u64;
    // SAFETY: a process snapshot walked with correctly sized entries; every
    // handle opened is closed.
    unsafe {
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else {
            return out;
        };
        let mut entry = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        let mut ok = Process32FirstW(snap, &mut entry).is_ok();
        while ok {
            let pid = entry.th32ProcessID;
            if pid != 0 {
                if let Ok(h) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) {
                    let (mut c, mut e, mut k, mut u) = Default::default();
                    let mut io = IO_COUNTERS::default();
                    if GetProcessTimes(h, &mut c, &mut e, &mut k, &mut u).is_ok() {
                        let _ = GetProcessIoCounters(h, &mut io);
                        let exe = crate::sys::from_wide(&entry.szExeFile);
                        out.insert(
                            pid,
                            (
                                exe,
                                Counters {
                                    cpu_100ns: ft(k) + ft(u),
                                    io_bytes: io.ReadTransferCount + io.WriteTransferCount,
                                },
                            ),
                        );
                    }
                    let _ = CloseHandle(h);
                }
            }
            ok = Process32NextW(snap, &mut entry).is_ok();
        }
        let _ = CloseHandle(snap);
    }
    out
}

mod gpu {
    //! Per-process GPU use from the "GPU Engine" performance counters.
    use std::collections::HashMap;
    use windows::core::PCWSTR;
    use windows::Win32::System::Performance::{
        PdhAddEnglishCounterW, PdhCloseQuery, PdhCollectQueryData, PdhGetFormattedCounterArrayW,
        PdhOpenQueryW, PDH_FMT_COUNTERVALUE_ITEM_W, PDH_FMT_DOUBLE,
    };

    pub struct GpuQuery {
        query: isize,
        counter: isize,
    }

    // SAFETY: PDH handles may be used from any thread, one at a time.
    unsafe impl Send for GpuQuery {}

    impl Drop for GpuQuery {
        fn drop(&mut self) {
            // SAFETY: the query was opened by PdhOpenQueryW and is closed once.
            unsafe {
                let _ = PdhCloseQuery(self.query);
            }
        }
    }

    impl GpuQuery {
        pub fn open() -> Option<GpuQuery> {
            let path = crate::sys::wide("\\GPU Engine(*)\\Utilization Percentage");
            // SAFETY: PDH calls with out-parameters; the query is closed on failure.
            unsafe {
                let mut query = 0isize;
                if PdhOpenQueryW(PCWSTR::null(), 0, &mut query) != 0 {
                    return None;
                }
                let mut counter = 0isize;
                if PdhAddEnglishCounterW(query, PCWSTR(path.as_ptr()), 0, &mut counter) != 0 {
                    let _ = PdhCloseQuery(query);
                    return None;
                }
                let _ = PdhCollectQueryData(query);
                Some(GpuQuery { query, counter })
            }
        }

        /// Percent per process id, summed over engines and capped at 100.
        pub fn read(&mut self) -> Option<HashMap<u32, f32>> {
            // SAFETY: the buffer is sized by the first call and holds the
            // item array PDH writes; names point into that buffer.
            unsafe {
                if PdhCollectQueryData(self.query) != 0 {
                    return None;
                }
                let mut size = 0u32;
                let mut count = 0u32;
                let _ = PdhGetFormattedCounterArrayW(
                    self.counter,
                    PDH_FMT_DOUBLE,
                    &mut size,
                    &mut count,
                    None,
                );
                if size == 0 {
                    return Some(HashMap::new());
                }
                let mut buf = vec![0u64; (size as usize).div_ceil(8) + 1];
                let items = buf.as_mut_ptr() as *mut PDH_FMT_COUNTERVALUE_ITEM_W;
                if PdhGetFormattedCounterArrayW(
                    self.counter,
                    PDH_FMT_DOUBLE,
                    &mut size,
                    &mut count,
                    Some(items),
                ) != 0
                {
                    return None;
                }
                let mut out: HashMap<u32, f32> = HashMap::new();
                for i in 0..count as usize {
                    let item = &*items.add(i);
                    let name = item.szName.to_string().unwrap_or_default();
                    let Some(pid) = super::pid_from_instance(&name) else {
                        continue;
                    };
                    let v = item.FmtValue.Anonymous.doubleValue as f32;
                    if v.is_finite() && v > 0.0 {
                        let e = out.entry(pid).or_default();
                        *e = (*e + v).min(100.0);
                    }
                }
                Some(out)
            }
        }
    }
}

/// "pid_1234_luid_0x..._engtype_3D" -> 1234.
pub fn pid_from_instance(name: &str) -> Option<u32> {
    let rest = name.split("pid_").nth(1)?;
    rest.split('_').next()?.parse().ok()
}

/// Invented apps for `--demo`.
pub fn demo() -> Vec<AppImpact> {
    let p = |exe: &str, n: u32, cpu: f32, gpu: f32, disk: f64| ProcUsage {
        exe: exe.to_string(),
        cpu_pct: cpu / n as f32,
        gpu_pct: Some(gpu / n as f32),
        disk_bps: disk / n as f64,
    };
    let mut procs = Vec::new();
    for (exe, n, cpu, gpu, disk) in [
        ("msedge.exe", 14, 38.0, 30.0, 1_200_000.0),
        ("Teams.exe", 6, 12.0, 10.0, 300_000.0),
        ("Code.exe", 9, 6.0, 1.0, 900_000.0),
        ("OneDrive.exe", 1, 1.4, 0.0, 6_500_000.0),
        ("MsMpEng.exe", 1, 2.0, 0.0, 3_000_000.0),
        ("dwm.exe", 1, 1.1, 5.5, 0.0),
        ("explorer.exe", 1, 0.6, 0.4, 120_000.0),
        ("Spotify.exe", 5, 0.9, 0.6, 80_000.0),
        ("SearchHost.exe", 1, 0.3, 0.2, 40_000.0),
        ("svchost.exe", 70, 1.2, 0.0, 500_000.0),
        ("audiodg.exe", 1, 0.4, 0.0, 0.0),
        ("PowerForge.exe", 1, 0.2, 0.3, 2_000.0),
    ] {
        for _ in 0..n {
            procs.push(p(exe, n, cpu, gpu, disk));
        }
    }
    group(&procs, 9.8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn estimates_and_levels() {
        assert_eq!(estimate_watts(0.0, 0.0, 0.0, 10.0), 0.0);
        assert!((estimate_watts(50.0, 0.0, 0.0, 10.0) - 3.0).abs() < 1e-5);
        assert!((estimate_watts(0.0, 100.0, 0.0, 10.0) - 3.0).abs() < 1e-5);
        assert!((estimate_watts(0.0, 0.0, 100_000_000.0, 10.0) - 2.0).abs() < 1e-5);
        assert_eq!(estimate_watts(0.0, 0.0, 1e12, 10.0), 4.0);
        // Out-of-range inputs are clamped.
        assert!((estimate_watts(500.0, -5.0, 0.0, 1000.0) - 120.0).abs() < 1e-3);
        assert_eq!(level(0.1), Level::VeryLow);
        assert_eq!(level(0.5), Level::Low);
        assert_eq!(level(2.0), Level::Moderate);
        assert_eq!(level(4.0), Level::High);
        assert_eq!(level(10.0), Level::VeryHigh);
        assert!(Level::High > Level::Low);
    }

    #[test]
    fn grouping() {
        let u = |exe: &str, cpu: f32, gpu: Option<f32>| ProcUsage {
            exe: exe.into(),
            cpu_pct: cpu,
            gpu_pct: gpu,
            disk_bps: 0.0,
        };
        let g = group(
            &[
                u("Chrome.exe", 10.0, Some(60.0)),
                u("chrome.exe", 5.0, Some(60.0)),
                u("idle.exe", 0.0, None),
                u("notepad.exe", 1.0, None),
            ],
            10.0,
        );
        assert_eq!(g.len(), 2);
        assert_eq!(g[0].exe, "Chrome.exe");
        assert_eq!(g[0].processes, 2);
        assert_eq!(g[0].cpu_pct, 15.0);
        assert_eq!(g[0].gpu_pct, Some(100.0));
        assert_eq!(g[1].exe, "notepad.exe");
        assert_eq!(g[1].gpu_pct, None);
        assert!(g[0].est_watts > g[1].est_watts);
        assert_eq!(
            pid_from_instance("pid_1234_luid_0x0_0x1_phys_0_eng_0_engtype_3D"),
            Some(1234)
        );
        assert_eq!(pid_from_instance("nothing"), None);
    }

    #[test]
    fn sampler_runs() {
        let mut s = Sampler::new();
        assert!(s.sample(TYPICAL_WATTS).is_empty());
        std::thread::sleep(std::time::Duration::from_millis(200));
        let apps = s.sample(TYPICAL_WATTS);
        assert!(apps.windows(2).all(|w| w[0].est_watts >= w[1].est_watts));
        let d = demo();
        assert_eq!(d.len(), 12);
        assert_eq!(d[0].exe, "msedge.exe");
    }
}
