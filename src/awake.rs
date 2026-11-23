//! Keep awake: indefinitely, for N minutes, until a time, while an app runs,
//! or while a download / file copy is active (network or disk above a
//! threshold). Uses SetThreadExecutionState from the engine thread. The
//! timer logic is pure and tested with a recording executor.

use crate::util;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum AwakeMode {
    Indefinite,
    /// Until this Unix time (used for both "for N minutes" and "until 18:30").
    Until(i64),
    /// While this exe (lower case) is running.
    WhileApp(String),
    /// While network or disk throughput stays above the thresholds (bytes
    /// per second); ends after `grace_secs` below both.
    WhileActivity {
        net_bps: u64,
        disk_bps: u64,
        grace_secs: u32,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AwakeRequest {
    pub mode: AwakeMode,
    /// Also keep the screen on.
    pub keep_display: bool,
}

/// What the UI and tray show.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AwakeStatus {
    pub active: bool,
    pub request: Option<AwakeRequest>,
    pub started: Option<i64>,
    /// When it ends, when known.
    pub ends_at: Option<i64>,
    /// "Keeping the PC awake for 25 more minutes", "Off", ...
    pub text: String,
    /// Why the last request ended, shown once ("backup.exe has closed").
    pub ended: Option<String>,
}

/// Applies the execution state. The real one calls SetThreadExecutionState;
/// tests use a recorder.
pub trait Executor {
    /// `system`: no idle sleep; `display`: no screen off. (false, false)
    /// releases everything.
    fn apply(&mut self, system: bool, display: bool);
}

/// SetThreadExecutionState on the calling thread (must stay alive while
/// the request holds: the engine thread).
#[derive(Default)]
pub struct SystemExecutor;

impl Executor for SystemExecutor {
    fn apply(&mut self, system: bool, display: bool) {
        use windows::Win32::System::Power::{
            SetThreadExecutionState, ES_CONTINUOUS, ES_DISPLAY_REQUIRED, ES_SYSTEM_REQUIRED,
        };
        let mut flags = ES_CONTINUOUS;
        if system {
            flags |= ES_SYSTEM_REQUIRED;
        }
        if display {
            flags |= ES_DISPLAY_REQUIRED;
        }
        // SAFETY: plain call; ES_CONTINUOUS alone clears this thread's request.
        unsafe {
            SetThreadExecutionState(flags);
        }
    }
}

/// Inputs to one tick.
#[derive(Clone, Debug, Default)]
pub struct Inputs {
    pub now: i64,
    /// Lower-case exe names of running processes (only needed for WhileApp).
    pub running: Option<HashSet<String>>,
    /// Current network and disk throughput (only needed for WhileActivity).
    pub net_bps: Option<f64>,
    pub disk_bps: Option<f64>,
}

/// How long to wait for an app to show up before giving up.
const APP_WAIT_SECS: i64 = 30;

/// The keep-awake state machine.
#[derive(Default)]
pub struct Awake {
    request: Option<AwakeRequest>,
    started: i64,
    /// The app was seen running at least once.
    app_seen: bool,
    /// Last time activity was above a threshold.
    last_busy: i64,
    /// Last measured throughput (for the status text).
    last_bps: f64,
    /// What is applied right now.
    applied: (bool, bool),
    ended: Option<String>,
}

impl Awake {
    pub fn new() -> Awake {
        Awake::default()
    }

    fn set(&mut self, want: (bool, bool), exec: &mut dyn Executor) {
        if want != self.applied {
            exec.apply(want.0, want.1);
            self.applied = want;
        }
    }

    pub fn start(&mut self, req: AwakeRequest, now: i64, exec: &mut dyn Executor) {
        let display = req.keep_display;
        self.request = Some(req);
        self.started = now;
        self.app_seen = false;
        self.last_busy = now;
        self.last_bps = 0.0;
        self.ended = None;
        self.set((true, display), exec);
    }

    pub fn stop(&mut self, exec: &mut dyn Executor) {
        self.request = None;
        self.set((false, false), exec);
    }

    fn end(&mut self, why: String, exec: &mut dyn Executor) {
        self.stop(exec);
        self.ended = Some(why);
    }

    /// Re-evaluates (the engine calls it every few seconds). Ends the
    /// request when its condition is over and releases the state.
    pub fn tick(&mut self, inputs: &Inputs, exec: &mut dyn Executor) -> AwakeStatus {
        let now = inputs.now;
        if let Some(req) = self.request.clone() {
            match &req.mode {
                AwakeMode::Indefinite => {}
                AwakeMode::Until(t) => {
                    if now >= *t {
                        self.end("The keep-awake time is over.".into(), exec);
                    }
                }
                AwakeMode::WhileApp(exe) => {
                    if let Some(running) = &inputs.running {
                        let up = running.contains(&exe.to_lowercase());
                        if up {
                            self.app_seen = true;
                        } else if self.app_seen {
                            self.end(format!("{exe} has closed."), exec);
                        } else if now - self.started >= APP_WAIT_SECS {
                            self.end(format!("{exe} is not running."), exec);
                        }
                    }
                }
                AwakeMode::WhileActivity {
                    net_bps,
                    disk_bps,
                    grace_secs,
                } => {
                    let net = inputs.net_bps.unwrap_or(0.0);
                    let disk = inputs.disk_bps.unwrap_or(0.0);
                    if inputs.net_bps.is_some() || inputs.disk_bps.is_some() {
                        self.last_bps = net + disk;
                        if net >= *net_bps as f64 || disk >= *disk_bps as f64 {
                            self.last_busy = now;
                        } else if now - self.last_busy >= *grace_secs as i64 {
                            self.end("Downloads and copies have finished.".into(), exec);
                        }
                    }
                }
            }
        }
        if self.request.is_some() {
            let display = self.request.as_ref().is_some_and(|r| r.keep_display);
            // Re-assert in case something cleared it.
            self.set((true, display), exec);
        }
        self.status(now)
    }

    pub fn status(&self, now: i64) -> AwakeStatus {
        let Some(req) = &self.request else {
            return AwakeStatus {
                text: "Off".into(),
                ended: self.ended.clone(),
                ..AwakeStatus::default()
            };
        };
        let screen = if req.keep_display {
            " (screen stays on)"
        } else {
            ""
        };
        let (ends_at, text) = match &req.mode {
            AwakeMode::Indefinite => (
                None,
                "Keeping the PC awake until you turn it off".to_string(),
            ),
            AwakeMode::Until(t) => {
                let left = (t - now).max(0) as u64;
                let text = if left < 3600 * 3 {
                    format!(
                        "Keeping the PC awake for {} more",
                        util::fmt_duration(left.div_ceil(60) * 60)
                    )
                } else {
                    format!("Keeping the PC awake until {}", util::fmt_time(*t))
                };
                (Some(*t), text)
            }
            AwakeMode::WhileApp(exe) => {
                let text = if self.app_seen {
                    format!("Keeping the PC awake while {exe} runs")
                } else {
                    format!("Waiting for {exe} to start")
                };
                (None, text)
            }
            AwakeMode::WhileActivity { .. } => (
                None,
                format!(
                    "Keeping the PC awake while downloads or copies are active (now {}/s)",
                    util::fmt_bytes(self.last_bps as u64)
                ),
            ),
        };
        AwakeStatus {
            active: true,
            request: Some(req.clone()),
            started: Some(self.started),
            ends_at,
            text: format!("{text}{screen}"),
            ended: None,
        }
    }

    /// Whether the engine must gather running processes / throughput.
    pub fn needs_processes(&self) -> bool {
        matches!(
            self.request.as_ref().map(|r| &r.mode),
            Some(AwakeMode::WhileApp(_))
        )
    }

    pub fn needs_activity(&self) -> bool {
        matches!(
            self.request.as_ref().map(|r| &r.mode),
            Some(AwakeMode::WhileActivity { .. })
        )
    }

    pub fn is_active(&self) -> bool {
        self.request.is_some()
    }
}

/// Parses "30m", "2h", "1h30m", "90" (minutes), "45s", "1.5h" into seconds.
pub fn parse_duration(text: &str) -> Option<u32> {
    let t = text.trim().to_lowercase().replace(' ', "");
    if t.is_empty() {
        return None;
    }
    if let Ok(min) = t.parse::<f64>() {
        return (min > 0.0 && min <= 60.0 * 24.0 * 30.0).then(|| (min * 60.0).round() as u32);
    }
    let mut total = 0f64;
    let mut num = String::new();
    let mut any = false;
    let mut chars = t.chars().peekable();
    while let Some(c) = chars.next() {
        if c.is_ascii_digit() || c == '.' {
            num.push(c);
            continue;
        }
        let v: f64 = num.parse().ok()?;
        num.clear();
        // Allow "min", "mins", "hr", "hours", "sec".
        let mut unit = c.to_string();
        while let Some(&n) = chars.peek() {
            if n.is_ascii_alphabetic() {
                unit.push(n);
                chars.next();
            } else {
                break;
            }
        }
        let mult = match unit.as_str() {
            "h" | "hr" | "hrs" | "hour" | "hours" => 3600.0,
            "m" | "min" | "mins" | "minute" | "minutes" => 60.0,
            "s" | "sec" | "secs" | "second" | "seconds" => 1.0,
            "d" | "day" | "days" => 86_400.0,
            _ => return None,
        };
        total += v * mult;
        any = true;
    }
    if !num.is_empty() || !any {
        return None;
    }
    (1.0..=30.0 * 86_400.0)
        .contains(&total)
        .then_some(total.round() as u32)
}

/// Parses "18:30", "6:30pm" or "6pm" into the next such local time after `now`.
pub fn parse_until(text: &str, now: i64) -> Option<i64> {
    let t = text.trim().to_lowercase().replace(' ', "");
    let (body, pm, am) = if let Some(b) = t.strip_suffix("pm") {
        (b.to_string(), true, false)
    } else if let Some(b) = t.strip_suffix("am") {
        (b.to_string(), false, true)
    } else {
        (t.clone(), false, false)
    };
    let (h, m) = match body.split_once([':', '.']) {
        Some((h, m)) if m.len() == 2 => (h.parse::<u32>().ok()?, m.parse::<u32>().ok()?),
        Some(_) => return None,
        None => (body.parse::<u32>().ok()?, 0),
    };
    if m > 59 {
        return None;
    }
    let h = if pm || am {
        if !(1..=12).contains(&h) {
            return None;
        }
        match (h, pm) {
            (12, false) => 0,
            (12, true) => 12,
            (h, true) => h + 12,
            (h, false) => h,
        }
    } else {
        if h > 23 {
            return None;
        }
        h
    };
    Some(util::next_local_time(now, h, m))
}

/// Total network and disk throughput, measured between calls.
#[derive(Default)]
pub struct ActivityMeter {
    last: Option<(std::time::Instant, u64, u64)>,
}

impl ActivityMeter {
    pub fn new() -> ActivityMeter {
        ActivityMeter::default()
    }

    /// (network bytes/s, disk bytes/s) since the last call; None on the
    /// first call or when a counter is unavailable.
    pub fn sample(&mut self) -> (Option<f64>, Option<f64>) {
        let net = network_bytes();
        let disk = disk_bytes();
        let now = std::time::Instant::now();
        let result = match self.last {
            Some((at, n0, d0)) => {
                let secs = now.duration_since(at).as_secs_f64().max(0.001);
                (
                    net.map(|n| n.saturating_sub(n0) as f64 / secs),
                    disk.map(|d| d.saturating_sub(d0) as f64 / secs),
                )
            }
            None => (None, None),
        };
        self.last = Some((now, net.unwrap_or(0), disk.unwrap_or(0)));
        result
    }
}

/// Bytes sent and received on all non-loopback interfaces that are up.
fn network_bytes() -> Option<u64> {
    use windows::Win32::NetworkManagement::IpHelper::{FreeMibTable, GetIfTable2, MIB_IF_TABLE2};
    use windows::Win32::NetworkManagement::Ndis::IfOperStatusUp;
    let mut table: *mut MIB_IF_TABLE2 = std::ptr::null_mut();
    // SAFETY: Windows allocates the table; it is read within its row count
    // and freed with FreeMibTable.
    unsafe {
        if GetIfTable2(&mut table).is_err() || table.is_null() {
            return None;
        }
        let rows =
            std::slice::from_raw_parts((*table).Table.as_ptr(), (*table).NumEntries as usize);
        let mut total = 0u64;
        for row in rows {
            // 24 = IF_TYPE_SOFTWARE_LOOPBACK. Skip filter/virtual duplicates.
            if row.Type == 24 || row.OperStatus != IfOperStatusUp {
                continue;
            }
            if row.InterfaceAndOperStatusFlags._bitfield & 1 == 0 {
                // Not a hardware interface.
                continue;
            }
            total = total
                .saturating_add(row.InOctets)
                .saturating_add(row.OutOctets);
        }
        FreeMibTable(table as *const _);
        Some(total)
    }
}

/// Bytes read and written by all processes so far (I/O transfer counters
/// summed over processes, a light stand-in for a disk counter).
fn disk_bytes() -> Option<u64> {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };
    use windows::Win32::System::Threading::{
        GetProcessIoCounters, OpenProcess, IO_COUNTERS, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    // SAFETY: a process snapshot walked with correctly sized entries; every
    // handle opened is closed.
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0).ok()?;
        let mut entry = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        let mut total = 0u64;
        let mut ok = Process32FirstW(snap, &mut entry).is_ok();
        while ok {
            if entry.th32ProcessID != 0 {
                if let Ok(h) = OpenProcess(
                    PROCESS_QUERY_LIMITED_INFORMATION,
                    false,
                    entry.th32ProcessID,
                ) {
                    let mut io = IO_COUNTERS::default();
                    if GetProcessIoCounters(h, &mut io).is_ok() {
                        // Read and write transfers include file and network I/O;
                        // the network part is small next to copies and downloads.
                        total = total
                            .saturating_add(io.ReadTransferCount)
                            .saturating_add(io.WriteTransferCount);
                    }
                    let _ = CloseHandle(h);
                }
            }
            ok = Process32NextW(snap, &mut entry).is_ok();
        }
        let _ = CloseHandle(snap);
        Some(total)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Recorder(Vec<(bool, bool)>);

    impl Executor for Recorder {
        fn apply(&mut self, system: bool, display: bool) {
            self.0.push((system, display));
        }
    }

    fn inputs(now: i64) -> Inputs {
        Inputs {
            now,
            ..Inputs::default()
        }
    }

    #[test]
    fn timed_request_expires_and_releases() {
        let mut ex = Recorder::default();
        let mut a = Awake::new();
        a.start(
            AwakeRequest {
                mode: AwakeMode::Until(1000 + 1800),
                keep_display: false,
            },
            1000,
            &mut ex,
        );
        assert_eq!(ex.0, vec![(true, false)]);
        let s = a.tick(&inputs(1000 + 300), &mut ex);
        assert!(s.active);
        assert_eq!(s.ends_at, Some(2800));
        assert!(s.text.contains("25 min more"), "{}", s.text);
        // Ticks don't re-apply the same state.
        assert_eq!(ex.0.len(), 1);
        let s = a.tick(&inputs(2800), &mut ex);
        assert!(!s.active);
        assert_eq!(ex.0, vec![(true, false), (false, false)]);
        assert!(s.ended.is_some());
        assert_eq!(s.text, "Off");
    }

    #[test]
    fn display_flag_and_stop() {
        let mut ex = Recorder::default();
        let mut a = Awake::new();
        a.start(
            AwakeRequest {
                mode: AwakeMode::Indefinite,
                keep_display: true,
            },
            0,
            &mut ex,
        );
        let s = a.tick(&inputs(100_000), &mut ex);
        assert!(s.active && s.text.contains("screen stays on"));
        // Switching to a request without the display changes the state once.
        a.start(
            AwakeRequest {
                mode: AwakeMode::Indefinite,
                keep_display: false,
            },
            5,
            &mut ex,
        );
        a.stop(&mut ex);
        a.stop(&mut ex);
        assert_eq!(ex.0, vec![(true, true), (true, false), (false, false)]);
        assert!(!a.is_active());
    }

    #[test]
    fn while_app_runs() {
        let mut ex = Recorder::default();
        let mut a = Awake::new();
        a.start(
            AwakeRequest {
                mode: AwakeMode::WhileApp("backup.exe".into()),
                keep_display: false,
            },
            0,
            &mut ex,
        );
        assert!(a.needs_processes() && !a.needs_activity());
        let with = |names: &[&str], now| Inputs {
            now,
            running: Some(names.iter().map(|s| s.to_string()).collect()),
            ..Inputs::default()
        };
        // Not started yet: waiting, within the grace period.
        assert!(a
            .tick(&with(&["explorer.exe"], 10), &mut ex)
            .text
            .contains("Waiting"));
        assert!(a
            .tick(&with(&["backup.exe"], 15), &mut ex)
            .text
            .contains("while backup.exe runs"));
        let s = a.tick(&with(&["explorer.exe"], 500), &mut ex);
        assert!(!s.active);
        assert_eq!(s.ended.as_deref(), Some("backup.exe has closed."));
        // Never appears: gives up after the wait.
        a.start(
            AwakeRequest {
                mode: AwakeMode::WhileApp("nothere.exe".into()),
                keep_display: false,
            },
            1000,
            &mut ex,
        );
        assert!(a.tick(&with(&[], 1020), &mut ex).active);
        let s = a.tick(&with(&[], 1031), &mut ex);
        assert!(!s.active && s.ended.unwrap().contains("not running"));
        // Without a process list nothing is decided.
        a.start(
            AwakeRequest {
                mode: AwakeMode::WhileApp("x.exe".into()),
                keep_display: false,
            },
            0,
            &mut ex,
        );
        assert!(a.tick(&inputs(10_000), &mut ex).active);
    }

    #[test]
    fn while_activity_with_grace() {
        let mut ex = Recorder::default();
        let mut a = Awake::new();
        a.start(
            AwakeRequest {
                mode: AwakeMode::WhileActivity {
                    net_bps: 100_000,
                    disk_bps: 1_000_000,
                    grace_secs: 120,
                },
                keep_display: false,
            },
            0,
            &mut ex,
        );
        assert!(a.needs_activity());
        let io = |now, net: f64, disk: f64| Inputs {
            now,
            net_bps: Some(net),
            disk_bps: Some(disk),
            ..Inputs::default()
        };
        assert!(a.tick(&io(10, 500_000.0, 0.0), &mut ex).active);
        // Quiet for less than the grace period.
        assert!(a.tick(&io(100, 10.0, 10.0), &mut ex).active);
        // Disk busy resets the grace.
        assert!(a.tick(&io(125, 0.0, 2_000_000.0), &mut ex).active);
        assert!(a.tick(&io(240, 0.0, 0.0), &mut ex).active);
        let s = a.tick(&io(245, 0.0, 0.0), &mut ex);
        assert!(!s.active);
        assert_eq!(ex.0.last(), Some(&(false, false)));
    }

    #[test]
    fn durations_and_times() {
        assert_eq!(parse_duration("30m"), Some(1800));
        assert_eq!(parse_duration("2h"), Some(7200));
        assert_eq!(parse_duration("1h30m"), Some(5400));
        assert_eq!(parse_duration("1h 30min"), Some(5400));
        assert_eq!(parse_duration("90"), Some(5400));
        assert_eq!(parse_duration("45s"), Some(45));
        assert_eq!(parse_duration("1.5h"), Some(5400));
        assert_eq!(parse_duration(""), None);
        assert_eq!(parse_duration("abc"), None);
        assert_eq!(parse_duration("10x"), None);
        assert_eq!(parse_duration("0"), None);
        assert_eq!(parse_duration("5h3"), None);
        util::set_fixed_offset(Some(0));
        let now = 10 * 3600; // 10:00 on day 0.
        assert_eq!(parse_until("18:30", now), Some(18 * 3600 + 1800));
        assert_eq!(parse_until("6:30pm", now), Some(18 * 3600 + 1800));
        assert_eq!(parse_until("6pm", now), Some(18 * 3600));
        assert_eq!(parse_until("9am", now), Some(86_400 + 9 * 3600));
        assert_eq!(parse_until("12am", now), Some(86_400));
        assert_eq!(parse_until("12pm", now), Some(12 * 3600));
        assert_eq!(parse_until("25:00", now), None);
        assert_eq!(parse_until("13pm", now), None);
        assert_eq!(parse_until("10:7", now), None);
        util::set_fixed_offset(None);
    }

    #[test]
    fn activity_meter_reads() {
        let mut m = ActivityMeter::new();
        assert_eq!(m.sample(), (None, None));
        let (net, _disk) = m.sample();
        assert!(net.unwrap_or(0.0) >= 0.0);
    }
}
