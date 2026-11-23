//! Sleep and wake diagnostics: what blocks sleep (`powercfg /requests`),
//! the last wake source (`/lastwake`), wake timers (`/waketimers`), devices
//! allowed to wake the PC (`/devicequery wake_armed` and
//! `wake_programmable`), sleep states and hibernation. Every finding comes
//! with a plain explanation and what to do.
//!
//! The parsers are pure and tested against English powercfg output. The
//! runners use `powercfg.rs`, and `elevate.rs` for the commands that need
//! administrator rights (only when the user asks for that action).

use crate::batteryreport::StandbySummary;
use crate::events::PowerEvent;
use crate::powercfg;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RequestKind {
    Display,
    System,
    AwayMode,
    Execution,
    PerfBoost,
    ActiveLockScreen,
}

impl RequestKind {
    fn from_header(h: &str) -> Option<RequestKind> {
        Some(match h {
            "DISPLAY" => RequestKind::Display,
            "SYSTEM" => RequestKind::System,
            "AWAYMODE" => RequestKind::AwayMode,
            "EXECUTION" => RequestKind::Execution,
            "PERFBOOST" => RequestKind::PerfBoost,
            "ACTIVELOCKSCREEN" => RequestKind::ActiveLockScreen,
            _ => return None,
        })
    }

    /// "Keeps the screen on", ...
    pub fn label(self) -> &'static str {
        match self {
            RequestKind::Display => "Keeps the screen on",
            RequestKind::System => "Keeps the PC from sleeping",
            RequestKind::AwayMode => "Away mode (media)",
            RequestKind::Execution => "Keeps a task running",
            RequestKind::PerfBoost => "Performance boost",
            RequestKind::ActiveLockScreen => "Active lock screen",
        }
    }

    /// Whether this kind stops the PC from going to sleep on its own.
    pub fn blocks_sleep(self) -> bool {
        matches!(
            self,
            RequestKind::System | RequestKind::AwayMode | RequestKind::Display
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceKind {
    Process,
    Service,
    Driver,
    Other,
}

/// One active power request.
#[derive(Clone, Debug, PartialEq)]
pub struct PowerRequest {
    pub kind: RequestKind,
    pub source: SourceKind,
    /// Program, service or driver name (paths shortened to the file name,
    /// device instance paths removed).
    pub name: String,
    /// The reason the requester gave, if any.
    pub reason: String,
    /// Plain explanation plus what to do.
    pub plain: String,
}

/// Shortens a powercfg owner such as
/// `[PROCESS] \Device\HarddiskVolume3\...\svchost.exe (wuauserv)` or
/// `Realtek Audio (HDAUDIO\FUNC_01&VEN_10EC...)` to `svchost.exe (wuauserv)`
/// / `Realtek Audio`. Returns the source kind and the clean name.
pub fn clean_owner(text: &str) -> (SourceKind, String) {
    let mut t = text.trim();
    let mut kind = SourceKind::Other;
    for (tag, k) in [
        ("[PROCESS]", SourceKind::Process),
        ("[SERVICE]", SourceKind::Service),
        ("[DRIVER]", SourceKind::Driver),
    ] {
        if let Some(rest) = t.strip_prefix(tag) {
            kind = k;
            t = rest.trim();
        }
    }
    let mut name = t.to_string();
    // A trailing "(...)" that holds a device instance path is dropped.
    if let Some(open) = name.rfind('(') {
        let inner = &name[open..];
        if inner.contains('\\') || inner.contains('&') {
            name = name[..open].trim_end().to_string();
        }
    }
    // A path becomes its file name; keep a trailing "(service)" part.
    let (path, suffix) = match name.rfind(" (") {
        Some(i) if name.ends_with(')') => (name[..i].to_string(), name[i..].to_string()),
        _ => (name.clone(), String::new()),
    };
    if path.contains('\\') {
        let file = path.rsplit('\\').next().unwrap_or(&path).to_string();
        name = format!("{file}{suffix}");
    }
    (kind, name.trim().to_string())
}

/// Plain explanation for a request.
fn explain_request(kind: RequestKind, name: &str, reason: &str) -> String {
    let n = name.to_lowercase();
    let r = reason.to_lowercase();
    let app = name.split(" (").next().unwrap_or(name);
    let screen_or_sleep = matches!(kind, RequestKind::Display | RequestKind::System);
    if screen_or_sleep
        && (n.contains("chrome")
            || n.contains("msedge")
            || n.contains("firefox")
            || n.contains("brave")
            || n.contains("opera"))
        && (r.contains("video")
            || r.contains("audio")
            || r.contains("playing")
            || r.contains("wake lock"))
    {
        return format!(
            "A browser tab ({app}) is playing video or audio, so Windows won't sleep or turn off the screen on its own. Pause or close that tab to let the PC sleep."
        );
    }
    if r.contains("audio stream") || n.contains("audio") && kind == RequestKind::System {
        return "Sound is playing (or an app holds the audio device open), which keeps the PC awake. Stop the playback, or close the app that uses the speakers or microphone.".into();
    }
    if n.contains("wuauserv")
        || n.contains("usoclient")
        || n.contains("musnotification")
        || r.contains("windows update")
    {
        return "Windows Update is downloading or installing updates. Let it finish; it stops holding the PC awake when it is done.".into();
    }
    if n.contains("termservice") || r.contains("remote") {
        return "A remote desktop session is connected. Windows stays awake while someone is connected remotely.".into();
    }
    if n.contains("spotify") || n.contains("vlc") || n.contains("wmplayer") || n.contains("music") {
        return format!("{app} is playing media. Pause it to let the PC sleep.");
    }
    if n.contains("teams") || n.contains("zoom") || n.contains("skype") || r.contains("call") {
        return format!("{app} is in a call or meeting, which keeps the PC awake until it ends.");
    }
    if n.contains("legacy kernel caller") {
        return "A driver asked Windows to stay awake (often a media or network driver). If it never goes away, update or restart that device's driver.".into();
    }
    if n.contains("powerforge") {
        return "This is PowerForge's own keep-awake. Turn it off on the Keep awake page.".into();
    }
    let who = match kind {
        RequestKind::Display => format!("{app} asked Windows to keep the screen on"),
        RequestKind::System => format!("{app} asked Windows not to sleep"),
        RequestKind::AwayMode => format!("{app} uses away mode: the PC looks asleep but keeps working (usually for recording or streaming media)"),
        RequestKind::Execution => format!("{app} asked to keep running for a while, even if the PC is about to sleep"),
        RequestKind::PerfBoost => format!("{app} asked for extra processor speed"),
        RequestKind::ActiveLockScreen => format!("{app} keeps the lock screen active"),
    };
    let what = if reason.is_empty() {
        String::new()
    } else {
        format!(" It says: \u{201c}{reason}\u{201d}.")
    };
    let todo = match kind {
        RequestKind::Display | RequestKind::System | RequestKind::AwayMode => {
            " If you don't need it, close or pause that program."
        }
        _ => " This usually ends by itself.",
    };
    format!("{who}.{what}{todo}")
}

pub fn parse_requests(text: &str) -> Vec<PowerRequest> {
    let mut out = Vec::new();
    let mut kind: Option<RequestKind> = None;
    let mut current: Option<(SourceKind, String, Vec<String>)> = None;
    let flush = |kind: Option<RequestKind>,
                 current: &mut Option<(SourceKind, String, Vec<String>)>,
                 out: &mut Vec<PowerRequest>| {
        if let (Some(k), Some((source, name, reason))) = (kind, current.take()) {
            let reason = reason.join(" ").trim().to_string();
            out.push(PowerRequest {
                kind: k,
                source,
                plain: explain_request(k, &name, &reason),
                name,
                reason,
            });
        }
    };
    for line in text.lines() {
        let l = line.trim();
        if let Some(h) = l.strip_suffix(':') {
            if let Some(k) = RequestKind::from_header(h.trim()) {
                flush(kind, &mut current, &mut out);
                kind = Some(k);
                continue;
            }
        }
        if l.is_empty() {
            flush(kind, &mut current, &mut out);
            continue;
        }
        if l.eq_ignore_ascii_case("none.") || kind.is_none() {
            continue;
        }
        if l.starts_with('[') {
            flush(kind, &mut current, &mut out);
            let (source, name) = clean_owner(l);
            current = Some((source, name, Vec::new()));
        } else if let Some((_, _, reason)) = current.as_mut() {
            reason.push(l.to_string());
        }
    }
    flush(kind, &mut current, &mut out);
    out
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct WakeSource {
    /// "Device", "Timer", "Power button", "Fixed feature", "Unknown".
    pub kind: String,
    /// Friendly device or timer owner name (no instance paths).
    pub name: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct LastWake {
    pub sources: Vec<WakeSource>,
    /// Plain sentence.
    pub plain: String,
}

pub fn parse_lastwake(text: &str) -> LastWake {
    let mut sources: Vec<WakeSource> = Vec::new();
    let mut in_source = false;
    for line in text.lines() {
        let l = line.trim();
        if l.is_empty() {
            continue;
        }
        if l.starts_with("Wake Source [") {
            sources.push(WakeSource::default());
            in_source = true;
            continue;
        }
        if l.starts_with("Wake History") || l.starts_with("Wake Source Count") {
            in_source = l.starts_with("Wake Source Count") && in_source;
            continue;
        }
        if !in_source {
            continue;
        }
        let Some(src) = sources.last_mut() else {
            continue;
        };
        match l.split_once(':') {
            Some((key, value)) => {
                let (key, value) = (key.trim(), value.trim());
                match key {
                    "Type" => src.kind = value.to_string(),
                    "Friendly Name" if !value.is_empty() => src.name = value.to_string(),
                    "Description" if src.name.is_empty() && !value.is_empty() => {
                        src.name = value.to_string()
                    }
                    "Owner" if src.name.is_empty() => src.name = clean_owner(value).1,
                    "Instance Path" | "Manufacturer" => {}
                    _ => {
                        if src.name.is_empty() && !key.contains("Path") && !value.contains('\\') {
                            src.name = l.to_string();
                        }
                    }
                }
            }
            None => {
                if src.name.is_empty() {
                    src.name = l.to_string();
                }
            }
        }
    }
    let plain = match sources.first() {
        None => "Windows did not record what woke the PC last time. That is common after waking it with the keyboard, the lid or the power button on Modern Standby PCs.".to_string(),
        Some(s) => {
            let name = if s.name.is_empty() { "an unknown source".to_string() } else { s.name.clone() };
            let kind = s.kind.to_lowercase();
            let lname = name.to_lowercase();
            if lname.contains("power button") {
                "The PC was woken by its power button.".to_string()
            } else if lname.contains("lid") {
                "The PC was woken by opening the lid.".to_string()
            } else if kind.contains("timer") {
                format!("The PC was woken by a wake timer set by {name}. If you didn't want that, turn off \u{201c}Let scheduled tasks wake the PC\u{201d} on the Power plans page.")
            } else if kind.contains("device") {
                format!("The PC was woken by a device: {name}. If that was unexpected, stop this device from waking the PC in the list below.")
            } else {
                format!("The PC was woken by: {name}.")
            }
        }
    };
    LastWake { sources, plain }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct WakeTimer {
    /// Who set it (program, service or task name).
    pub owner: String,
    /// Expiry as printed, and parsed when possible.
    pub expires_text: String,
    pub expires: Option<i64>,
    pub reason: String,
    pub plain: String,
}

/// "03:12:45 on 25/11/2026" (local time) -> Unix time. Day/month order is
/// guessed when one part is above 12, otherwise day first.
fn parse_timer_time(text: &str) -> Option<i64> {
    let (time, date) = text.split_once(" on ")?;
    let mut tp = time.trim().split(':').map(|p| p.trim().parse::<i64>().ok());
    let h = tp.next()??;
    let m = tp.next()??;
    let s = tp.next().flatten().unwrap_or(0);
    let parts: Vec<i64> = date
        .trim()
        .trim_end_matches('.')
        .split(['/', '.', '-'])
        .filter_map(|p| p.trim().parse().ok())
        .collect();
    if parts.len() != 3 {
        return None;
    }
    let (y, mo, d) = if parts[0] > 31 {
        (parts[0], parts[1], parts[2])
    } else if parts[1] > 12 {
        (parts[2], parts[0], parts[1])
    } else {
        (parts[2], parts[1], parts[0])
    };
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || m > 59 {
        return None;
    }
    let local =
        crate::util::days_from_civil(y, mo as u32, d as u32) * 86_400 + h * 3600 + m * 60 + s;
    Some(local - crate::util::local_offset(local))
}

pub fn parse_waketimers(text: &str) -> Vec<WakeTimer> {
    let mut out: Vec<WakeTimer> = Vec::new();
    for line in text.lines() {
        let l = line.trim();
        if let Some(rest) = l.strip_prefix("Timer set by ") {
            let (owner, expires) = match rest.split_once(" expires at ") {
                Some((o, e)) => (o, e.trim().trim_end_matches('.').to_string()),
                None => (rest, String::new()),
            };
            out.push(WakeTimer {
                owner: clean_owner(owner).1,
                expires: parse_timer_time(&expires),
                expires_text: expires,
                ..WakeTimer::default()
            });
        } else if let Some(reason) = l.strip_prefix("Reason:") {
            if let Some(t) = out.last_mut() {
                t.reason = reason.trim().to_string();
            }
        }
    }
    for t in &mut out {
        let when = match t.expires {
            Some(at) => crate::util::fmt_datetime(at),
            None if !t.expires_text.is_empty() => t.expires_text.clone(),
            None => "a set time".into(),
        };
        let task = t
            .reason
            .split('\'')
            .nth(1)
            .map(|path| path.rsplit('\\').next().unwrap_or(path).to_string());
        t.plain = match task {
            Some(task) => format!(
                "The scheduled task \u{201c}{task}\u{201d} will wake the PC at {when}. To stop it, turn off \u{201c}Let scheduled tasks wake the PC\u{201d} on the Power plans page, or untick \u{201c}Wake the computer to run this task\u{201d} for that task in Task Scheduler."
            ),
            None => format!(
                "{} will wake the PC at {when}. To stop wake timers, turn off \u{201c}Let scheduled tasks wake the PC\u{201d} on the Power plans page.",
                t.owner
            ),
        };
    }
    out
}

/// Device names, one per line; "NONE" means none.
pub fn parse_devicequery(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in text.lines() {
        let l = line.trim();
        if l.is_empty() || l.eq_ignore_ascii_case("none") || powercfg::needs_admin(l) {
            continue;
        }
        if !out.iter().any(|d: &String| d == l) {
            out.push(l.to_string());
        }
    }
    out
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct WakeDevice {
    pub name: String,
    /// Currently allowed to wake the PC.
    pub armed: bool,
    /// "Network adapter", "Keyboard", "Mouse", "USB controller", "Bluetooth", "Other".
    pub category: String,
}

/// Guesses what kind of device a name is.
pub fn device_category(name: &str) -> &'static str {
    let n = name.to_lowercase();
    if n.contains("keyboard") {
        "Keyboard"
    } else if n.contains("mouse") || n.contains("touchpad") || n.contains("pointing") {
        "Mouse or touchpad"
    } else if n.contains("bluetooth") {
        "Bluetooth"
    } else if [
        "ethernet", "wi-fi", "wifi", "wireless", "network", "nic", "lan", "wlan", "802.11",
    ]
    .iter()
    .any(|k| n.contains(k))
    {
        "Network adapter"
    } else if n.contains("usb") || n.contains("xhci") || n.contains("hub") {
        "USB controller"
    } else {
        "Other"
    }
}

/// What a device category means for waking.
pub fn device_advice(category: &str) -> &'static str {
    match category {
        "Keyboard" | "Mouse or touchpad" => "Lets you wake the PC by pressing a key or moving the mouse. Usually wanted.",
        "Network adapter" => "Network traffic can wake the PC. A common cause of unexpected wake-ups; turn it off unless you use Wake-on-LAN.",
        "Bluetooth" => "Bluetooth devices can wake the PC. Turn it off if the PC wakes up by itself.",
        "USB controller" => "Any device on this USB controller can wake the PC.",
        _ => "This device is allowed to wake the PC.",
    }
}

/// Sleep states and hibernation.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SleepCaps {
    pub modern_standby: bool,
    pub s3: bool,
    pub hibernate_supported: bool,
    pub hibernate_enabled: bool,
    pub hiberfile_bytes: Option<u64>,
    pub fast_startup: bool,
    pub hybrid_sleep: bool,
    pub lid_present: bool,
    /// Plain sentences describing how this PC sleeps.
    pub notes: Vec<String>,
}

/// The plain notes for a set of capabilities.
pub fn caps_notes(c: &SleepCaps) -> Vec<String> {
    let mut notes = Vec::new();
    if c.modern_standby {
        notes.push("This PC uses Modern Standby: while asleep it stays partly on, like a phone, so it can wake instantly and fetch mail. It uses a little battery while asleep.".to_string());
    } else if c.s3 {
        notes.push("This PC uses classic sleep (S3): almost everything turns off while asleep, and it wakes in a few seconds.".to_string());
    } else {
        notes.push("Windows reports no sleep state on this PC. Sleep may be turned off in the firmware or by a driver.".to_string());
    }
    if c.hibernate_enabled {
        let size = c
            .hiberfile_bytes
            .map(|b| {
                format!(
                    " The hibernation file takes {} of disk space.",
                    crate::util::fmt_bytes(b)
                )
            })
            .unwrap_or_default();
        notes.push(format!("Hibernation is on: the PC can save its state to disk and turn off completely, using no power.{size}"));
    } else if c.hibernate_supported {
        notes.push("Hibernation is off. Turning it on lets the PC save everything to disk and use no power at all during long breaks.".to_string());
    }
    if c.fast_startup {
        notes.push("Fast startup is on: shutting down is a partial hibernation, so the PC starts faster. Restart (not shut down) when an update or driver asks for it.".to_string());
    }
    notes
}

/// Reads sleep capabilities (GetPwrCapabilities, hiberfil.sys size via its
/// directory entry, fast startup from the registry). No admin needed.
pub fn capabilities() -> SleepCaps {
    use windows::Win32::System::Power::{GetPwrCapabilities, SYSTEM_POWER_CAPABILITIES};
    let mut caps = SYSTEM_POWER_CAPABILITIES::default();
    // SAFETY: plain out-parameter call.
    let ok = unsafe { GetPwrCapabilities(&mut caps) }.as_bool();
    let mut c = SleepCaps::default();
    if ok {
        c.modern_standby = caps.AoAc.as_bool();
        c.s3 = caps.SystemS3.as_bool();
        c.hibernate_supported = caps.SystemS4.as_bool() || caps.HiberFilePresent.as_bool();
        c.hibernate_enabled = caps.HiberFilePresent.as_bool();
        c.lid_present = caps.LidPresent.as_bool();
    }
    c.hiberfile_bytes = hiberfile_size();
    if c.hiberfile_bytes.is_some() && !ok {
        c.hibernate_enabled = true;
    }
    let hiberboot = winreg::RegKey::predef(winreg::enums::HKEY_LOCAL_MACHINE)
        .open_subkey("SYSTEM\\CurrentControlSet\\Control\\Session Manager\\Power")
        .and_then(|k| k.get_value::<u32, _>("HiberbootEnabled"))
        .unwrap_or(0);
    c.fast_startup = hiberboot == 1 && c.hibernate_enabled;
    c.hybrid_sleep = c.s3 && c.hibernate_enabled;
    c.notes = caps_notes(&c);
    c
}

/// Size of hiberfil.sys from its directory entry (the file itself is never opened).
fn hiberfile_size() -> Option<u64> {
    use windows::core::PCWSTR;
    use windows::Win32::Storage::FileSystem::{FindClose, FindFirstFileW, WIN32_FIND_DATAW};
    let drive = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into());
    let path = crate::sys::wide(&format!("{drive}\\hiberfil.sys"));
    let mut data = WIN32_FIND_DATAW::default();
    // SAFETY: a NUL-terminated path and an out-structure; the handle is closed.
    unsafe {
        let h = FindFirstFileW(PCWSTR(path.as_ptr()), &mut data).ok()?;
        let _ = FindClose(h);
    }
    let size = ((data.nFileSizeHigh as u64) << 32) | data.nFileSizeLow as u64;
    (size > 0).then_some(size)
}

#[derive(Clone, Debug, PartialEq)]
pub enum DiagError {
    /// The command needs administrator rights and was not elevated.
    NeedsAdmin,
    /// The user declined the UAC prompt.
    Declined,
    Failed(String),
}

impl std::fmt::Display for DiagError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DiagError::NeedsAdmin => write!(f, "this check needs administrator rights"),
            DiagError::Declined => write!(f, "the administrator prompt was declined"),
            DiagError::Failed(e) => write!(f, "{e}"),
        }
    }
}

/// Runs a query unelevated; with `elevate` retries as administrator when needed.
fn query(args: &[&str], elevate: bool) -> Result<String, DiagError> {
    if crate::elevate::is_elevated() || !elevate {
        let out = powercfg::run(args, 60).map_err(DiagError::Failed)?;
        if powercfg::needs_admin(&out.text) {
            if !elevate {
                return Err(DiagError::NeedsAdmin);
            }
        } else {
            return Ok(out.text);
        }
    }
    let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    let text = crate::elevate::powercfg(&args, 120)?;
    if powercfg::needs_admin(&text) {
        return Err(DiagError::NeedsAdmin);
    }
    Ok(text)
}

/// Runs `/requests`; with `elevate` it asks for administrator rights for this
/// one command (UAC prompt) unless PowerForge already runs elevated.
pub fn query_requests(elevate: bool) -> Result<Vec<PowerRequest>, DiagError> {
    query(&["/requests"], elevate).map(|t| parse_requests(&t))
}

/// `/lastwake` (no admin needed).
pub fn query_lastwake() -> Result<LastWake, DiagError> {
    query(&["/lastwake"], false).map(|t| parse_lastwake(&t))
}

/// `/waketimers` (admin on recent Windows).
pub fn query_waketimers(elevate: bool) -> Result<Vec<WakeTimer>, DiagError> {
    query(&["/waketimers"], elevate).map(|t| parse_waketimers(&t))
}

/// Devices that can wake the PC, with whether each is armed (no admin needed).
pub fn query_wake_devices() -> Result<Vec<WakeDevice>, DiagError> {
    let armed = parse_devicequery(&query(&["/devicequery", "wake_armed"], false)?);
    let programmable = query(&["/devicequery", "wake_programmable"], false)
        .map(|t| parse_devicequery(&t))
        .unwrap_or_default();
    Ok(merge_devices(&programmable, &armed))
}

/// Armed devices first, then the rest; each with a category.
pub fn merge_devices(programmable: &[String], armed: &[String]) -> Vec<WakeDevice> {
    let mut out: Vec<WakeDevice> = armed
        .iter()
        .map(|n| WakeDevice {
            name: n.clone(),
            armed: true,
            category: device_category(n).into(),
        })
        .collect();
    for n in programmable {
        if !armed.contains(n) {
            out.push(WakeDevice {
                name: n.clone(),
                armed: false,
                category: device_category(n).into(),
            });
        }
    }
    out
}

/// `/deviceenablewake` or `/devicedisablewake` (admin, elevated per action).
pub fn set_device_wake(name: &str, enable: bool) -> Result<(), DiagError> {
    let verb = if enable {
        "/deviceenablewake"
    } else {
        "/devicedisablewake"
    };
    let out = query(&[verb, name], true)?;
    if out.to_lowercase().contains("not") && out.to_lowercase().contains("found") {
        return Err(DiagError::Failed(out.trim().to_string()));
    }
    Ok(())
}

/// `powercfg /hibernate on|off` (admin, elevated per action).
pub fn set_hibernate(on: bool) -> Result<(), DiagError> {
    query(&["/hibernate", if on { "on" } else { "off" }], true).map(|_| ())
}

/// Everything the Sleep & wake page shows. `None` = not checked yet.
#[derive(Clone, Debug, Default)]
pub struct DiagSnapshot {
    pub requests: Option<Vec<PowerRequest>>,
    pub last_wake: Option<LastWake>,
    pub wake_timers: Option<Vec<WakeTimer>>,
    pub wake_devices: Option<Vec<WakeDevice>>,
    pub caps: Option<SleepCaps>,
    pub events: Option<Vec<PowerEvent>>,
    pub standby: Option<StandbySummary>,
}

/// Invented diagnostics for `--demo`.
pub fn demo(now: i64) -> DiagSnapshot {
    let requests = parse_requests(DEMO_REQUESTS);
    let last_wake = parse_lastwake(DEMO_LASTWAKE);
    let wake_timers = parse_waketimers(&format!(
        "Timer set by [SERVICE] \\Device\\HarddiskVolume3\\Windows\\System32\\svchost.exe (SystemEventsBroker) expires at {} on {}.\n  Reason: Windows will execute 'NT TASK\\Microsoft\\Windows\\UpdateOrchestrator\\Scheduled Scan' scheduled task that requested waking the computer.\n",
        {
            let t = crate::util::next_local_time(now, 3, 12);
            let (_, _, _, h, m, s) = crate::util::local_parts(t);
            format!("{h:02}:{m:02}:{s:02}")
        },
        {
            let t = crate::util::next_local_time(now, 3, 12);
            let (y, mo, d, ..) = crate::util::local_parts(t);
            format!("{d:02}/{mo:02}/{y}")
        }
    ));
    let devices = merge_devices(
        &[
            "Northwind Wi-Fi 6E Adapter".into(),
            "HID Keyboard Device".into(),
            "HID-compliant mouse".into(),
            "USB Root Hub (USB 3.0)".into(),
            "Northwind Bluetooth Radio".into(),
        ],
        &[
            "Northwind Wi-Fi 6E Adapter".into(),
            "HID Keyboard Device".into(),
            "HID-compliant mouse".into(),
        ],
    );
    let caps = SleepCaps {
        modern_standby: true,
        s3: false,
        hibernate_supported: true,
        hibernate_enabled: true,
        hiberfile_bytes: Some(6_442_450_944),
        fast_startup: true,
        hybrid_sleep: false,
        lid_present: true,
        notes: Vec::new(),
    };
    let caps = SleepCaps {
        notes: caps_notes(&caps),
        ..caps
    };
    DiagSnapshot {
        requests: Some(requests),
        last_wake: Some(last_wake),
        wake_timers: Some(wake_timers),
        wake_devices: Some(devices),
        caps: Some(caps),
        events: Some(crate::events::demo(now)),
        standby: crate::batteryreport::standby_summary(&crate::batteryreport::demo()),
    }
}

const DEMO_REQUESTS: &str = "DISPLAY:\n[PROCESS] \\Device\\HarddiskVolume3\\Program Files\\Browser\\Application\\msedge.exe\nVideo Wake Lock\n\nSYSTEM:\n[DRIVER] Northwind High Definition Audio (HDAUDIO\\FUNC_01&VEN_0000&DEV_0000\\0&00000000&0&0001)\nAn audio stream is currently in use.\n\nAWAYMODE:\nNone.\n\nEXECUTION:\n[PROCESS] \\Device\\HarddiskVolume3\\Program Files\\Browser\\Application\\msedge.exe\nPlaying audio\n\nPERFBOOST:\nNone.\n\nACTIVELOCKSCREEN:\nNone.\n";

const DEMO_LASTWAKE: &str = "Wake History Count - 1\nWake History [0]\n  Wake Source Count - 1\n  Wake Source [0]\n    Type: Device\n    Instance Path: PCI\\VEN_0000&DEV_0000\\0&0000\n    Friendly Name: Northwind Wi-Fi 6E Adapter\n    Description: Wireless network adapter\n    Manufacturer: Northwind\n";

#[cfg(test)]
mod tests {
    use super::*;

    const REQUESTS: &str = include_str!("../tests/fixtures/requests-busy.txt");
    const REQUESTS_NONE: &str = "DISPLAY:\nNone.\n\nSYSTEM:\nNone.\n\nAWAYMODE:\nNone.\n\nEXECUTION:\nNone.\n\nPERFBOOST:\nNone.\n\nACTIVELOCKSCREEN:\nNone.\n\n";

    #[test]
    fn requests() {
        assert!(parse_requests(REQUESTS_NONE).is_empty());
        let r = parse_requests(REQUESTS);
        assert_eq!(r.len(), 5, "{r:#?}");
        assert_eq!(r[0].kind, RequestKind::Display);
        assert_eq!(r[0].source, SourceKind::Process);
        assert_eq!(r[0].name, "chrome.exe");
        assert_eq!(r[0].reason, "Video Wake Lock");
        assert!(r[0].plain.contains("browser tab"));
        assert_eq!(r[1].kind, RequestKind::System);
        assert_eq!(r[1].source, SourceKind::Driver);
        assert_eq!(r[1].name, "Example High Definition Audio");
        assert!(!r[1].name.contains("HDAUDIO"));
        assert!(r[1].plain.contains("Sound is playing"));
        assert_eq!(r[2].kind, RequestKind::System);
        assert_eq!(r[2].source, SourceKind::Service);
        assert_eq!(r[2].name, "svchost.exe (TermService)");
        assert!(r[2].plain.contains("remote"));
        assert_eq!(r[3].kind, RequestKind::AwayMode);
        assert_eq!(r[3].name, "recorder.exe");
        assert_eq!(r[3].reason, "Recording a show, part two");
        assert!(r[3].plain.contains("away mode"));
        assert_eq!(r[4].kind, RequestKind::Execution);
        assert_eq!(r[4].name, "svchost.exe (wuauserv)");
        assert!(r[4].plain.contains("Windows Update"));
    }

    #[test]
    fn owners() {
        assert_eq!(
            clean_owner("[PROCESS] \\Device\\HarddiskVolume3\\Windows\\System32\\svchost.exe (SystemEventsBroker)"),
            (SourceKind::Process, "svchost.exe (SystemEventsBroker)".into())
        );
        assert_eq!(
            clean_owner("[DRIVER] Legacy Kernel Caller"),
            (SourceKind::Driver, "Legacy Kernel Caller".into())
        );
        assert_eq!(clean_owner("plain").1, "plain");
    }

    #[test]
    fn lastwake() {
        let none =
            parse_lastwake("Wake History Count - 1\nWake History [0]\n  Wake Source Count - 0\n");
        assert!(none.sources.is_empty());
        assert!(none.plain.contains("did not record"));
        let dev = parse_lastwake(include_str!("../tests/fixtures/lastwake-device.txt"));
        assert_eq!(dev.sources.len(), 1);
        assert_eq!(dev.sources[0].kind, "Device");
        assert_eq!(
            dev.sources[0].name,
            "Example USB 3.1 eXtensible Host Controller"
        );
        assert!(dev.plain.contains("woken by a device"));
        assert!(!format!("{dev:?}").contains("VEN_"));
        let button = parse_lastwake(include_str!("../tests/fixtures/lastwake-button.txt"));
        assert_eq!(button.sources[0].kind, "Fixed Feature");
        assert!(button.plain.contains("power button"), "{button:?}");
        let timer = parse_lastwake(include_str!("../tests/fixtures/lastwake-timer.txt"));
        assert_eq!(timer.sources[0].kind, "Wake Timer");
        assert_eq!(timer.sources[0].name, "svchost.exe (SystemEventsBroker)");
        assert!(timer.plain.contains("wake timer"));
    }

    #[test]
    fn waketimers() {
        assert!(parse_waketimers("There are no active wake timers in the system.\n").is_empty());
        crate::util::set_fixed_offset(Some(0));
        let t = parse_waketimers(include_str!("../tests/fixtures/waketimers-one.txt"));
        assert_eq!(t.len(), 2);
        assert_eq!(t[0].owner, "svchost.exe (SystemEventsBroker)");
        assert_eq!(t[0].expires_text, "03:12:45 on 25/11/2026");
        assert_eq!(
            t[0].expires,
            crate::util::parse_iso_datetime("2026-11-25T03:12:45Z")
        );
        assert!(t[0].reason.starts_with("Windows will execute"));
        assert!(t[0].plain.contains("Universal Orchestrator Start"));
        // Month-first date with a day above 12.
        assert_eq!(
            t[1].expires,
            crate::util::parse_iso_datetime("2026-12-24T07:00:00Z")
        );
        assert!(t[1].plain.contains("backup.exe"));
        crate::util::set_fixed_offset(None);
        assert_eq!(parse_timer_time("garbage"), None);
    }

    #[test]
    fn devicequery_and_categories() {
        assert!(parse_devicequery("NONE\n\n").is_empty());
        let d = parse_devicequery(include_str!("../tests/fixtures/devicequery-armed.txt"));
        assert_eq!(
            d,
            vec![
                "Example Wi-Fi 7 Adapter",
                "HID Keyboard Device",
                "HID-compliant mouse"
            ]
        );
        assert_eq!(
            device_category("Example Wi-Fi 7 Adapter"),
            "Network adapter"
        );
        assert_eq!(
            device_category("Intel(R) Ethernet Connection"),
            "Network adapter"
        );
        assert_eq!(device_category("HID Keyboard Device"), "Keyboard");
        assert_eq!(device_category("HID-compliant mouse"), "Mouse or touchpad");
        assert_eq!(device_category("Generic Bluetooth Radio"), "Bluetooth");
        assert_eq!(device_category("USB Root Hub (USB 3.0)"), "USB controller");
        assert_eq!(device_category("Something"), "Other");
        let merged = merge_devices(&["A".into(), "B".into()], &["B".into()]);
        assert_eq!(merged.len(), 2);
        assert!(merged[0].armed && merged[0].name == "B");
        assert!(!merged[1].armed);
        assert!(!device_advice("Network adapter").is_empty());
    }

    #[test]
    fn caps_and_notes() {
        let c = SleepCaps {
            modern_standby: true,
            hibernate_supported: true,
            hibernate_enabled: true,
            hiberfile_bytes: Some(6 * 1024 * 1024 * 1024),
            fast_startup: true,
            ..SleepCaps::default()
        };
        let n = caps_notes(&c);
        assert_eq!(n.len(), 3);
        assert!(n[0].contains("Modern Standby"));
        assert!(n[1].contains("6.0 GB"));
        // The real capabilities are readable without admin.
        let real = capabilities();
        assert!(!real.notes.is_empty());
    }

    #[test]
    fn demo_snapshot() {
        let d = demo(1_790_000_000);
        assert_eq!(d.requests.as_ref().map(|r| r.len()), Some(3));
        assert_eq!(d.wake_timers.as_ref().map(|r| r.len()), Some(1));
        assert!(d.wake_devices.unwrap().iter().filter(|w| w.armed).count() == 3);
    }

    #[test]
    fn real_lastwake_is_readable() {
        // Read-only, no admin needed.
        let w = query_lastwake().unwrap();
        assert!(!w.plain.is_empty());
    }
}
