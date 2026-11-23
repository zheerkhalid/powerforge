//! Sleep and wake history from the System event log (Kernel-Power,
//! Power-Troubleshooter, Kernel-General), read with `wevtutil` (no admin
//! needed). The computer name and user SIDs in the events are never kept.

use crate::util;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventKind {
    /// Entered sleep (Kernel-Power 42).
    Sleep,
    /// Woke up (Power-Troubleshooter 1; carries source and sleep length).
    Wake,
    /// Entered / left Modern Standby (Kernel-Power 506 / 507).
    StandbyEnter,
    StandbyExit,
    /// Started (Kernel-General 12) / shut down (Kernel-General 13).
    Boot,
    Shutdown,
    /// Restarted without a clean shutdown (Kernel-Power 41).
    Unexpected,
}

impl EventKind {
    pub fn label(self) -> &'static str {
        match self {
            EventKind::Sleep => "Went to sleep",
            EventKind::Wake => "Woke up",
            EventKind::StandbyEnter => "Screen off (standby)",
            EventKind::StandbyExit => "Left standby",
            EventKind::Boot => "Started",
            EventKind::Shutdown => "Shut down",
            EventKind::Unexpected => "Unexpected restart",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct PowerEvent {
    pub t: i64,
    pub kind: EventKind,
    /// Plain sentence.
    pub text: String,
    pub wake_source: Option<String>,
    /// For wakes: how long it slept, when recorded.
    pub slept_secs: Option<u64>,
}

/// Kernel-Power 42 sleep reasons.
fn sleep_reason(code: &str) -> &'static str {
    match code {
        "0" => "a button or the lid",
        "1" | "2" => "an app or the Start menu",
        "4" => "the battery running low",
        "6" | "7" => "the PC being idle",
        "8" => "the battery running critically low",
        _ => "",
    }
}

/// Kernel-Power 506/507 reasons (a small, common subset).
fn standby_reason(code: &str) -> &'static str {
    match code {
        "1" | "31" => "the power button",
        "12" | "15" => "the lid",
        "6" | "32" => "the PC being idle",
        "44" | "45" => "input (keyboard, mouse or touch)",
        _ => "",
    }
}

fn target_state(code: &str) -> &'static str {
    match code {
        // SYSTEM_POWER_STATE: 5 = S4 (hibernate); 2..=4 are sleep states.
        "5" => "hibernation",
        _ => "sleep",
    }
}

/// Parses `wevtutil qe System /f:xml` output (a sequence of `<Event>`
/// elements without a root), newest first.
pub fn parse(xml: &str) -> Vec<PowerEvent> {
    let body = xml.trim_start_matches('\u{feff}');
    let wrapped = format!("<Events>{body}</Events>");
    let Ok(doc) = roxmltree::Document::parse(&wrapped) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for ev in doc
        .root_element()
        .children()
        .filter(|n| n.has_tag_name_local("Event"))
    {
        let Some(system) = ev.children().find(|n| n.has_tag_name_local("System")) else {
            continue;
        };
        let child = |name: &str| system.children().find(|n| n.has_tag_name_local(name));
        let provider = child("Provider")
            .and_then(|p| p.attribute("Name"))
            .unwrap_or("");
        let id: u32 = child("EventID")
            .and_then(|n| n.text())
            .and_then(|t| t.trim().parse().ok())
            .unwrap_or(0);
        let Some(t) = child("TimeCreated")
            .and_then(|n| n.attribute("SystemTime"))
            .and_then(util::parse_iso_datetime)
        else {
            continue;
        };
        let data: Vec<(String, String)> = ev
            .children()
            .find(|n| n.has_tag_name_local("EventData"))
            .map(|d| {
                d.children()
                    .filter(|n| n.has_tag_name_local("Data"))
                    .map(|n| {
                        (
                            n.attribute("Name").unwrap_or("").to_string(),
                            n.text().unwrap_or("").trim().to_string(),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        let get = |name: &str| {
            data.iter()
                .find(|(k, _)| k == name)
                .map(|(_, v)| v.as_str())
                .unwrap_or("")
        };
        let event = match (provider, id) {
            ("Microsoft-Windows-Kernel-Power", 42) => {
                let into = target_state(get("TargetState"));
                let why = sleep_reason(get("Reason"));
                let text = if why.is_empty() {
                    format!("Went into {into}.")
                } else {
                    format!("Went into {into}, started by {why}.")
                };
                PowerEvent {
                    t,
                    kind: EventKind::Sleep,
                    text,
                    wake_source: None,
                    slept_secs: None,
                }
            }
            ("Microsoft-Windows-Power-Troubleshooter", 1) => {
                let slept = match (
                    util::parse_iso_datetime(get("SleepTime")),
                    util::parse_iso_datetime(get("WakeTime")),
                ) {
                    (Some(a), Some(b)) if b > a => Some((b - a) as u64),
                    _ => None,
                };
                let from = target_state(get("TargetState"));
                let source = get("WakeSourceText");
                let source = (!source.is_empty()).then(|| source.to_string());
                let mut text = match slept {
                    Some(s) => format!("Woke from {from} after {}.", util::fmt_duration(s)),
                    None => format!("Woke from {from}."),
                };
                match &source {
                    Some(s) => text.push_str(&format!(" Woken by: {s}.")),
                    None => text.push_str(" Windows did not record what woke it."),
                }
                PowerEvent {
                    t,
                    kind: EventKind::Wake,
                    text,
                    wake_source: source,
                    slept_secs: slept,
                }
            }
            ("Microsoft-Windows-Kernel-Power", 506) => {
                let why = standby_reason(get("Reason"));
                PowerEvent {
                    t,
                    kind: EventKind::StandbyEnter,
                    text: if why.is_empty() {
                        "Entered Modern Standby (screen off, mostly asleep).".into()
                    } else {
                        format!("Entered Modern Standby, started by {why}.")
                    },
                    wake_source: None,
                    slept_secs: None,
                }
            }
            ("Microsoft-Windows-Kernel-Power", 507) => {
                let why = standby_reason(get("Reason"));
                PowerEvent {
                    t,
                    kind: EventKind::StandbyExit,
                    text: if why.is_empty() {
                        "Left Modern Standby.".into()
                    } else {
                        format!("Left Modern Standby because of {why}.")
                    },
                    wake_source: (!why.is_empty()).then(|| why.to_string()),
                    slept_secs: None,
                }
            }
            ("Microsoft-Windows-Kernel-Power", 41) => PowerEvent {
                t,
                kind: EventKind::Unexpected,
                text: "Restarted without shutting down cleanly (a crash, a forced power-off or a power cut).".into(),
                wake_source: None,
                slept_secs: None,
            },
            ("Microsoft-Windows-Kernel-General", 12) => PowerEvent {
                t,
                kind: EventKind::Boot,
                text: "Windows started.".into(),
                wake_source: None,
                slept_secs: None,
            },
            ("Microsoft-Windows-Kernel-General", 13) => PowerEvent {
                t,
                kind: EventKind::Shutdown,
                text: "Windows shut down.".into(),
                wake_source: None,
                slept_secs: None,
            },
            _ => continue,
        };
        out.push(event);
    }
    out.sort_by_key(|a| std::cmp::Reverse(a.t));
    out
}

trait LocalName {
    fn has_tag_name_local(&self, name: &str) -> bool;
}

impl LocalName for roxmltree::Node<'_, '_> {
    fn has_tag_name_local(&self, name: &str) -> bool {
        self.is_element() && self.tag_name().name() == name
    }
}

/// The last `max` power events from the past `days` days, newest first.
pub fn read_recent(days: u32, max: usize) -> Result<Vec<PowerEvent>, String> {
    let ms = days as u64 * 86_400_000;
    let query = format!(
        "/q:*[System[((Provider[@Name='Microsoft-Windows-Kernel-Power'] and (EventID=42 or EventID=506 or EventID=507 or EventID=41)) or (Provider[@Name='Microsoft-Windows-Power-Troubleshooter'] and EventID=1) or (Provider[@Name='Microsoft-Windows-Kernel-General'] and (EventID=12 or EventID=13))) and TimeCreated[timediff(@SystemTime) <= {ms}]]]"
    );
    let count = format!("/c:{}", max.clamp(1, 2000));
    let out = crate::powercfg::run_system_tool(
        "wevtutil.exe",
        &["qe", "System", &query, "/f:xml", "/rd:true", &count],
        60,
    )?;
    if out.code != 0 && !out.text.contains("<Event") {
        return Err(out.text.trim().to_string());
    }
    Ok(parse(&out.text))
}

/// Invented events for `--demo`: a few days of sleeping and waking.
pub fn demo(now: i64) -> Vec<PowerEvent> {
    let mut out = Vec::new();
    let mut t = now - 6 * 3600;
    let sources = [
        None,
        Some("Northwind Wi-Fi 6E Adapter"),
        Some("Power button"),
        None,
    ];
    for (day, source) in sources.iter().enumerate() {
        let sleep_at = t - 9 * 3600;
        let slept = (t - sleep_at) as u64;
        out.push(PowerEvent {
            t,
            kind: EventKind::Wake,
            text: match source {
                Some(s) => format!(
                    "Woke from sleep after {}. Woken by: {s}.",
                    util::fmt_duration(slept)
                ),
                None => format!(
                    "Woke from sleep after {}. Windows did not record what woke it.",
                    util::fmt_duration(slept)
                ),
            },
            wake_source: source.map(|s| s.to_string()),
            slept_secs: Some(slept),
        });
        out.push(PowerEvent {
            t: sleep_at,
            kind: EventKind::Sleep,
            text: if day % 2 == 0 {
                "Went into sleep, started by the PC being idle.".into()
            } else {
                "Went into sleep, started by a button or the lid.".into()
            },
            wake_source: None,
            slept_secs: None,
        });
        t = sleep_at - 6 * 3600;
    }
    out.insert(
        2,
        PowerEvent {
            t: now - 6 * 3600 - 9 * 3600 - 3 * 3600,
            kind: EventKind::Boot,
            text: "Windows started.".into(),
            wake_source: None,
            slept_secs: None,
        },
    );
    out.sort_by_key(|a| std::cmp::Reverse(a.t));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_events() {
        let ev = parse(include_str!("../tests/fixtures/events-sample.xml"));
        assert_eq!(ev.len(), 6, "{ev:#?}");
        // Newest first.
        assert!(ev.windows(2).all(|w| w[0].t >= w[1].t));
        let wake = ev.iter().find(|e| e.kind == EventKind::Wake).unwrap();
        assert_eq!(wake.slept_secs, Some(9 * 3600 + 30 * 60));
        assert!(wake.text.contains("Woke from hibernation after 9 h 30 min"));
        assert!(wake.text.contains("did not record"));
        let sleep = ev.iter().find(|e| e.kind == EventKind::Sleep).unwrap();
        assert!(sleep.text.contains("idle"));
        assert!(ev.iter().any(|e| e.kind == EventKind::StandbyEnter));
        let exit = ev
            .iter()
            .find(|e| e.kind == EventKind::StandbyExit)
            .unwrap();
        assert!(exit.text.contains("power button"));
        assert!(ev.iter().any(|e| e.kind == EventKind::Unexpected));
        assert!(ev.iter().any(|e| e.kind == EventKind::Boot));
        // Nothing identifying is kept.
        let all = format!("{ev:?}");
        assert!(!all.contains("EXAMPLE-PC") && !all.contains("S-1-5"));
        assert!(parse("").is_empty());
        assert!(parse("<not xml").is_empty());
    }

    #[test]
    fn demo_is_ordered() {
        let d = demo(1_790_000_000);
        assert!(d.len() >= 8);
        assert!(d.windows(2).all(|w| w[0].t >= w[1].t));
    }

    #[test]
    fn reads_the_real_log() {
        // Read-only; may legitimately be empty.
        let ev = read_recent(7, 50).unwrap();
        assert!(ev.len() <= 50);
    }
}
