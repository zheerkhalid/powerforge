//! The Windows battery report (`powercfg /batteryreport /xml`): parsing and
//! running it. The serial number, battery Id and computer name in the
//! report are never read.

use crate::util;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ReportBattery {
    pub manufacturer: String,
    pub chemistry: String,
    pub design_mwh: Option<u32>,
    pub full_mwh: Option<u32>,
    pub cycle_count: Option<u32>,
}

/// One period of the report's capacity and usage history.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct HistoryPeriod {
    pub start: i64,
    pub end: i64,
    pub design_mwh: Option<u32>,
    pub full_mwh: Option<u32>,
    pub cycle_count: Option<u32>,
    /// Seconds of active use on battery / plugged in, and in standby.
    pub active_dc_secs: u64,
    pub active_ac_secs: u64,
    pub standby_dc_secs: u64,
    pub standby_ac_secs: u64,
    /// Estimated active runtime on a full charge at the time.
    pub est_full_active_secs: Option<u64>,
    pub est_design_active_secs: Option<u64>,
    /// The battery was replaced during this period.
    pub battery_changed: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum UsageKind {
    #[default]
    Active,
    Suspend,
    /// Modern Standby (connected standby).
    Standby,
    Hibernate,
    Other,
}

impl UsageKind {
    pub fn label(self) -> &'static str {
        match self {
            UsageKind::Active => "In use",
            UsageKind::Suspend => "Asleep",
            UsageKind::Standby => "Standby",
            UsageKind::Hibernate => "Hibernated",
            UsageKind::Other => "Other",
        }
    }
}

/// One entry of "recent usage" (the last few days).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct UsageEntry {
    pub t: i64,
    pub duration_secs: u64,
    pub ac: bool,
    pub kind: UsageKind,
    pub charge_mwh: Option<u32>,
    pub full_mwh: Option<u32>,
    /// Energy used during the entry (negative = charged).
    pub discharge_mwh: i64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct BatteryReport {
    pub scan_time: i64,
    /// The PC supports Modern Standby (connected standby).
    pub modern_standby: bool,
    pub batteries: Vec<ReportBattery>,
    pub history: Vec<HistoryPeriod>,
    pub recent: Vec<UsageEntry>,
    /// Current estimates of active-use runtime on a full charge (seconds).
    pub runtime_full_secs: Option<u64>,
    pub runtime_design_secs: Option<u64>,
    /// Same, in Modern Standby.
    pub standby_full_secs: Option<u64>,
    pub standby_design_secs: Option<u64>,
}

/// Summary of Modern Standby sessions from the recent usage: a light
/// "sleep study" that needs no administrator rights.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct StandbySummary {
    pub sessions: u32,
    pub total_secs: u64,
    pub drained_mwh: i64,
    /// Average drain in percent of a full charge per hour of standby.
    pub pct_per_hour: Option<f32>,
    /// Plain-language verdict.
    pub verdict: String,
}

type Node<'a, 'b> = roxmltree::Node<'a, 'b>;

fn child<'a, 'b>(n: Node<'a, 'b>, name: &str) -> Option<Node<'a, 'b>> {
    n.children()
        .find(|c| c.is_element() && c.tag_name().name() == name)
}

fn text_of(n: Node, name: &str) -> String {
    child(n, name)
        .and_then(|c| c.text())
        .unwrap_or("")
        .trim()
        .to_string()
}

fn num(text: &str) -> Option<u32> {
    text.trim().parse::<u32>().ok().filter(|v| *v > 0)
}

fn attr_u32(n: Node, name: &str) -> Option<u32> {
    n.attribute(name).and_then(num)
}

fn attr_secs(n: Node, name: &str) -> u64 {
    n.attribute(name)
        .and_then(util::parse_iso_duration)
        .unwrap_or(0)
}

/// Parses the XML of `powercfg /batteryreport /xml`.
pub fn parse(xml: &str) -> Result<BatteryReport, String> {
    let xml = xml.trim_start_matches('\u{feff}');
    let doc = roxmltree::Document::parse(xml).map_err(|e| format!("not a battery report: {e}"))?;
    let root = doc.root_element();
    if root.tag_name().name() != "BatteryReport" {
        return Err("not a battery report".into());
    }
    let mut r = BatteryReport::default();
    if let Some(info) = child(root, "ReportInformation") {
        r.scan_time = util::parse_iso_datetime(&text_of(info, "ScanTime")).unwrap_or(0);
    }
    if let Some(sys) = child(root, "SystemInformation") {
        r.modern_standby = text_of(sys, "ConnectedStandby") == "1";
    }
    if let Some(bats) = child(root, "Batteries") {
        for b in bats
            .children()
            .filter(|c| c.is_element() && c.tag_name().name() == "Battery")
        {
            // Only these fields; Id and SerialNumber are deliberately not read.
            let relative = text_of(b, "RelativeCapacity") == "1";
            let cap = |name: &str| (!relative).then(|| num(&text_of(b, name))).flatten();
            r.batteries.push(ReportBattery {
                manufacturer: text_of(b, "Manufacturer"),
                chemistry: crate::battery::chemistry_name(text_of(b, "Chemistry").as_bytes()),
                design_mwh: cap("DesignCapacity"),
                full_mwh: cap("FullChargeCapacity"),
                cycle_count: num(&text_of(b, "CycleCount")),
            });
        }
    }
    if let Some(est) = child(root, "RuntimeEstimates") {
        let get = |which: &str, what: &str| {
            child(est, which).and_then(|n| util::parse_iso_duration(&text_of(n, what)))
        };
        r.runtime_full_secs = get("FullChargeCapacity", "ActiveRuntime").filter(|s| *s > 0);
        r.runtime_design_secs = get("DesignCapacity", "ActiveRuntime").filter(|s| *s > 0);
        r.standby_full_secs =
            get("FullChargeCapacity", "ConnectedStandbyRuntime").filter(|s| *s > 0);
        r.standby_design_secs = get("DesignCapacity", "ConnectedStandbyRuntime").filter(|s| *s > 0);
    }
    if let Some(recent) = child(root, "RecentUsage") {
        for e in recent
            .children()
            .filter(|c| c.is_element() && c.tag_name().name() == "UsageEntry")
        {
            let Some(t) = e.attribute("Timestamp").and_then(util::parse_iso_datetime) else {
                continue;
            };
            let kind = match e.attribute("EntryType").unwrap_or("") {
                "Active" => UsageKind::Active,
                "Suspend" => UsageKind::Suspend,
                "ConnectedStandby" => UsageKind::Standby,
                "Hibernate" => UsageKind::Hibernate,
                _ => UsageKind::Other,
            };
            r.recent.push(UsageEntry {
                t,
                // 100-nanosecond units.
                duration_secs: e
                    .attribute("Duration")
                    .and_then(|d| d.parse::<u64>().ok())
                    .map(|d| d / 10_000_000)
                    .unwrap_or(0),
                ac: e.attribute("Ac") == Some("1"),
                kind,
                charge_mwh: attr_u32(e, "ChargeCapacity"),
                full_mwh: attr_u32(e, "FullChargeCapacity"),
                discharge_mwh: e
                    .attribute("Discharge")
                    .and_then(|d| d.parse().ok())
                    .unwrap_or(0),
            });
        }
    }
    if let Some(hist) = child(root, "History") {
        for e in hist
            .children()
            .filter(|c| c.is_element() && c.tag_name().name() == "HistoryEntry")
        {
            let (Some(start), Some(end)) = (
                e.attribute("StartDate").and_then(util::parse_iso_datetime),
                e.attribute("EndDate").and_then(util::parse_iso_datetime),
            ) else {
                continue;
            };
            let est = |name: &str| {
                e.attribute(name)
                    .and_then(util::parse_iso_duration)
                    .filter(|s| *s > 0)
            };
            r.history.push(HistoryPeriod {
                start,
                end,
                design_mwh: attr_u32(e, "DesignCapacity"),
                full_mwh: attr_u32(e, "FullChargeCapacity"),
                cycle_count: attr_u32(e, "CycleCount"),
                active_dc_secs: attr_secs(e, "ActiveDcTime"),
                active_ac_secs: attr_secs(e, "ActiveAcTime"),
                standby_dc_secs: attr_secs(e, "CsDcTime"),
                standby_ac_secs: attr_secs(e, "CsAcTime"),
                est_full_active_secs: est("EstimatedFullChargeActiveTime"),
                est_design_active_secs: est("EstimatedDesignActiveTime"),
                battery_changed: e.attribute("BatteryChanged") == Some("1"),
            });
        }
    }
    r.history.sort_by_key(|h| h.start);
    r.recent.sort_by_key(|u| u.t);
    Ok(r)
}

/// Runs `powercfg /batteryreport /xml` into a temporary file, parses it and
/// deletes the file. No administrator rights needed. Takes a few seconds:
/// call from a worker thread.
pub fn generate() -> Result<BatteryReport, String> {
    let dir = crate::elevate::temp_job_dir("report").map_err(|e| e.to_string())?;
    let file = dir.join("battery-report.xml");
    let path = file.to_string_lossy().to_string();
    let result = crate::powercfg::run(&["/batteryreport", "/xml", "/output", &path], 90)
        .and_then(|out| {
            std::fs::read(&file).map_err(|_| {
                let t = out.text.trim();
                if t.is_empty() {
                    "Windows did not create a battery report.".to_string()
                } else {
                    t.to_string()
                }
            })
        })
        .and_then(|bytes| parse(&String::from_utf8_lossy(&bytes)));
    let _ = std::fs::remove_dir_all(&dir);
    result
}

/// Summarises the Modern Standby entries of the recent usage.
pub fn standby_summary(report: &BatteryReport) -> Option<StandbySummary> {
    let entries: Vec<&UsageEntry> = report
        .recent
        .iter()
        .filter(|e| e.kind == UsageKind::Standby && !e.ac && e.duration_secs > 0)
        .collect();
    if entries.is_empty() {
        return None;
    }
    let total_secs: u64 = entries.iter().map(|e| e.duration_secs).sum();
    let drained_mwh: i64 = entries.iter().map(|e| e.discharge_mwh).sum();
    let full = entries
        .iter()
        .filter_map(|e| e.full_mwh)
        .max()
        .or_else(|| report.batteries.first().and_then(|b| b.full_mwh));
    let hours = total_secs as f64 / 3600.0;
    let pct_per_hour = match full {
        Some(f) if hours >= 0.25 && f > 0 => {
            Some((drained_mwh.max(0) as f64 / f as f64 * 100.0 / hours) as f32)
        }
        _ => None,
    };
    let verdict = match pct_per_hour {
        None => "Not enough time in standby yet to judge the drain.".to_string(),
        Some(p) if p < 1.0 => format!(
            "Normal: the battery loses about {p:.1} % per hour while asleep. A full battery would last {} in standby.",
            util::fmt_duration((100.0 / p.max(0.05) * 3600.0) as u64)
        ),
        Some(p) if p <= 3.0 => format!(
            "Noticeable: the battery loses about {p:.1} % per hour while asleep. Check which devices may wake the PC and what keeps it busy on the Sleep & wake page."
        ),
        Some(p) => format!(
            "High drain: the battery loses about {p:.1} % per hour while asleep. Something keeps the PC busy in standby: check wake-armed devices and wake timers on the Sleep & wake page, or turn on hibernation for long breaks."
        ),
    };
    Some(StandbySummary {
        sessions: entries.len() as u32,
        total_secs,
        drained_mwh,
        pct_per_hour,
        verdict,
    })
}

/// Splits usage into hours on battery and plugged in, per kind, for the
/// last `days` days before `now`.
pub fn usage_split(report: &BatteryReport, now: i64, days: u32) -> (u64, u64, u64) {
    let from = now - days as i64 * 86_400;
    let mut active_dc = 0;
    let mut active_ac = 0;
    let mut asleep = 0;
    for e in report.recent.iter().filter(|e| e.t >= from) {
        match (e.kind, e.ac) {
            (UsageKind::Active, false) => active_dc += e.duration_secs,
            (UsageKind::Active, true) => active_ac += e.duration_secs,
            _ => asleep += e.duration_secs,
        }
    }
    (active_dc, active_ac, asleep)
}

/// An invented report for `--demo` (two years of history, matching
/// `battery::demo()`).
pub fn demo() -> BatteryReport {
    let now = util::now();
    let weeks = 104i64;
    let start = now - weeks * 7 * 86_400;
    let history = (0..weeks)
        .map(|w| {
            let f = w as f32 / (weeks - 1) as f32;
            // Wear is a little faster at first, then steady, with a wobble.
            let wobble = ((w as f32) * 0.9).sin() * 180.0;
            let full = 56_000.0 - 7_280.0 * (0.35 * f.sqrt() + 0.65 * f) + wobble;
            HistoryPeriod {
                start: start + w * 7 * 86_400,
                end: start + (w + 1) * 7 * 86_400,
                design_mwh: Some(56_000),
                full_mwh: Some(full.min(56_000.0) as u32),
                cycle_count: Some((f * 312.0) as u32),
                active_dc_secs: 9 * 3600 + (w as u64 % 5) * 1800,
                active_ac_secs: 22 * 3600,
                standby_dc_secs: 30 * 3600,
                standby_ac_secs: 10 * 3600,
                est_full_active_secs: Some((full / 9_800.0 * 3600.0) as u64),
                est_design_active_secs: Some((56_000.0 / 9_800.0 * 3600.0) as u64),
                battery_changed: false,
            }
        })
        .collect();
    let mut recent = Vec::new();
    let mut t = now - 3 * 86_400;
    let mut charge = 46_000i64;
    while t < now - 3600 {
        // Work on battery, a standby night, then charging.
        let steps: [(UsageKind, bool, u64, i64); 4] = [
            (UsageKind::Active, false, 3 * 3600 + 1200, 30_500),
            (UsageKind::Standby, false, 9 * 3600, 3_900),
            (UsageKind::Active, true, 2 * 3600, -36_000),
            (UsageKind::Active, false, 5 * 3600, 0),
        ];
        for (kind, ac, dur, drain) in steps {
            let drain = if drain == 0 {
                (charge - 20_000).max(0) / 2
            } else {
                drain
            };
            recent.push(UsageEntry {
                t,
                duration_secs: dur,
                ac,
                kind,
                charge_mwh: Some(charge.clamp(0, 48_720) as u32),
                full_mwh: Some(48_720),
                discharge_mwh: drain,
            });
            charge = (charge - drain).clamp(2_000, 48_720);
            t += dur as i64;
        }
    }
    BatteryReport {
        scan_time: now,
        modern_standby: true,
        batteries: vec![ReportBattery {
            manufacturer: "Northwind Cells".into(),
            chemistry: "Lithium-ion".into(),
            design_mwh: Some(56_000),
            full_mwh: Some(48_720),
            cycle_count: Some(312),
        }],
        history,
        recent,
        runtime_full_secs: Some(48_720 * 3600 / 9_800),
        runtime_design_secs: Some(56_000 * 3600 / 9_800),
        standby_full_secs: Some(48_720 * 3600 / 430),
        standby_design_secs: Some(56_000 * 3600 / 430),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = include_str!("../tests/fixtures/battery-report-sample.xml");

    #[test]
    fn parses_sample() {
        let r = parse(SAMPLE).unwrap();
        assert_eq!(
            r.scan_time,
            util::parse_iso_datetime("2026-11-23T21:24:21Z").unwrap()
        );
        assert!(r.modern_standby);
        assert_eq!(r.batteries.len(), 1);
        let b = &r.batteries[0];
        assert_eq!(b.manufacturer, "Example Cells");
        assert_eq!(b.chemistry, "Lithium polymer");
        assert_eq!(b.design_mwh, Some(50_000));
        assert_eq!(b.full_mwh, Some(46_500));
        assert_eq!(b.cycle_count, Some(140));
        assert_eq!(r.runtime_full_secs, Some(3 * 3600 + 22 * 60 + 30));
        assert_eq!(r.standby_design_secs, Some(2 * 86_400 + 3600 + 2 * 60 + 51));
        assert_eq!(r.recent.len(), 4);
        assert_eq!(r.recent[0].kind, UsageKind::Standby);
        assert_eq!(r.recent[0].duration_secs, 46);
        assert_eq!(r.recent[2].discharge_mwh, -11_880);
        assert!(r.recent[2].ac);
        assert_eq!(r.history.len(), 3);
        assert_eq!(r.history[0].design_mwh, Some(60_000));
        assert_eq!(r.history[0].active_dc_secs, 2 * 3600 + 38 * 60 + 43);
        assert_eq!(r.history[0].est_full_active_secs, Some(2 * 3600 + 60 + 21));
        assert!(r.history[2].battery_changed);
        // The serial number, battery Id and computer name are never kept.
        let all = format!("{r:?}");
        assert!(
            !all.contains("SERIAL-0000") && !all.contains("EXAMPLE-PC") && !all.contains("ID-0000")
        );
    }

    #[test]
    fn rejects_garbage_and_handles_desktops() {
        assert!(parse("<html/>").is_err());
        assert!(parse("not xml").is_err());
        let desktop = parse(
            "<?xml version=\"1.0\"?><BatteryReport xmlns=\"http://schemas.microsoft.com/battery/2012\"><Batteries/></BatteryReport>",
        )
        .unwrap();
        assert!(desktop.batteries.is_empty());
        assert!(standby_summary(&desktop).is_none());
    }

    #[test]
    fn standby_verdicts() {
        let mut r = BatteryReport::default();
        let entry = |dur: u64, drain: i64| UsageEntry {
            t: 0,
            duration_secs: dur,
            ac: false,
            kind: UsageKind::Standby,
            charge_mwh: None,
            full_mwh: Some(50_000),
            discharge_mwh: drain,
        };
        r.recent = vec![entry(10 * 3600, 2_500)];
        let s = standby_summary(&r).unwrap();
        assert_eq!(s.sessions, 1);
        assert!((s.pct_per_hour.unwrap() - 0.5).abs() < 0.01);
        assert!(s.verdict.starts_with("Normal"));
        r.recent = vec![entry(5 * 3600, 5_000)];
        assert!(standby_summary(&r)
            .unwrap()
            .verdict
            .starts_with("Noticeable"));
        r.recent = vec![entry(2 * 3600, 5_000)];
        assert!(standby_summary(&r)
            .unwrap()
            .verdict
            .starts_with("High drain"));
        r.recent = vec![entry(300, 10)];
        assert!(standby_summary(&r).unwrap().pct_per_hour.is_none());
    }

    #[test]
    fn demo_matches_live_demo() {
        let r = demo();
        assert_eq!(r.history.len(), 104);
        let last = r.history.last().unwrap();
        let h = crate::health::health_percent(last.design_mwh, last.full_mwh).unwrap();
        assert!((h - 87.0).abs() < 1.5, "{h}");
        let s = standby_summary(&r).unwrap();
        assert!(s.verdict.starts_with("Normal"), "{}", s.verdict);
        let (dc, ac, asleep) = usage_split(&r, util::now(), 7);
        assert!(dc > 0 && ac > 0 && asleep > 0);
    }

    #[test]
    #[ignore = "runs powercfg /batteryreport on this PC (read-only, temp file removed)"]
    fn generates_on_this_pc() {
        let r = generate().unwrap();
        assert!(r.scan_time > 0);
    }
}
