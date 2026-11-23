//! The energy efficiency report (`powercfg /energy`, administrator, about
//! 60 seconds): running it elevated and summarising the findings in plain
//! language.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Severity {
    Error,
    Warning,
    Info,
}

impl Severity {
    pub fn label(self) -> &'static str {
        match self {
            Severity::Error => "Problem",
            Severity::Warning => "Worth a look",
            Severity::Info => "Information",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Finding {
    pub severity: Severity,
    /// The report's own title (English, as Windows wrote it).
    pub title: String,
    /// What it means, in plain language.
    pub plain: String,
    /// What to do about it (may be empty for information).
    pub advice: String,
    /// Name/value details from the report (no serials or instance IDs).
    pub details: Vec<(String, String)>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct EnergyReport {
    pub t: i64,
    pub duration_secs: u32,
    pub findings: Vec<Finding>,
}

impl EnergyReport {
    pub fn count(&self, severity: Severity) -> usize {
        self.findings
            .iter()
            .filter(|f| f.severity == severity)
            .count()
    }
}

/// Keyword table: (match in the title, plain meaning, advice).
const KNOWN: &[(&str, &str, &str)] = &[
    (
        "usb suspend",
        "A USB device is not going into its power-saving state, so the USB controller and the processor can't rest.",
        "Unplug USB devices you aren't using, or update the device's driver.",
    ),
    (
        "timer resolution",
        "A program asked Windows to check the clock more often than normal. That keeps the processor from resting and drains the battery faster.",
        "Close the program named in the details when you don't need it (browsers playing media and some games do this).",
    ),
    (
        "cpu utilization",
        "The processor was noticeably busy during the measurement.",
        "Look at App power on the left to see which apps use the most power.",
    ),
    (
        "processor utilization",
        "The processor was noticeably busy during the measurement.",
        "Look at App power on the left to see which apps use the most power.",
    ),
    (
        "sleep timeout is disabled",
        "The PC is set never to sleep on its own.",
        "Set \u{201c}Put the PC to sleep after\u{201d} on the Power plans page.",
    ),
    (
        "display timeout",
        "The screen is set to stay on for a long time (or never turn off) when the PC isn't used.",
        "Set a shorter \u{201c}Turn off the screen after\u{201d} on the Power plans page.",
    ),
    (
        "minimum processor",
        "The processor is not allowed to slow down when idle, which wastes power.",
        "Lower \u{201c}Lowest processor speed\u{201d} (5 % is typical) on the Power plans page.",
    ),
    (
        "power policy",
        "A power plan setting is less efficient than it could be.",
        "Check the plan's settings on the Power plans page.",
    ),
    (
        "wireless adapter power",
        "The Wi-Fi adapter is set to use maximum power instead of saving energy.",
        "Set the wireless adapter's power saving mode to medium or maximum in the plan's advanced settings.",
    ),
    (
        "aspm",
        "PCI Express link power management is off, so some internal links stay at full power.",
        "Usually set by the PC maker; a firmware or chipset driver update may enable it.",
    ),
    (
        "pci express",
        "PCI Express link power management is off, so some internal links stay at full power.",
        "Usually set by the PC maker; a firmware or chipset driver update may enable it.",
    ),
    (
        "platform power management",
        "Information about the power-saving features the PC's hardware supports.",
        "",
    ),
    (
        "battery",
        "Information about the battery that Windows recorded.",
        "",
    ),
    (
        "video quality",
        "Video playback is set to favour quality over battery life.",
        "In Windows Settings, Apps, Video playback, choose to optimise for battery life.",
    ),
    (
        "device failed to suspend",
        "A device did not go into its power-saving state when asked.",
        "Update the device's driver, or unplug it when not in use.",
    ),
];

fn explain(title: &str, description: &str) -> (String, String) {
    let t = title.to_lowercase();
    for (key, plain, advice) in KNOWN {
        if t.contains(key) {
            return (plain.to_string(), advice.to_string());
        }
    }
    let plain = if description.is_empty() {
        title.to_string()
    } else {
        description.to_string()
    };
    (plain, String::new())
}

/// Device instance paths, GUIDs in braces and serial numbers are dropped.
fn identifying(name: &str, value: &str) -> bool {
    let n = name.to_lowercase();
    let v = value.to_uppercase();
    n.contains("serial")
        || n.contains("instance")
        || n.contains("device id")
        || n.contains("computer")
        || (value.contains('\\')
            && [
                "VEN_",
                "PCI\\",
                "USB\\",
                "ACPI\\",
                "HID\\",
                "HDAUDIO\\",
                "BTH",
                "SWD\\",
                "ROOT\\",
            ]
            .iter()
            .any(|k| v.contains(k)))
        || (value.contains('{') && value.contains('}') && value.contains('-'))
}

fn severity_of(text: &str) -> Severity {
    match text.trim().to_lowercase().as_str() {
        "error" | "1" | "critical" => Severity::Error,
        "warning" | "2" => Severity::Warning,
        _ => Severity::Info,
    }
}

/// Parses the XML of `powercfg /energy /xml`.
pub fn parse(xml: &str) -> Result<EnergyReport, String> {
    let xml = xml.trim_start_matches('\u{feff}');
    let doc = roxmltree::Document::parse(xml).map_err(|e| format!("not an energy report: {e}"))?;
    let named = |n: &roxmltree::Node, name: &str| {
        n.is_element() && n.tag_name().name().eq_ignore_ascii_case(name)
    };
    let mut report = EnergyReport::default();
    let mut any = false;
    for entry in doc.descendants().filter(|n| named(n, "LogEntry")) {
        any = true;
        let child_text = |name: &str| {
            entry
                .children()
                .find(|c| named(c, name))
                .and_then(|c| c.text())
                .unwrap_or("")
                .trim()
                .to_string()
        };
        let mut title = child_text("Name");
        if title.is_empty() {
            title = child_text("Title");
        }
        let severity = severity_of(&child_text("Severity"));
        let description = child_text("Description");
        let mut details = Vec::new();
        if let Some(d) = entry.children().find(|c| named(c, "Details")) {
            for detail in d.children().filter(|c| named(c, "Detail")) {
                let get = |name: &str| {
                    detail
                        .children()
                        .find(|c| named(c, name))
                        .and_then(|c| c.text())
                        .unwrap_or("")
                        .trim()
                        .to_string()
                };
                let (name, value) = (get("Name"), get("Value"));
                if name.is_empty() || identifying(&name, &value) {
                    continue;
                }
                details.push((name, value));
            }
        }
        let (plain, advice) = explain(&title, &description);
        report.findings.push(Finding {
            severity,
            title,
            plain,
            advice,
            details,
        });
    }
    if !any && !doc.root_element().tag_name().name().contains("Energy") {
        return Err("not an energy report".into());
    }
    report.findings.sort_by_key(|f| match f.severity {
        Severity::Error => 0,
        Severity::Warning => 1,
        Severity::Info => 2,
    });
    Ok(report)
}

/// Runs the trace elevated (one UAC prompt) for `duration_secs`, calling
/// `progress(0.0..=1.0)` about once a second, then parses and deletes the
/// output file. Blocking: call from a worker thread.
pub fn run(duration_secs: u32, progress: &dyn Fn(f32)) -> Result<EnergyReport, String> {
    let duration = duration_secs.clamp(5, 300);
    let dir = crate::elevate::temp_job_dir("energy").map_err(|e| e.to_string())?;
    let file = dir.join("energy-report.xml");
    let args: Vec<String> = vec![
        "/energy".into(),
        "/xml".into(),
        "/output".into(),
        file.to_string_lossy().to_string(),
        "/duration".into(),
        duration.to_string(),
    ];
    let worker = std::thread::spawn(move || crate::elevate::powercfg(&args, duration as u64 + 120));
    let start = std::time::Instant::now();
    while !worker.is_finished() {
        let f = start.elapsed().as_secs_f32() / (duration as f32 + 5.0);
        progress(f.min(0.98));
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    let outcome = worker
        .join()
        .map_err(|_| "the energy report stopped unexpectedly".to_string())
        .and_then(|r| r.map_err(|e| e.to_string()));
    let result = outcome.and_then(|text| {
        let bytes = std::fs::read(&file).map_err(|_| {
            let t = text.trim();
            if t.is_empty() {
                "Windows did not create an energy report.".to_string()
            } else {
                t.to_string()
            }
        })?;
        let mut r = parse(&String::from_utf8_lossy(&bytes))?;
        r.t = crate::util::now();
        r.duration_secs = duration;
        Ok(r)
    });
    let _ = std::fs::remove_dir_all(&dir);
    progress(1.0);
    result
}

/// Invented findings for `--demo`.
pub fn demo() -> EnergyReport {
    let f = |severity, title: &str, details: &[(&str, &str)]| {
        let (plain, advice) = explain(title, "");
        Finding {
            severity,
            title: title.to_string(),
            plain,
            advice,
            details: details
                .iter()
                .map(|(a, b)| (a.to_string(), b.to_string()))
                .collect(),
        }
    };
    EnergyReport {
        t: crate::util::now() - 600,
        duration_secs: 60,
        findings: vec![
            f(
                Severity::Error,
                "Platform Timer Resolution:Platform Timer Resolution",
                &[
                    ("Requested period", "10000"),
                    ("Requesting process", "media-player.exe"),
                ],
            ),
            f(
                Severity::Error,
                "USB Suspend:USB Device not Entering Suspend",
                &[
                    ("Device name", "USB Receiver"),
                    ("Host controller", "USB eXtensible Host Controller"),
                ],
            ),
            f(
                Severity::Warning,
                "Power Policy:Display timeout is long (Plugged In)",
                &[("Timeout (seconds)", "3600")],
            ),
            f(
                Severity::Warning,
                "Power Policy:Minimum processor performance state is 100% (Plugged In)",
                &[],
            ),
            f(
                Severity::Warning,
                "CPU Utilization:Processor utilization is high",
                &[("Average utilization (%)", "18.4")],
            ),
            f(
                Severity::Warning,
                "Wireless Adapter Power Saving Mode",
                &[("Plugged in", "Maximum performance")],
            ),
            f(
                Severity::Info,
                "Battery:Battery Information",
                &[("Design capacity", "56000"), ("Last full charge", "48720")],
            ),
            f(
                Severity::Info,
                "Platform Power Management Capabilities:Supported Sleep States",
                &[("S0 low power idle", "Supported")],
            ),
            f(
                Severity::Info,
                "Platform Power Management Capabilities:Processor Power Management Capabilities",
                &[("Idle state count", "3")],
            ),
            f(
                Severity::Info,
                "Platform Power Management Capabilities:PCI Express ASPM Disabled",
                &[],
            ),
            f(Severity::Info, "Video quality setting", &[]),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_sample() {
        let r = parse(include_str!("../tests/fixtures/energy-sample.xml")).unwrap();
        assert_eq!(r.findings.len(), 4);
        assert_eq!(r.count(Severity::Error), 1);
        assert_eq!(r.count(Severity::Warning), 2);
        assert_eq!(r.count(Severity::Info), 1);
        // Errors first.
        assert_eq!(r.findings[0].severity, Severity::Error);
        assert!(r.findings[0].plain.contains("check the clock more often"));
        assert!(r.findings[0].details.iter().any(|(_, v)| v == "player.exe"));
        // The device instance path and the serial are dropped.
        let all = format!("{r:?}");
        assert!(!all.contains("VEN_") && !all.contains("SN-1234") && !all.contains("{4d36"));
        let usb = r.findings.iter().find(|f| f.title.contains("USB")).unwrap();
        assert!(usb.details.iter().any(|(n, _)| n == "Device Name"));
        // An unknown title falls back to the report's description.
        let other = r
            .findings
            .iter()
            .find(|f| f.title == "Something new")
            .unwrap();
        assert_eq!(other.plain, "A description from Windows.");
        assert!(other.advice.is_empty());
    }

    #[test]
    fn rejects_other_xml_and_maps_severity() {
        assert!(parse("<BatteryReport/>").is_err());
        assert!(parse("nope").is_err());
        assert!(parse("<EnergyReport/>").unwrap().findings.is_empty());
        assert_eq!(severity_of("Error"), Severity::Error);
        assert_eq!(severity_of("2"), Severity::Warning);
        assert_eq!(severity_of("Information"), Severity::Info);
        assert!(identifying("Device ID", "x"));
        assert!(identifying("Path", "PCI\\VEN_8086&DEV_0000\\3&0"));
        assert!(!identifying("Device Name", "USB Receiver"));
    }

    #[test]
    fn demo_counts() {
        let d = demo();
        assert_eq!(d.count(Severity::Error), 2);
        assert_eq!(d.count(Severity::Warning), 4);
        assert_eq!(d.count(Severity::Info), 5);
        assert!(d.findings.iter().all(|f| !f.plain.is_empty()));
    }
}
