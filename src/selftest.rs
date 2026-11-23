//! `--selftest`: runs the logic end to end against mock backends and
//! sample data, in a temporary folder. Changes nothing on the PC: plans are
//! an in-memory fake, keep-awake uses a recording executor.

use crate::alerts::{Alert, AlertSettings, AlertState};
use crate::autoswitch::{AutoSettings, Context, Rule, Switcher, Trigger};
use crate::awake::{Awake, AwakeMode, AwakeRequest, Executor, Inputs};
use crate::battery::{BatteryInfo, ChargeState};
use crate::plans::{Guid, MockPlans, PowerBackend, SettingKey};
use crate::{batteryreport, diag, energy, events, export, health, history, util};

struct Recorder(Vec<(bool, bool)>);

impl Executor for Recorder {
    fn apply(&mut self, system: bool, display: bool) {
        self.0.push((system, display));
    }
}

fn check(ok: bool, what: &str, log: &mut Vec<String>) -> Result<(), String> {
    if ok {
        log.push(format!("ok   {what}"));
        Ok(())
    } else {
        Err(format!("FAIL {what}"))
    }
}

/// Returns the log lines, or the first failure.
pub fn run() -> Result<Vec<String>, String> {
    let mut log = Vec::new();
    let now = util::now();

    // Battery report and health.
    let report = batteryreport::parse(include_str!("../tests/fixtures/battery-report-sample.xml"))?;
    check(
        report.batteries.len() == 1 && report.history.len() == 3,
        "battery report parses",
        &mut log,
    )?;
    let b = &report.batteries[0];
    let h = health::health_percent(b.design_mwh, b.full_mwh).unwrap_or(0.0);
    check(
        (h - 93.0).abs() < 0.01,
        "health = full / design (93 %)",
        &mut log,
    )?;
    check(
        health::grade(h) == health::Grade::Excellent,
        "health grade",
        &mut log,
    )?;
    let demo = batteryreport::demo();
    let points: Vec<health::HealthPoint> = demo
        .history
        .iter()
        .filter_map(|p| {
            Some(health::HealthPoint {
                t: p.end,
                health_pct: health::health_percent(p.design_mwh, p.full_mwh)?,
                cycles: p.cycle_count,
            })
        })
        .collect();
    let trend = health::wear_trend(&points).ok_or("no wear trend")?;
    check(
        trend.pct_per_year > 4.0 && trend.pct_per_year < 9.0,
        "wear trend over two years",
        &mut log,
    )?;
    check(
        batteryreport::standby_summary(&demo).is_some(),
        "standby summary",
        &mut log,
    )?;

    // History in a temporary folder.
    let dir = std::env::temp_dir().join(format!("PowerForge-selftest-{}", std::process::id()));
    let mut hist = history::History::default();
    let reading = crate::battery::demo();
    hist.record(&reading, now);
    hist.save(&dir).map_err(|e| e.to_string())?;
    let loaded = history::History::load(&dir);
    let _ = std::fs::remove_dir_all(&dir);
    check(
        loaded == hist && !dir.exists(),
        "history saves, loads, temp folder removed",
        &mut log,
    )?;

    // Alerts.
    let mut alerts = AlertState::default();
    let s = AlertSettings {
        limit_enabled: true,
        ..AlertSettings::default()
    };
    let at = |pct: f32, state: ChargeState| BatteryInfo {
        percent: pct,
        state,
        ac_online: state != ChargeState::Discharging,
        ..Default::default()
    };
    let fired = alerts.check(&s, &at(15.0, ChargeState::Discharging));
    let again = alerts.check(&s, &at(14.0, ChargeState::Discharging));
    let limit = alerts.check(&s, &at(80.0, ChargeState::Charging));
    check(
        fired == vec![Alert::Low(15)] && again.is_empty() && limit == vec![Alert::Limit(80)],
        "alerts fire once per crossing",
        &mut log,
    )?;

    // Power plans on the in-memory fake.
    let mut plans = MockPlans::new();
    let copy = plans
        .duplicate(Guid::BALANCED, "Selftest")
        .map_err(|e| e.to_string())?;
    plans
        .write(copy, SettingKey::SleepAfter, false, 600)
        .map_err(|e| e.to_string())?;
    plans.set_active(copy).map_err(|e| e.to_string())?;
    let blocked = plans.delete(copy).is_err();
    plans
        .set_active(Guid::BALANCED)
        .map_err(|e| e.to_string())?;
    plans.delete(copy).map_err(|e| e.to_string())?;
    check(
        blocked && plans.list().map(|l| l.len()).unwrap_or(0) == 3,
        "plans: duplicate, edit, switch, delete (mock)",
        &mut log,
    )?;
    check(
        Guid::parse(&Guid::BALANCED.to_string()) == Some(Guid::BALANCED),
        "plan GUIDs round-trip",
        &mut log,
    )?;

    // Automatic switching.
    let auto = AutoSettings {
        enabled: true,
        rules: vec![Rule {
            enabled: true,
            trigger: Trigger::OnBattery,
            plan: Guid::POWER_SAVER,
        }],
    };
    let mut sw = Switcher::new();
    let battery_ctx = Context {
        on_ac: Some(false),
        ..Context::default()
    };
    let ac_ctx = Context {
        on_ac: Some(true),
        ..Context::default()
    };
    let first = sw.step(&auto, &battery_ctx, &mut plans);
    sw.step(&auto, &ac_ctx, &mut plans);
    let back = sw.step(&auto, &ac_ctx, &mut plans);
    check(
        first.map(|s| s.to) == Some(Guid::POWER_SAVER)
            && back.map(|s| s.to) == Some(Guid::BALANCED),
        "auto-switch to Power saver on battery and back (mock)",
        &mut log,
    )?;

    // Keep awake with a recording executor.
    let mut rec = Recorder(Vec::new());
    let mut a = Awake::new();
    a.start(
        AwakeRequest {
            mode: AwakeMode::Until(now + 60),
            keep_display: true,
        },
        now,
        &mut rec,
    );
    let during = a.tick(
        &Inputs {
            now: now + 30,
            ..Inputs::default()
        },
        &mut rec,
    );
    let after = a.tick(
        &Inputs {
            now: now + 61,
            ..Inputs::default()
        },
        &mut rec,
    );
    check(
        during.active && !after.active && rec.0 == vec![(true, true), (false, false)],
        "keep-awake holds, then releases on time",
        &mut log,
    )?;
    check(
        crate::awake::parse_duration("1h30m") == Some(5400),
        "keep-awake durations parse",
        &mut log,
    )?;

    // Diagnostics parsers.
    let req = diag::parse_requests(include_str!("../tests/fixtures/requests-busy.txt"));
    check(
        req.len() == 5 && req.iter().all(|r| !r.plain.is_empty()),
        "power requests parse and explain",
        &mut log,
    )?;
    let wake = diag::parse_lastwake(include_str!("../tests/fixtures/lastwake-device.txt"));
    check(
        wake.sources.len() == 1 && !format!("{wake:?}").contains("VEN_"),
        "last wake parses, no instance path",
        &mut log,
    )?;
    let timers = diag::parse_waketimers(include_str!("../tests/fixtures/waketimers-one.txt"));
    check(timers.len() == 2, "wake timers parse", &mut log)?;
    let devs = diag::parse_devicequery(include_str!("../tests/fixtures/devicequery-armed.txt"));
    check(devs.len() == 3, "wake-armed devices parse", &mut log)?;
    let ev = events::parse(include_str!("../tests/fixtures/events-sample.xml"));
    check(ev.len() == 6, "sleep and wake events parse", &mut log)?;
    let en = energy::parse(include_str!("../tests/fixtures/energy-sample.xml"))?;
    check(en.findings.len() == 4, "energy report parses", &mut log)?;

    // Export.
    let d = diag::demo(now);
    let input = export::ExportInput {
        now,
        battery: Some(&reading),
        report: Some(&demo),
        history: &hist,
        diag: Some(&d),
        energy: Some(&en),
        plan: Some("Balanced"),
    };
    let html = export::html(&input);
    check(
        html.contains("Energy check") && !html.contains("<script"),
        "HTML report",
        &mut log,
    )?;
    check(
        export::csv_capacity(&input).lines().count() > 100,
        "CSV capacity history",
        &mut log,
    )?;
    log.push("Nothing on this PC was changed.".into());
    Ok(log)
}

#[cfg(test)]
mod tests {
    #[test]
    fn selftest_passes() {
        let log = super::run().unwrap();
        assert!(log.len() > 15);
    }
}
