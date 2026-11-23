//! Exported reports: a self-contained HTML page (no scripts, no external
//! files) and CSV files. No serials, device instance IDs or computer names
//! appear in any export.

use crate::battery::BatteryInfo;
use crate::batteryreport::{self, BatteryReport};
use crate::diag::DiagSnapshot;
use crate::energy::{EnergyReport, Severity};
use crate::health;
use crate::history::History;
use crate::util::{self, csv_field, html_escape as esc};
use std::fmt::Write;

/// Everything a report may contain; absent parts are left out.
pub struct ExportInput<'a> {
    pub now: i64,
    pub battery: Option<&'a BatteryInfo>,
    pub report: Option<&'a BatteryReport>,
    pub history: &'a History,
    pub diag: Option<&'a DiagSnapshot>,
    pub energy: Option<&'a EnergyReport>,
    /// Name of the active power plan.
    pub plan: Option<&'a str>,
}

const STYLE: &str = "
:root{--bg:#f5f6f8;--card:#fff;--text:#1a1d24;--muted:#5d6472;--border:#dde1e8;--accent:#0b7f86;--warn:#a06a00;--bad:#c0392b;--good:#1b8a5a}
@media (prefers-color-scheme:dark){:root{--bg:#101318;--card:#181c23;--text:#e9ecf1;--muted:#9aa3b2;--border:#2a303b;--accent:#36c2b8;--warn:#f0c040;--bad:#ff6b5e;--good:#3ecf8e}}
*{box-sizing:border-box}body{margin:0;padding:32px 16px;background:var(--bg);color:var(--text);font:15px/1.5 'Segoe UI',system-ui,sans-serif}
main{max-width:960px;margin:0 auto}h1{margin:0 0 4px;font-size:28px}h2{margin:0 0 12px;font-size:19px}
.sub{color:var(--muted);margin:0 0 24px}.card{background:var(--card);border:1px solid var(--border);border-radius:12px;padding:20px;margin:0 0 16px}
.grid{display:grid;grid-template-columns:repeat(auto-fit,minmax(170px,1fr));gap:12px}.stat b{display:block;font-size:22px}.stat span{color:var(--muted);font-size:13px}
table{width:100%;border-collapse:collapse;font-size:14px}th,td{text-align:left;padding:6px 8px;border-bottom:1px solid var(--border);vertical-align:top}th{color:var(--muted);font-weight:600}
.e{color:var(--bad);font-weight:600}.w{color:var(--warn);font-weight:600}.i{color:var(--muted);font-weight:600}.muted{color:var(--muted)}
svg{width:100%;height:auto}footer{color:var(--muted);font-size:13px;text-align:center;margin-top:24px}
";

fn stat(out: &mut String, value: &str, caption: &str) {
    let _ = write!(
        out,
        "<div class=\"stat\"><b>{}</b><span>{}</span></div>",
        esc(value),
        esc(caption)
    );
}

/// An inline SVG line chart of health over time.
fn health_chart(points: &[health::HealthPoint]) -> String {
    if points.len() < 2 {
        return String::new();
    }
    let (w, h, pad) = (900.0f32, 220.0f32, 36.0f32);
    let t0 = points[0].t as f32;
    let t1 = points[points.len() - 1].t as f32;
    let lo = points
        .iter()
        .map(|p| p.health_pct)
        .fold(f32::MAX, f32::min)
        .min(80.0)
        .floor();
    let hi = 100.0f32.max(points.iter().map(|p| p.health_pct).fold(0.0, f32::max));
    let x = |t: i64| pad + (t as f32 - t0) / (t1 - t0).max(1.0) * (w - 2.0 * pad);
    let y = |v: f32| h - pad - (v - lo) / (hi - lo).max(1.0) * (h - 2.0 * pad);
    let mut path = String::new();
    for (i, p) in points.iter().enumerate() {
        let _ = write!(
            path,
            "{}{:.1},{:.1} ",
            if i == 0 { "M" } else { "L" },
            x(p.t),
            y(p.health_pct)
        );
    }
    let mut s = format!(
        "<svg viewBox=\"0 0 {w} {h}\" role=\"img\" aria-label=\"Battery health over time\">"
    );
    for v in [lo, (lo + hi) / 2.0, hi] {
        let _ = write!(
            s,
            "<line x1=\"{pad}\" x2=\"{}\" y1=\"{:.1}\" y2=\"{:.1}\" stroke=\"currentColor\" stroke-opacity=\".15\"/><text x=\"2\" y=\"{:.1}\" font-size=\"11\" fill=\"currentColor\" fill-opacity=\".6\">{:.0} %</text>",
            w - pad,
            y(v),
            y(v),
            y(v) + 4.0,
            v
        );
    }
    let _ = write!(
        s,
        "<text x=\"{pad}\" y=\"{}\" font-size=\"11\" fill=\"currentColor\" fill-opacity=\".6\">{}</text><text x=\"{}\" y=\"{}\" font-size=\"11\" text-anchor=\"end\" fill=\"currentColor\" fill-opacity=\".6\">{}</text>",
        h - 10.0,
        esc(&util::fmt_day(points[0].t)),
        w - pad,
        h - 10.0,
        esc(&util::fmt_day(points[points.len() - 1].t))
    );
    let _ = write!(
        s,
        "<path d=\"{path}\" fill=\"none\" stroke=\"#0b9f99\" stroke-width=\"2.5\" stroke-linejoin=\"round\"/></svg>"
    );
    s
}

/// A complete HTML document (inline CSS, readable in light and dark).
pub fn html(input: &ExportInput) -> String {
    let mut o = String::new();
    let _ = write!(
        o,
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>PowerForge report {}</title><style>{STYLE}</style></head><body><main>",
        esc(&util::fmt_date(input.now))
    );
    let _ = write!(
        o,
        "<h1>Power and battery report</h1><p class=\"sub\">{}</p>",
        esc(&util::fmt_datetime(input.now))
    );

    // Summary.
    o.push_str("<section class=\"card\"><h2>Summary</h2><div class=\"grid\">");
    match input.battery {
        Some(b) => {
            stat(&mut o, &format!("{:.0} %", b.percent), "Charge");
            stat(&mut o, b.state.label(), "State");
            stat(
                &mut o,
                if b.ac_online { "Plugged in" } else { "Battery" },
                "Power source",
            );
            if let Some(h) = health::health_percent(b.design_mwh, b.full_mwh) {
                stat(
                    &mut o,
                    &format!("{h:.0} %"),
                    &format!("Health ({})", health::grade(h).label()),
                );
            }
            if let Some(c) = b.cycle_count {
                stat(&mut o, &c.to_string(), "Charge cycles");
            }
            if let Some(d) = b.design_mwh {
                stat(&mut o, &util::fmt_wh(d as u64), "Design capacity");
            }
            if let Some(f) = b.full_mwh {
                stat(&mut o, &util::fmt_wh(f as u64), "Full charge capacity");
            }
            if !b.chemistry.is_empty() {
                stat(&mut o, &b.chemistry, "Chemistry");
            }
            if !b.manufacturer.is_empty() {
                stat(&mut o, &b.manufacturer, "Manufacturer");
            }
        }
        None => stat(&mut o, "No battery", "This PC runs on mains power"),
    }
    if let Some(plan) = input.plan {
        stat(&mut o, plan, "Power plan");
    }
    o.push_str("</div>");
    if let Some(b) = input.battery {
        if let Some(h) = health::health_percent(b.design_mwh, b.full_mwh) {
            let _ = write!(o, "<p>{}</p>", esc(&health::explain(h, b.cycle_count)));
        }
    }
    o.push_str("</section>");

    // Health history.
    let points = input.history.health_points(input.report);
    if !points.is_empty() {
        o.push_str("<section class=\"card\"><h2>Battery health over time</h2>");
        o.push_str(&health_chart(&points));
        if let Some(t) = health::wear_trend(&points) {
            let _ = write!(o, "<p>{}</p>", esc(&health::explain_trend(&t)));
        }
        o.push_str("<table><tr><th>Date</th><th>Health</th><th>Charge cycles</th></tr>");
        let step = (points.len() / 24).max(1);
        let mut shown: Vec<&health::HealthPoint> = points.iter().step_by(step).collect();
        if let Some(last) = points.last() {
            if shown.last().map(|p| p.t) != Some(last.t) {
                shown.push(last);
            }
        }
        for p in shown.iter().rev() {
            let _ = write!(
                o,
                "<tr><td>{}</td><td>{:.1} %</td><td>{}</td></tr>",
                esc(&util::fmt_day(p.t)),
                p.health_pct,
                p.cycles
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| "-".into())
            );
        }
        o.push_str("</table></section>");
    }

    // Usage and standby.
    if let Some(r) = input.report {
        o.push_str("<section class=\"card\"><h2>Recent use</h2><div class=\"grid\">");
        let (dc, ac, asleep) = batteryreport::usage_split(r, input.now, 7);
        stat(
            &mut o,
            &util::fmt_duration(dc),
            "In use on battery (7 days)",
        );
        stat(
            &mut o,
            &util::fmt_duration(ac),
            "In use plugged in (7 days)",
        );
        stat(&mut o, &util::fmt_duration(asleep), "Asleep (7 days)");
        if let Some(s) = r.runtime_full_secs {
            stat(
                &mut o,
                &util::fmt_duration(s),
                "Windows' estimate for a full charge",
            );
        }
        o.push_str("</div>");
        if let Some(s) = batteryreport::standby_summary(r) {
            let _ = write!(
                o,
                "<h2 style=\"margin-top:16px\">Standby</h2><p>{} ({} sessions, {} in total.)</p>",
                esc(&s.verdict),
                s.sessions,
                esc(&util::fmt_duration(s.total_secs))
            );
        }
        o.push_str("</section>");
    }

    // Sleep and wake.
    if let Some(d) = input.diag {
        o.push_str("<section class=\"card\"><h2>Sleep and wake</h2>");
        if let Some(c) = &d.caps {
            for n in &c.notes {
                let _ = write!(o, "<p>{}</p>", esc(n));
            }
        }
        if let Some(w) = &d.last_wake {
            let _ = write!(o, "<p><b>Last wake:</b> {}</p>", esc(&w.plain));
        }
        match &d.requests {
            Some(r) if r.is_empty() => {
                o.push_str("<p><b>Blocking sleep:</b> nothing right now.</p>")
            }
            Some(r) => {
                o.push_str("<h3>What keeps the PC awake</h3><table><tr><th>Program or device</th><th>Effect</th><th>What it means</th></tr>");
                for q in r {
                    let _ = write!(
                        o,
                        "<tr><td>{}</td><td>{}</td><td>{}</td></tr>",
                        esc(&q.name),
                        esc(q.kind.label()),
                        esc(&q.plain)
                    );
                }
                o.push_str("</table>");
            }
            None => {}
        }
        if let Some(t) = &d.wake_timers {
            if t.is_empty() {
                o.push_str("<p><b>Wake timers:</b> none.</p>");
            } else {
                o.push_str("<h3>Wake timers</h3><ul>");
                for w in t {
                    let _ = write!(o, "<li>{}</li>", esc(&w.plain));
                }
                o.push_str("</ul>");
            }
        }
        if let Some(devs) = &d.wake_devices {
            let armed: Vec<&str> = devs
                .iter()
                .filter(|w| w.armed)
                .map(|w| w.name.as_str())
                .collect();
            let _ = write!(
                o,
                "<p><b>Devices allowed to wake the PC:</b> {}</p>",
                if armed.is_empty() {
                    "none".to_string()
                } else {
                    esc(&armed.join(", "))
                }
            );
        }
        if let Some(ev) = &d.events {
            if !ev.is_empty() {
                o.push_str("<h3>Recent sleep and wake events</h3><table><tr><th>Time</th><th>Event</th><th>Details</th></tr>");
                for e in ev.iter().take(40) {
                    let _ = write!(
                        o,
                        "<tr><td>{}</td><td>{}</td><td>{}</td></tr>",
                        esc(&util::fmt_datetime(e.t)),
                        esc(e.kind.label()),
                        esc(&e.text)
                    );
                }
                o.push_str("</table>");
            }
        }
        o.push_str("</section>");
    }

    // Energy findings.
    if let Some(e) = input.energy {
        let _ = write!(
            o,
            "<section class=\"card\"><h2>Energy check</h2><p class=\"muted\">{} problems, {} worth a look, {} for information.</p><table><tr><th></th><th>Finding</th><th>What it means</th><th>What to do</th></tr>",
            e.count(Severity::Error),
            e.count(Severity::Warning),
            e.count(Severity::Info)
        );
        for f in &e.findings {
            let class = match f.severity {
                Severity::Error => "e",
                Severity::Warning => "w",
                Severity::Info => "i",
            };
            let _ = write!(
                o,
                "<tr><td class=\"{class}\">{}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
                esc(f.severity.label()),
                esc(&f.title),
                esc(&f.plain),
                esc(&f.advice)
            );
        }
        o.push_str("</table></section>");
    }

    let _ = writeln!(
        o,
        "<footer>Generated by PowerForge {} on {}. No data left this PC.</footer></main></body></html>\n",
        env!("CARGO_PKG_VERSION"),
        esc(&util::fmt_datetime(input.now))
    );
    o
}

/// CSV of capacity history (local records merged with the report):
/// `date,full_mwh,design_mwh,health_pct,cycles,source`.
pub fn csv_capacity(input: &ExportInput) -> String {
    let mut rows: Vec<(i64, String)> = Vec::new();
    for r in &input.history.capacity {
        let h = health::health_percent(Some(r.design_mwh), Some(r.full_mwh));
        rows.push((
            r.t,
            format!(
                "{},{},{},{},{},recorded",
                util::fmt_date(r.t),
                r.full_mwh,
                r.design_mwh,
                h.map(|v| format!("{v:.1}")).unwrap_or_default(),
                r.cycles.map(|c| c.to_string()).unwrap_or_default()
            ),
        ));
    }
    if let Some(rep) = input.report {
        for p in &rep.history {
            let h = health::health_percent(p.design_mwh, p.full_mwh);
            rows.push((
                p.end,
                format!(
                    "{},{},{},{},{},battery report",
                    util::fmt_date(p.end),
                    p.full_mwh.map(|v| v.to_string()).unwrap_or_default(),
                    p.design_mwh.map(|v| v.to_string()).unwrap_or_default(),
                    h.map(|v| format!("{v:.1}")).unwrap_or_default(),
                    p.cycle_count.map(|c| c.to_string()).unwrap_or_default()
                ),
            ));
        }
    }
    rows.sort_by_key(|(t, _)| *t);
    let mut out = String::from("date,full_mwh,design_mwh,health_pct,cycles,source\n");
    for (_, r) in rows {
        out.push_str(&r);
        out.push('\n');
    }
    out
}

/// CSV of recorded charge samples: `time,percent,rate_w,plugged_in`.
pub fn csv_charge(history: &History) -> String {
    let mut out = String::from("time,percent,rate_w,plugged_in\n");
    for s in &history.charge {
        let _ = writeln!(
            out,
            "{},{:.1},{},{}",
            csv_field(&util::fmt_datetime(s.t)),
            s.percent,
            s.rate_mw
                .map(|r| format!("{:.2}", r as f64 / 1000.0))
                .unwrap_or_default(),
            if s.ac { "yes" } else { "no" }
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_html_report() {
        let now = 1_790_000_000;
        let battery = crate::battery::demo();
        let report = crate::batteryreport::demo();
        let history = crate::history::demo(now);
        let diag = crate::diag::demo(now);
        let energy = crate::energy::demo();
        let input = ExportInput {
            now,
            battery: Some(&battery),
            report: Some(&report),
            history: &history,
            diag: Some(&diag),
            energy: Some(&energy),
            plan: Some("Balanced <custom>"),
        };
        let h = html(&input);
        assert!(h.starts_with("<!doctype html>"));
        for part in [
            "Summary",
            "Battery health over time",
            "<svg",
            "Recent use",
            "Standby",
            "Sleep and wake",
            "Energy check",
            "No data left this PC",
        ] {
            assert!(h.contains(part), "missing {part}");
        }
        assert!(h.contains("Balanced &lt;custom&gt;"));
        assert!(!h.contains("<script") && !h.contains("http"));
        assert!(!h.contains("VEN_") && !h.contains("HDAUDIO"));
    }

    #[test]
    fn minimal_html_for_a_desktop() {
        let history = History::default();
        let input = ExportInput {
            now: 0,
            battery: None,
            report: None,
            history: &history,
            diag: None,
            energy: None,
            plan: None,
        };
        let h = html(&input);
        assert!(h.contains("No battery"));
        assert!(!h.contains("Battery health over time"));
        assert!(!h.contains("Energy check"));
    }

    #[test]
    fn csv_files() {
        let now = 1_790_000_000;
        let history = crate::history::demo(now);
        let report = crate::batteryreport::demo();
        let input = ExportInput {
            now,
            battery: None,
            report: Some(&report),
            history: &history,
            diag: None,
            energy: None,
            plan: None,
        };
        let c = csv_capacity(&input);
        assert!(c.starts_with("date,full_mwh,design_mwh,health_pct,cycles,source\n"));
        assert_eq!(c.lines().count(), 1 + 90 + 104);
        assert!(c.contains(",recorded") && c.contains(",battery report"));
        let ch = csv_charge(&history);
        assert!(ch.starts_with("time,percent,rate_w,plugged_in\n"));
        assert!(ch.lines().nth(1).unwrap().split(',').count() == 4);
    }
}
