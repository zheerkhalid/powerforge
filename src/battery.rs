//! Live battery state: charge, rate, capacities, cycle count, chemistry.
//! Read from the battery driver (IOCTL_BATTERY_*) and GetSystemPowerStatus.
//! Serial numbers and device paths are never read into public data.

use serde::{Deserialize, Serialize};
use std::sync::Mutex;
use std::time::Instant;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ChargeState {
    #[default]
    Unknown,
    Charging,
    Discharging,
    /// Plugged in and full.
    Full,
    /// Plugged in but not charging (for example held at a charge limit).
    Idle,
}

impl ChargeState {
    pub fn label(self) -> &'static str {
        match self {
            ChargeState::Unknown => "Unknown",
            ChargeState::Charging => "Charging",
            ChargeState::Discharging => "On battery",
            ChargeState::Full => "Fully charged",
            ChargeState::Idle => "Plugged in, not charging",
        }
    }
}

/// One reading of the (first) battery.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct BatteryInfo {
    /// Charge, 0..=100.
    pub percent: f32,
    pub state: ChargeState,
    /// Mains connected.
    pub ac_online: bool,
    /// Milliwatts: positive while charging, negative while discharging.
    pub rate_mw: Option<i32>,
    pub voltage_mv: Option<u32>,
    pub remaining_mwh: Option<u32>,
    pub full_mwh: Option<u32>,
    pub design_mwh: Option<u32>,
    pub cycle_count: Option<u32>,
    /// "Lithium-ion", "Lithium polymer", ...
    pub chemistry: String,
    pub manufacturer: String,
    /// The model name the driver reports (never the serial number).
    pub name: String,
    pub temperature_c: Option<f32>,
    /// Seconds to empty (discharging) or to full (charging).
    pub seconds_left: Option<u32>,
    /// Windows battery saver is on.
    pub saver_on: bool,
    /// Number of batteries found (capacities and rates are summed when
    /// there are several).
    pub count: u32,
}

const IOCTL_BATTERY_QUERY_TAG: u32 = 0x0029_4040;
const IOCTL_BATTERY_QUERY_INFORMATION: u32 = 0x0029_4044;
const IOCTL_BATTERY_QUERY_STATUS: u32 = 0x0029_404C;

const LEVEL_INFORMATION: u32 = 0;
const LEVEL_TEMPERATURE: u32 = 2;
const LEVEL_DEVICE_NAME: u32 = 4;
const LEVEL_MANUFACTURE_NAME: u32 = 6;

const POWER_ON_LINE: u32 = 1;
const DISCHARGING: u32 = 2;
const CHARGING: u32 = 4;
const CAPACITY_RELATIVE: u32 = 0x4000_0000;
const UNKNOWN: u32 = 0xFFFF_FFFF;
const UNKNOWN_RATE: i32 = i32::MIN;
const MAX_MWH: u32 = 1_000_000;
const MAX_MV: u32 = 100_000;
const MAX_SECONDS: f64 = 14.0 * 86_400.0;

/// The numbers the battery driver hands out, before interpretation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RawBattery {
    pub capabilities: u32,
    pub chemistry: [u8; 4],
    pub design: u32,
    pub full: u32,
    pub cycle_count: u32,
    pub power_state: u32,
    pub remaining: u32,
    pub voltage: u32,
    pub rate: i32,
    pub ac_online: Option<bool>,
    pub system_percent: Option<u8>,
}

impl Default for RawBattery {
    fn default() -> Self {
        RawBattery {
            capabilities: 0,
            chemistry: [0; 4],
            design: UNKNOWN,
            full: UNKNOWN,
            cycle_count: 0,
            power_state: 0,
            remaining: UNKNOWN,
            voltage: UNKNOWN,
            rate: UNKNOWN_RATE,
            ac_online: None,
            system_percent: None,
        }
    }
}

/// The chemistry code of the battery driver in words.
pub fn chemistry_name(code: &[u8]) -> String {
    let text: String = code
        .iter()
        .take_while(|&&b| b != 0)
        .filter(|b| b.is_ascii_graphic() || **b == b' ')
        .map(|&b| b as char)
        .collect();
    let text = text.trim();
    match text.to_ascii_uppercase().as_str() {
        "LION" | "LI-I" | "LIIO" | "LI" => "Lithium-ion".into(),
        "LIP" | "LIPO" | "LI-P" => "Lithium polymer".into(),
        "NIMH" => "Nickel-metal hydride".into(),
        "NICD" => "Nickel-cadmium".into(),
        "PBAC" => "Lead-acid".into(),
        "RAM" => "Rechargeable alkaline".into(),
        _ => text.to_string(),
    }
}

fn state_from_bits(bits: u32, percent: Option<f32>, ac_online: Option<bool>) -> ChargeState {
    let full = percent.is_some_and(|p| p >= 99.5);
    if bits & CHARGING != 0 {
        ChargeState::Charging
    } else if bits & DISCHARGING != 0 {
        ChargeState::Discharging
    } else if bits & POWER_ON_LINE != 0 || ac_online == Some(true) {
        if full {
            ChargeState::Full
        } else {
            ChargeState::Idle
        }
    } else if ac_online == Some(false) {
        ChargeState::Discharging
    } else {
        ChargeState::Unknown
    }
}

/// Seconds until empty (discharging) or full (charging).
pub fn seconds_left(
    state: ChargeState,
    remaining: Option<u32>,
    full: Option<u32>,
    rate_mw: Option<i32>,
) -> Option<u32> {
    let rate = f64::from(rate_mw?.unsigned_abs());
    if rate < 50.0 {
        return None;
    }
    let mwh = match state {
        ChargeState::Discharging => f64::from(remaining?),
        ChargeState::Charging => f64::from(full?.saturating_sub(remaining?)),
        _ => return None,
    };
    let s = mwh / rate * 3600.0;
    (s.is_finite() && s <= MAX_SECONDS).then_some(s as u32)
}

/// Interprets the raw numbers: sentinels become `None`, relative-unit
/// batteries get a percentage only, absurd values are dropped.
pub fn interpret(raw: &RawBattery) -> BatteryInfo {
    let relative = raw.capabilities & CAPACITY_RELATIVE != 0;
    let capacity = |v: u32| (v != UNKNOWN && v > 0 && v <= MAX_MWH && !relative).then_some(v);
    let design = capacity(raw.design);
    let full = capacity(raw.full);
    let remaining = (raw.remaining != UNKNOWN && raw.remaining <= MAX_MWH && !relative)
        .then_some(raw.remaining);
    let pct = match (remaining, full) {
        (Some(r), Some(f)) if f > 0 => Some((r as f32 / f as f32 * 100.0).clamp(0.0, 100.0)),
        _ => None,
    }
    .or(raw.system_percent.filter(|&p| p <= 100).map(f32::from));
    let state = state_from_bits(raw.power_state, pct, raw.ac_online);
    let rate =
        (raw.rate != UNKNOWN_RATE && raw.rate.unsigned_abs() <= MAX_MWH && !relative).then(|| {
            match state {
                ChargeState::Discharging => -raw.rate.abs(),
                ChargeState::Charging => raw.rate.abs(),
                _ => raw.rate,
            }
        });
    BatteryInfo {
        percent: pct.unwrap_or(0.0),
        state,
        ac_online: raw
            .ac_online
            .unwrap_or(raw.power_state & POWER_ON_LINE != 0),
        rate_mw: rate,
        voltage_mv: (raw.voltage != UNKNOWN && raw.voltage > 0 && raw.voltage <= MAX_MV)
            .then_some(raw.voltage),
        remaining_mwh: remaining,
        full_mwh: full,
        design_mwh: design,
        cycle_count: (raw.cycle_count > 0 && raw.cycle_count <= 100_000).then_some(raw.cycle_count),
        chemistry: chemistry_name(&raw.chemistry),
        seconds_left: seconds_left(state, remaining, full, rate),
        count: 1,
        ..Default::default()
    }
}

/// Adds a second battery to the first: capacities and rates are summed and
/// the charge is recomputed.
pub fn combine(mut a: BatteryInfo, b: &BatteryInfo) -> BatteryInfo {
    let sum = |x: Option<u32>, y: Option<u32>| match (x, y) {
        (Some(x), Some(y)) => Some(x + y),
        (x, None) => x,
        (None, y) => y,
    };
    a.design_mwh = sum(a.design_mwh, b.design_mwh);
    a.full_mwh = sum(a.full_mwh, b.full_mwh);
    a.remaining_mwh = sum(a.remaining_mwh, b.remaining_mwh);
    a.rate_mw = match (a.rate_mw, b.rate_mw) {
        (Some(x), Some(y)) => Some(x + y),
        (x, None) => x,
        (None, y) => y,
    };
    if let (Some(r), Some(f)) = (a.remaining_mwh, a.full_mwh) {
        if f > 0 {
            a.percent = (r as f32 / f as f32 * 100.0).clamp(0.0, 100.0);
        }
    }
    if a.state != b.state && b.state == ChargeState::Charging {
        a.state = ChargeState::Charging;
    }
    a.seconds_left = seconds_left(a.state, a.remaining_mwh, a.full_mwh, a.rate_mw);
    a.count += b.count;
    a
}

// ----------------------------------------------------------------------------------
// Reading the device
// ----------------------------------------------------------------------------------

mod device {
    use std::ffi::c_void;
    use windows::core::PCWSTR;
    use windows::Win32::Devices::DeviceAndDriverInstallation::{
        SetupDiDestroyDeviceInfoList, SetupDiEnumDeviceInterfaces, SetupDiGetClassDevsW,
        SetupDiGetDeviceInterfaceDetailW, DIGCF_DEVICEINTERFACE, DIGCF_PRESENT,
        GUID_DEVCLASS_BATTERY, SP_DEVICE_INTERFACE_DATA, SP_DEVICE_INTERFACE_DETAIL_DATA_W,
    };
    use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND};
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };
    use windows::Win32::System::IO::DeviceIoControl;

    pub struct Handle(HANDLE);

    impl Drop for Handle {
        fn drop(&mut self) {
            // SAFETY: opened by CreateFileW, closed exactly once.
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }

    pub fn open(path: &str) -> Option<Handle> {
        let w = crate::sys::wide(path);
        for access in [0xC000_0000u32, 0x8000_0000, 0] {
            // SAFETY: the path is NUL-terminated and outlives the call.
            let r = unsafe {
                CreateFileW(
                    PCWSTR(w.as_ptr()),
                    access,
                    FILE_SHARE_READ | FILE_SHARE_WRITE,
                    None,
                    OPEN_EXISTING,
                    FILE_FLAGS_AND_ATTRIBUTES(0),
                    HANDLE::default(),
                )
            };
            if let Ok(h) = r {
                if !h.is_invalid() {
                    return Some(Handle(h));
                }
            }
        }
        None
    }

    pub fn ioctl(h: &Handle, code: u32, input: &[u8], output: &mut [u8]) -> Option<usize> {
        let mut returned = 0u32;
        // SAFETY: both buffers are live slices and their true lengths are passed.
        let r = unsafe {
            DeviceIoControl(
                h.0,
                code,
                Some(input.as_ptr() as *const c_void),
                input.len() as u32,
                Some(output.as_mut_ptr() as *mut c_void),
                output.len() as u32,
                Some(&mut returned),
                None,
            )
        };
        r.ok().map(|_| (returned as usize).min(output.len()))
    }

    /// Device interface paths of every battery that is present.
    pub fn paths() -> Vec<String> {
        let mut paths = Vec::new();
        // SAFETY: the device information set is destroyed before returning;
        // every structure passed has its size field set.
        unsafe {
            let Ok(set) = SetupDiGetClassDevsW(
                Some(&GUID_DEVCLASS_BATTERY),
                PCWSTR::null(),
                HWND::default(),
                DIGCF_PRESENT | DIGCF_DEVICEINTERFACE,
            ) else {
                return paths;
            };
            for index in 0..8u32 {
                let mut data = SP_DEVICE_INTERFACE_DATA {
                    cbSize: std::mem::size_of::<SP_DEVICE_INTERFACE_DATA>() as u32,
                    ..Default::default()
                };
                if SetupDiEnumDeviceInterfaces(set, None, &GUID_DEVCLASS_BATTERY, index, &mut data)
                    .is_err()
                {
                    break;
                }
                let mut needed = 0u32;
                let _ =
                    SetupDiGetDeviceInterfaceDetailW(set, &data, None, 0, Some(&mut needed), None);
                if !(8..=8192).contains(&needed) {
                    continue;
                }
                let mut buf = vec![0u32; (needed as usize).div_ceil(4) + 1];
                buf[0] = std::mem::size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>() as u32;
                if SetupDiGetDeviceInterfaceDetailW(
                    set,
                    &data,
                    Some(buf.as_mut_ptr() as *mut SP_DEVICE_INTERFACE_DETAIL_DATA_W),
                    needed,
                    None,
                    None,
                )
                .is_ok()
                {
                    let units: Vec<u16> = buf[1..]
                        .iter()
                        .flat_map(|w| [(w & 0xFFFF) as u16, (w >> 16) as u16])
                        .collect();
                    let path = crate::sys::from_wide(&units);
                    if !path.is_empty() {
                        paths.push(path);
                    }
                }
            }
            let _ = SetupDiDestroyDeviceInfoList(set);
        }
        paths
    }
}

fn le_u32(b: &[u8], at: usize) -> Option<u32> {
    let s = b.get(at..at + 4)?;
    Some(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

fn query_info(h: &device::Handle, tag: u32, level: u32, size: usize) -> Option<Vec<u8>> {
    let mut input = [0u8; 12];
    input[..4].copy_from_slice(&tag.to_le_bytes());
    input[4..8].copy_from_slice(&level.to_le_bytes());
    let mut out = vec![0u8; size];
    let n = device::ioctl(h, IOCTL_BATTERY_QUERY_INFORMATION, &input, &mut out)?;
    out.truncate(n);
    Some(out)
}

fn query_text(h: &device::Handle, tag: u32, level: u32) -> String {
    query_info(h, tag, level, 512)
        .map(|b| {
            let units: Vec<u16> = b
                .as_chunks::<2>()
                .0
                .iter()
                .map(|c| u16::from_le_bytes(*c))
                .collect();
            crate::sys::from_wide(&units).trim().to_string()
        })
        .unwrap_or_default()
}

fn system_status() -> Option<windows::Win32::System::Power::SYSTEM_POWER_STATUS> {
    use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
    let mut s = SYSTEM_POWER_STATUS::default();
    // SAFETY: plain out-parameter call.
    unsafe { GetSystemPowerStatus(&mut s) }.ok()?;
    Some(s)
}

fn read_device(
    path: &str,
    system: &windows::Win32::System::Power::SYSTEM_POWER_STATUS,
) -> Option<BatteryInfo> {
    let h = device::open(path)?;
    let mut tag_out = [0u8; 4];
    let n = device::ioctl(
        &h,
        IOCTL_BATTERY_QUERY_TAG,
        &0u32.to_le_bytes(),
        &mut tag_out,
    )?;
    let tag = le_u32(&tag_out[..n], 0).filter(|&t| t != 0)?;
    let mut raw = RawBattery {
        ac_online: match system.ACLineStatus {
            0 => Some(false),
            1 => Some(true),
            _ => None,
        },
        system_percent: (system.BatteryLifePercent <= 100).then_some(system.BatteryLifePercent),
        ..Default::default()
    };
    if let Some(b) = query_info(&h, tag, LEVEL_INFORMATION, 64) {
        if b.len() >= 36 {
            raw.capabilities = le_u32(&b, 0).unwrap_or(0);
            raw.chemistry.copy_from_slice(&b[8..12]);
            raw.design = le_u32(&b, 12).unwrap_or(UNKNOWN);
            raw.full = le_u32(&b, 16).unwrap_or(UNKNOWN);
            raw.cycle_count = le_u32(&b, 32).unwrap_or(0);
        }
    }
    // BATTERY_WAIT_STATUS with a zero timeout: answer immediately.
    let mut wait = [0u8; 20];
    wait[..4].copy_from_slice(&tag.to_le_bytes());
    let mut status = [0u8; 16];
    if let Some(n) = device::ioctl(&h, IOCTL_BATTERY_QUERY_STATUS, &wait, &mut status) {
        let s = &status[..n];
        raw.power_state = le_u32(s, 0).unwrap_or(0);
        raw.remaining = le_u32(s, 4).unwrap_or(UNKNOWN);
        raw.voltage = le_u32(s, 8).unwrap_or(UNKNOWN);
        raw.rate = le_u32(s, 12).map(|v| v as i32).unwrap_or(UNKNOWN_RATE);
    }
    let mut info = interpret(&raw);
    info.name = query_text(&h, tag, LEVEL_DEVICE_NAME);
    info.manufacturer = query_text(&h, tag, LEVEL_MANUFACTURE_NAME);
    info.temperature_c = query_info(&h, tag, LEVEL_TEMPERATURE, 8)
        .and_then(|b| le_u32(&b, 0))
        .and_then(|k| {
            let c = k as f32 / 10.0 - 273.15;
            (k != UNKNOWN && (-40.0..=120.0).contains(&c)).then_some(c)
        });
    Some(info)
}

fn from_system(system: &windows::Win32::System::Power::SYSTEM_POWER_STATUS) -> BatteryInfo {
    let percent =
        (system.BatteryLifePercent <= 100).then_some(f32::from(system.BatteryLifePercent));
    let ac = system.ACLineStatus == 1;
    let state = if system.BatteryFlag != 255 && system.BatteryFlag & 8 != 0 {
        ChargeState::Charging
    } else {
        state_from_bits(0, percent, (system.ACLineStatus <= 1).then_some(ac))
    };
    BatteryInfo {
        percent: percent.unwrap_or(0.0),
        state,
        ac_online: ac,
        count: 1,
        ..Default::default()
    }
}

struct Cache {
    paths: Option<Vec<String>>,
    searched: Option<Instant>,
}

static CACHE: Mutex<Cache> = Mutex::new(Cache {
    paths: None,
    searched: None,
});

/// The battery (summed when there are several), or `None` on machines
/// without one. Cheap enough to call every few seconds.
pub fn read() -> Option<BatteryInfo> {
    let system = system_status()?;
    // 128: "no system battery". The cheap exit for desktops.
    if system.BatteryFlag != 255 && system.BatteryFlag & 128 != 0 {
        return None;
    }
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    let stale = cache.searched.is_none_or(|at| at.elapsed().as_secs() > 60);
    if cache.paths.as_ref().is_none_or(|p| p.is_empty()) && stale {
        cache.paths = Some(device::paths());
        cache.searched = Some(Instant::now());
    }
    let paths = cache.paths.clone().unwrap_or_default();
    drop(cache);
    let mut found: Option<BatteryInfo> = None;
    for path in &paths {
        if let Some(info) = read_device(path, &system) {
            found = Some(match found {
                None => info,
                Some(first) => combine(first, &info),
            });
        }
    }
    let mut info = match found {
        Some(i) => i,
        None => {
            if let Ok(mut c) = CACHE.lock() {
                c.paths = None;
            }
            if system.BatteryFlag == 255 || system.BatteryLifePercent > 100 {
                return None;
            }
            from_system(&system)
        }
    };
    info.saver_on = system.SystemStatusFlag == 1;
    if info.seconds_left.is_none()
        && info.state == ChargeState::Discharging
        && system.BatteryLifeTime != UNKNOWN
        && (system.BatteryLifeTime as f64) < MAX_SECONDS
    {
        info.seconds_left = Some(system.BatteryLifeTime);
    }
    Some(info)
}

/// Whether mains power is connected (`None` when Windows does not know).
/// Works on desktops too.
pub fn ac_online() -> Option<bool> {
    match system_status()?.ACLineStatus {
        0 => Some(false),
        1 => Some(true),
        _ => None,
    }
}

/// An invented laptop battery for `--demo` (87 % health, discharging).
pub fn demo() -> BatteryInfo {
    BatteryInfo {
        percent: 64.0,
        state: ChargeState::Discharging,
        ac_online: false,
        rate_mw: Some(-9_800),
        voltage_mv: Some(11_620),
        remaining_mwh: Some(31_180),
        full_mwh: Some(48_720),
        design_mwh: Some(56_000),
        cycle_count: Some(312),
        chemistry: "Lithium-ion".into(),
        manufacturer: "Northwind Cells".into(),
        name: "NW-56 4-cell".into(),
        temperature_c: Some(31.0),
        seconds_left: Some(31_180 * 3600 / 9_800),
        saver_on: false,
        count: 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw() -> RawBattery {
        RawBattery {
            capabilities: 0x8000_0000,
            chemistry: *b"LION",
            design: 60_000,
            full: 54_000,
            cycle_count: 212,
            power_state: DISCHARGING,
            remaining: 27_000,
            voltage: 11_400,
            rate: -9_000,
            ac_online: Some(false),
            system_percent: Some(50),
        }
    }

    #[test]
    fn discharging() {
        let b = interpret(&raw());
        assert_eq!(b.chemistry, "Lithium-ion");
        assert_eq!(b.percent, 50.0);
        assert_eq!(b.state, ChargeState::Discharging);
        assert_eq!(b.rate_mw, Some(-9_000));
        assert_eq!(b.seconds_left, Some(3 * 3600));
        assert_eq!(b.cycle_count, Some(212));
        // Some drivers report the discharge rate as a positive number.
        let b = interpret(&RawBattery {
            rate: 9_000,
            ..raw()
        });
        assert_eq!(b.rate_mw, Some(-9_000));
    }

    #[test]
    fn charging_full_idle_and_sentinels() {
        let b = interpret(&RawBattery {
            power_state: POWER_ON_LINE | CHARGING,
            rate: 13_500,
            ac_online: Some(true),
            ..raw()
        });
        assert_eq!(b.state, ChargeState::Charging);
        assert_eq!(b.seconds_left, Some(2 * 3600));
        let b = interpret(&RawBattery {
            power_state: POWER_ON_LINE,
            remaining: 54_000,
            rate: 0,
            ac_online: Some(true),
            ..raw()
        });
        assert_eq!(b.state, ChargeState::Full);
        assert_eq!(b.seconds_left, None);
        let b = interpret(&RawBattery {
            power_state: POWER_ON_LINE,
            remaining: 43_200,
            ac_online: Some(true),
            ..raw()
        });
        assert_eq!(b.state, ChargeState::Idle);
        let b = interpret(&RawBattery {
            design: UNKNOWN,
            full: UNKNOWN,
            remaining: UNKNOWN,
            rate: UNKNOWN_RATE,
            voltage: UNKNOWN,
            cycle_count: 0,
            ..raw()
        });
        assert_eq!(b.design_mwh, None);
        assert_eq!(b.percent, 50.0);
        assert_eq!(b.rate_mw, None);
        assert_eq!(b.voltage_mv, None);
        assert_eq!(b.cycle_count, None);
        // Relative-unit batteries: percentage only.
        let b = interpret(&RawBattery {
            capabilities: CAPACITY_RELATIVE,
            design: 100,
            full: 100,
            remaining: 40,
            ..raw()
        });
        assert_eq!(b.full_mwh, None);
        assert_eq!(b.percent, 50.0);
    }

    #[test]
    fn names_and_two_batteries() {
        assert_eq!(chemistry_name(b"LiP\0"), "Lithium polymer");
        assert_eq!(chemistry_name(b"NiMH"), "Nickel-metal hydride");
        assert_eq!(chemistry_name(b"XYZ\0"), "XYZ");
        let a = interpret(&raw());
        let b = interpret(&RawBattery {
            remaining: 13_000,
            full: 26_000,
            design: 30_000,
            rate: -3_000,
            ..raw()
        });
        let c = combine(a, &b);
        assert_eq!(c.count, 2);
        assert_eq!(c.full_mwh, Some(80_000));
        assert_eq!(c.remaining_mwh, Some(40_000));
        assert_eq!(c.percent, 50.0);
        assert_eq!(c.rate_mw, Some(-12_000));
        assert_eq!(c.seconds_left, Some(12_000));
    }

    #[test]
    fn demo_is_consistent() {
        let d = demo();
        let h = crate::health::health_percent(d.design_mwh, d.full_mwh).unwrap();
        assert!((h - 87.0).abs() < 0.1);
        assert!(
            (d.remaining_mwh.unwrap() as f32 / d.full_mwh.unwrap() as f32 * 100.0 - d.percent)
                .abs()
                < 0.5
        );
    }

    #[test]
    fn reading_this_pc_does_not_fail() {
        // Read-only. A desktop returns None; a laptop a plausible reading.
        if let Some(b) = read() {
            assert!((0.0..=100.0).contains(&b.percent));
            assert!(b.count >= 1);
        }
        let _ = ac_online();
    }
}
