#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use eframe::egui;
use powerforge::awake::{self, AwakeMode, AwakeRequest};
use powerforge::engine::{self, Engine, Shared, UiRequest};
use powerforge::history::History;
use powerforge::plans::{self, PowerBackend, SystemPlans};
use powerforge::settings::{self, Settings};
use powerforge::ui::{self, Options};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::Arc;

const USAGE: &str = "PowerForge: battery health, power plans, keep-awake and sleep diagnostics

  PowerForge.exe                   open the window (keeps running in the notification area)
  PowerForge.exe --background      start in the notification area only
                                   (what the start-with-Windows entry runs)
  PowerForge.exe --awake <time>    keep the PC awake: 30m, 2h, 1h30m, 18:30, on, off
  PowerForge.exe --plan <name>     switch to a power plan by name or GUID, then exit
  PowerForge.exe --plans           list the power plans, then exit
  PowerForge.exe --report [--out <file.html>] [--csv <file.csv>]
                                   write a battery and power report, then exit
  PowerForge.exe --quit            ask the running PowerForge to exit
  PowerForge.exe --demo            show an invented laptop; nothing is read or changed
  PowerForge.exe --screenshot <png> [--view <name>] [--light]
                                   render a page of the demo to a PNG and exit
  PowerForge.exe --selftest        run the logic against mock backends and sample data
  --data <folder>                  use this data folder instead of
                                   %LOCALAPPDATA%\\PowerForge (also: POWERFORGE_DATA)

PowerForge makes no network requests. Everything it records stays in its data folder.";

fn main() -> eframe::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // The administrator helper (`--elevated <job>`) runs one whitelisted command.
    if let Some(code) = powerforge::elevate::handle_cli(&args) {
        std::process::exit(code);
    }
    let has = |flag: &str| args.iter().any(|a| a == flag);
    let value = |flag: &str| {
        args.iter()
            .position(|a| a == flag)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    let console = || powerforge::sys::attach_parent_console();

    if has("--help") || has("-h") || has("/?") {
        console();
        println!("{USAGE}");
        return Ok(());
    }
    if has("--selftest") {
        console();
        match powerforge::selftest::run() {
            Ok(log) => {
                for line in log {
                    println!("{line}");
                }
                println!("SELFTEST PASSED");
                std::process::exit(0);
            }
            Err(e) => {
                println!("{e}");
                println!("SELFTEST FAILED");
                std::process::exit(1);
            }
        }
    }
    if has("--plans") {
        console();
        match SystemPlans.list() {
            Ok(list) => {
                for p in list {
                    println!(
                        "{} {}  {}",
                        if p.active { "*" } else { " " },
                        p.guid,
                        p.name
                    );
                }
            }
            Err(e) => {
                eprintln!("PowerForge: {e}");
                std::process::exit(2);
            }
        }
        return Ok(());
    }
    if let Some(name) = value("--plan") {
        console();
        let mut sys = SystemPlans;
        let code = match sys
            .list()
            .ok()
            .and_then(|list| plans::find_plan(&list, &name).map(|g| (g, list)))
        {
            Some((guid, list)) => match sys.set_active(guid) {
                Ok(()) => {
                    let n = list.iter().find(|p| p.guid == guid).map(|p| p.name.clone());
                    println!("Switched to {}", n.unwrap_or_default());
                    0
                }
                Err(e) => {
                    eprintln!("PowerForge: {e}");
                    2
                }
            },
            None => {
                eprintln!("PowerForge: no power plan called \"{name}\" (see --plans)");
                2
            }
        };
        std::process::exit(code);
    }
    let data_dir = value("--data")
        .map(PathBuf::from)
        .unwrap_or_else(settings::default_data_dir);

    if has("--report") {
        console();
        std::process::exit(report_command(&data_dir, value("--out"), value("--csv")));
    }
    if has("--quit") {
        engine::signal_running_instance(engine::SIGNAL_QUIT, 0);
        return Ok(());
    }

    // `--awake 30m` / `--awake off`.
    let mut awake_request = None;
    if let Some(text) = value("--awake") {
        let settings = Settings::load(&data_dir);
        let t = text.trim().to_lowercase();
        let (signal, secs, req) = if t == "off" || t == "stop" {
            (engine::SIGNAL_AWAKE_OFF, 0, None)
        } else if t == "on" || t == "forever" || t == "indefinitely" {
            (engine::SIGNAL_AWAKE, 0, Some(AwakeMode::Indefinite))
        } else if let Some(s) = awake::parse_duration(&t) {
            (
                engine::SIGNAL_AWAKE,
                s as isize,
                Some(AwakeMode::Until(powerforge::util::now() + s as i64)),
            )
        } else if let Some(at) = awake::parse_until(&t, powerforge::util::now()) {
            (
                engine::SIGNAL_AWAKE,
                (at - powerforge::util::now()) as isize,
                Some(AwakeMode::Until(at)),
            )
        } else {
            console();
            eprintln!("PowerForge: \"{text}\" is not a time (try 30m, 2h, 18:30, on or off)");
            std::process::exit(2);
        };
        if engine::signal_running_instance(signal, secs) {
            return Ok(());
        }
        let Some(mode) = req else {
            return Ok(());
        };
        awake_request = Some(AwakeRequest {
            mode,
            keep_display: settings.awake_display,
        });
    }

    // The demo and screenshots: invented data, no engine, no files.
    if has("--demo") || has("--screenshot") {
        let options = Options {
            demo: true,
            screenshot: value("--screenshot").map(PathBuf::from),
            view: value("--view"),
            light: has("--light"),
            ..Default::default()
        };
        return ui::run(options, window_icon());
    }

    // One engine per user session: a second start opens the first one's window.
    let Some(_instance) = powerforge::sys::single_instance("PowerForge.Instance") else {
        engine::signal_running_instance(engine::SIGNAL_OPEN, 0);
        return Ok(());
    };
    let settings = Settings::load(&data_dir);
    let shared = Arc::new(Shared::new(data_dir.clone(), History::load(&data_dir)));
    let background = has("--background") || awake_request.is_some();
    let Some(engine) = Engine::start(shared.clone(), settings, awake_request) else {
        rfd::MessageDialog::new()
            .set_title("PowerForge")
            .set_description("PowerForge could not start its background part.")
            .set_level(rfd::MessageLevel::Error)
            .show();
        std::process::exit(1);
    };
    let engine = Arc::new(engine);

    let mut open = !background;
    let mut page: Option<String> = value("--view");
    loop {
        if open {
            engine.drain_requests();
            let options = Options {
                view: page.take(),
                shared: Some(shared.clone()),
                control: Some(engine.clone()),
                ..Default::default()
            };
            let result = ui::run(options, window_icon());
            if result.is_err() || shared.quit.load(Ordering::SeqCst) || engine.drain_requests() {
                break;
            }
            if !Settings::load(&data_dir).close_to_tray {
                break;
            }
        }
        match engine.wait_request() {
            UiRequest::Open(view) => {
                open = true;
                page = view;
            }
            UiRequest::Quit => break,
        }
    }
    engine.stop();
    Ok(())
}

/// `--report`: battery report, local history and diagnostics that need no
/// administrator rights, written as HTML (and optionally CSV).
fn report_command(dir: &std::path::Path, out: Option<String>, csv: Option<String>) -> i32 {
    use powerforge::diag;
    let now = powerforge::util::now();
    let battery = powerforge::battery::read();
    let report = powerforge::batteryreport::generate().ok();
    let history = History::load(dir);
    let mut snapshot = diag::DiagSnapshot {
        last_wake: diag::query_lastwake().ok(),
        wake_devices: diag::query_wake_devices().ok(),
        caps: Some(diag::capabilities()),
        events: powerforge::events::read_recent(14, 200).ok(),
        ..Default::default()
    };
    snapshot.standby = report
        .as_ref()
        .and_then(powerforge::batteryreport::standby_summary);
    let plans = SystemPlans;
    let plan_name = plans.active().ok().and_then(|a| {
        plans
            .list()
            .ok()?
            .into_iter()
            .find(|p| p.guid == a)
            .map(|p| p.name)
    });
    let input = powerforge::export::ExportInput {
        now,
        battery: battery.as_ref(),
        report: report.as_ref(),
        history: &history,
        diag: Some(&snapshot),
        energy: None,
        plan: plan_name.as_deref(),
    };
    let path = out.map(PathBuf::from).unwrap_or_else(|| {
        PathBuf::from(format!(
            "PowerForge-report-{}.html",
            powerforge::util::fmt_date(now)
        ))
    });
    if let Err(e) = std::fs::write(&path, powerforge::export::html(&input)) {
        eprintln!("PowerForge: {}: {e}", path.display());
        return 2;
    }
    println!("Report written to {}", path.display());
    if let Some(csv) = csv {
        if let Err(e) = std::fs::write(&csv, powerforge::export::csv_capacity(&input)) {
            eprintln!("PowerForge: {csv}: {e}");
            return 2;
        }
        println!("Capacity history written to {csv}");
    }
    0
}

fn window_icon() -> egui::IconData {
    let bytes = include_bytes!(concat!(env!("OUT_DIR"), "/icon-256.png"));
    match image::load_from_memory(bytes) {
        Ok(img) => {
            let rgba = img.into_rgba8();
            let (width, height) = rgba.dimensions();
            egui::IconData {
                rgba: rgba.into_raw(),
                width,
                height,
            }
        }
        Err(_) => egui::IconData::default(),
    }
}
