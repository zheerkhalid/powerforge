//! Application state and the window frame: header, navigation rail,
//! toasts, background jobs, screenshots, and the plumbing between the pages
//! and the engine.

use super::theme::{self, pal, pal_ctx, regular, semibold, Icon};
use super::widgets;
use super::Options;
use crate::awake::{Awake, AwakeMode, AwakeRequest, Executor};
use crate::batteryreport::BatteryReport;
use crate::diag::DiagSnapshot;
use crate::energy::EnergyReport;
use crate::engine::{Cmd, Control, LiveState, RatePoint, Shared};
use crate::history::History;
use crate::impact::AppImpact;
use crate::plans::{AcDc, Guid, Plan, PowerBackend, SettingKey, SystemPlans};
use crate::settings::Settings;
use crate::util;
use eframe::egui::{
    self, vec2, Align, Align2, Color32, CornerRadius, Frame, Layout, Margin, Rect, RichText, Sense,
    Stroke, TextureHandle, Ui,
};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;
use std::time::Instant;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    Overview,
    Health,
    Plans,
    Auto,
    Awake,
    Sleep,
    Energy,
    Apps,
    Settings,
}

/// A background job whose result arrives later.
pub enum Job<T> {
    Idle,
    Running(Receiver<Result<T, String>>),
    Done(T),
    Failed(String),
}

impl<T: Send + 'static> Job<T> {
    pub fn start(
        ctx: &egui::Context,
        f: impl FnOnce() -> Result<T, String> + Send + 'static,
    ) -> Job<T> {
        let (tx, rx) = channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(f());
            ctx.request_repaint();
        });
        Job::Running(rx)
    }

    /// Picks up a finished result. Returns true when it changed.
    pub fn poll(&mut self) -> bool {
        if let Job::Running(rx) = self {
            match rx.try_recv() {
                Ok(Ok(v)) => *self = Job::Done(v),
                Ok(Err(e)) => *self = Job::Failed(e),
                Err(std::sync::mpsc::TryRecvError::Empty) => return false,
                Err(_) => *self = Job::Failed("the task stopped unexpectedly".into()),
            }
            return true;
        }
        false
    }

    pub fn value(&self) -> Option<&T> {
        match self {
            Job::Done(v) => Some(v),
            _ => None,
        }
    }

    pub fn running(&self) -> bool {
        matches!(self, Job::Running(..))
    }

    pub fn is_idle(&self) -> bool {
        matches!(self, Job::Idle)
    }
}

/// Does nothing: the demo's keep-awake changes no system state.
pub struct NoExec;

impl Executor for NoExec {
    fn apply(&mut self, _system: bool, _display: bool) {}
}

pub struct Toast {
    pub text: String,
    pub error: bool,
    pub born: Instant,
}

struct Shot {
    path: PathBuf,
    frames: u32,
    requested: bool,
}

/// Dialogs over the window.
pub enum Dialog {
    Rename(Guid, String),
    Duplicate(Guid, String),
    Delete(Guid, String),
    Restore(Guid, String),
    /// Elevated action confirmation: (title, text, action).
    Admin(String, String, AdminAction),
}

#[derive(Clone, Debug)]
pub enum AdminAction {
    Requests,
    WakeTimers,
    DeviceWake(String, bool),
    Hibernate(bool),
    Energy,
}

/// The state of the keep-awake page's form.
pub struct AwakeForm {
    pub minutes: String,
    pub until: String,
    pub app: String,
    pub net_kbps: String,
    pub disk_kbps: String,
}

/// Diagnostics jobs.
#[derive(Default)]
pub struct DiagJobs {
    pub snapshot: DiagSnapshot,
    pub lastwake: Option<Job<crate::diag::LastWake>>,
    pub devices: Option<Job<Vec<crate::diag::WakeDevice>>>,
    pub events: Option<Job<Vec<crate::events::PowerEvent>>>,
    pub requests: Option<Job<Vec<crate::diag::PowerRequest>>>,
    pub timers: Option<Job<Vec<crate::diag::WakeTimer>>>,
    pub action: Option<Job<String>>,
    pub started: bool,
}

pub struct EnergyState {
    pub job: Job<EnergyReport>,
    pub progress: Arc<std::sync::Mutex<f32>>,
    pub duration: u32,
    pub expanded: Option<usize>,
    /// Demo: a pretend run.
    pub demo_started: Option<Instant>,
}

pub struct ImpactState {
    pub rx: Option<Receiver<Vec<AppImpact>>>,
    pub stop: Arc<AtomicBool>,
    pub apps: Vec<AppImpact>,
    pub updated: Option<Instant>,
}

pub struct App {
    pub(super) demo: bool,
    pub(super) shared: Arc<Shared>,
    control: Option<Arc<dyn Control>>,
    pub(super) settings: Settings,
    saved_settings: Settings,
    pub(super) page: Page,
    pub(super) live: LiveState,
    pub(super) plans: Box<dyn PowerBackend>,
    pub(super) plan_list: Vec<Plan>,
    pub(super) plan_sel: Option<Guid>,
    pub(super) plan_values: HashMap<SettingKey, Option<AcDc>>,
    pub(super) report: Job<BatteryReport>,
    pub(super) diag: DiagJobs,
    pub(super) energy: EnergyState,
    pub(super) impact: ImpactState,
    pub(super) awake_form: AwakeForm,
    pub(super) demo_awake: Awake,
    pub(super) dialog: Option<Dialog>,
    pub(super) rule_form: super::auto_page::RuleForm,
    pub(super) about_open: bool,
    pub(super) toasts: Vec<Toast>,
    pub(super) logo: Option<TextureHandle>,
    pub(super) startup: bool,
    shot: Option<Shot>,
}

/// Opens the window and returns when it is closed.
pub fn run(options: Options, icon: egui::IconData) -> eframe::Result<()> {
    let viewport = egui::ViewportBuilder::default()
        .with_title(if options.demo {
            "PowerForge (demo)"
        } else {
            "PowerForge"
        })
        .with_inner_size([1280.0, 800.0])
        .with_min_inner_size([1040.0, 640.0])
        .with_icon(icon.clone());
    let native = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    eframe::run_native(
        "PowerForge",
        native,
        Box::new(move |cc| Ok(Box::new(App::new(cc, options, icon)))),
    )
}

/// An invented few hours of the live graph for the demo.
fn demo_rate(now: i64) -> std::collections::VecDeque<RatePoint> {
    let mut out = std::collections::VecDeque::new();
    let start = now - 3 * 3600;
    let mut pct = 92.0f32;
    let mut t = start;
    while t <= now {
        let i = ((t - start) / 60) as f32;
        let charging = (t - start) < 25 * 60;
        let watts = if charging {
            if i < 18.0 {
                27.5 + (i * 0.7).sin() * 0.6
            } else {
                27.5 - (i - 18.0) * 3.2
            }
        } else {
            -(8.6
                + (i * 0.37).sin() * 1.6
                + (i * 0.11).cos() * 2.1
                + if (90.0..105.0).contains(&i) { 9.0 } else { 0.0 })
        };
        pct = (pct + watts / 48.72 * 100.0 / 60.0).clamp(0.0, 100.0);
        out.push_back(RatePoint {
            t,
            watts,
            percent: pct,
        });
        t += 60;
    }
    // End on the demo battery's charge.
    let last = out.back().map(|p| p.percent).unwrap_or(64.0);
    let shift = 64.0 - last;
    for p in out.iter_mut() {
        p.percent = (p.percent + shift).clamp(0.0, 100.0);
    }
    out
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>, options: Options, icon: egui::IconData) -> Self {
        let ctx = &cc.egui_ctx;
        theme::install(ctx);
        let demo = options.demo;
        let now = util::now();
        let (shared, mut settings, plans): (Arc<Shared>, Settings, Box<dyn PowerBackend>) = if demo
        {
            let shared = Arc::new(Shared::new(
                PathBuf::from("C:\\Users\\Alex\\AppData\\Local\\PowerForge"),
                crate::history::demo(now),
            ));
            let mut s = Settings {
                theme: crate::settings::ThemeChoice::Dark,
                ..Settings::default()
            };
            s.alerts.limit_enabled = true;
            s.alerts.plug_enabled = true;
            s.auto.enabled = true;
            s.auto.rules = vec![
                crate::autoswitch::Rule {
                    enabled: true,
                    trigger: crate::autoswitch::Trigger::OnBattery,
                    plan: Guid::POWER_SAVER,
                },
                crate::autoswitch::Rule {
                    enabled: true,
                    trigger: crate::autoswitch::Trigger::PluggedIn,
                    plan: Guid::BALANCED,
                },
                crate::autoswitch::Rule {
                    enabled: true,
                    trigger: crate::autoswitch::Trigger::AppRunning("blender.exe".into()),
                    plan: Guid::HIGH_PERFORMANCE,
                },
                crate::autoswitch::Rule {
                    enabled: false,
                    trigger: crate::autoswitch::Trigger::AppForeground("game.exe".into()),
                    plan: Guid::HIGH_PERFORMANCE,
                },
            ];
            let mut plans = crate::plans::demo();
            // On battery with an "On battery: Power saver" rule.
            let _ = plans.set_active(Guid::POWER_SAVER);
            (shared, s, Box::new(plans))
        } else {
            let shared = options.shared.clone().unwrap_or_else(|| {
                let dir = crate::settings::default_data_dir();
                Arc::new(Shared::new(dir.clone(), History::load(&dir)))
            });
            let s = Settings::load(&shared.dir);
            (shared, s, Box::new(SystemPlans))
        };
        if options.light {
            settings.theme = crate::settings::ThemeChoice::Light;
        }
        theme::apply_choice(ctx, settings.theme);

        let wake_ctx = ctx.clone();
        if let Ok(mut waker) = shared.waker.lock() {
            *waker = Some(Box::new(move || wake_ctx.request_repaint()));
        }
        let logo = (icon.width > 0).then(|| {
            let image = egui::ColorImage::from_rgba_unmultiplied(
                [icon.width as usize, icon.height as usize],
                &icon.rgba,
            );
            ctx.load_texture("logo", image, egui::TextureOptions::LINEAR)
        });
        let mut demo_awake = Awake::new();
        if demo {
            demo_awake.start(
                AwakeRequest {
                    mode: AwakeMode::Until(now + 25 * 60 + 30),
                    keep_display: true,
                },
                now - 35 * 60,
                &mut NoExec,
            );
        }
        let mut app = App {
            demo,
            shared,
            control: options.control.clone(),
            saved_settings: settings.clone(),
            awake_form: AwakeForm {
                minutes: settings.awake_minutes.to_string(),
                until: "18:30".into(),
                app: String::new(),
                net_kbps: settings.activity_net_kbps.to_string(),
                disk_kbps: settings.activity_disk_kbps.to_string(),
            },
            settings,
            page: Page::Overview,
            live: LiveState::default(),
            plans,
            plan_list: Vec::new(),
            plan_sel: None,
            plan_values: HashMap::new(),
            report: Job::Idle,
            diag: DiagJobs::default(),
            energy: EnergyState {
                job: Job::Idle,
                progress: Arc::new(std::sync::Mutex::new(0.0)),
                duration: 60,
                expanded: None,
                demo_started: None,
            },
            impact: ImpactState {
                rx: None,
                stop: Arc::new(AtomicBool::new(false)),
                apps: Vec::new(),
                updated: None,
            },
            demo_awake,
            dialog: None,
            rule_form: Default::default(),
            about_open: false,
            toasts: Vec::new(),
            logo,
            startup: if demo {
                true
            } else {
                crate::sys::startup_enabled(crate::sys::RUN_VALUE)
            },
            shot: options.screenshot.clone().map(|path| Shot {
                path,
                frames: 0,
                requested: false,
            }),
        };
        if demo {
            app.report = Job::Done(crate::batteryreport::demo());
            app.diag.snapshot = crate::diag::demo(now);
            app.diag.started = true;
            app.energy.job = Job::Done(crate::energy::demo());
            app.impact.apps = crate::impact::demo();
            app.impact.updated = Some(Instant::now());
            app.awake_form.app = "backup.exe".into();
        }
        app.refresh_live();
        app.refresh_plans();
        app.apply_view(options.view.as_deref().unwrap_or("overview"), ctx);
        app
    }

    pub(super) fn apply_view(&mut self, view: &str, ctx: &egui::Context) {
        self.page = match view {
            "health" | "battery" => Page::Health,
            "plans" => Page::Plans,
            "auto" => Page::Auto,
            "awake" => Page::Awake,
            "sleep" => Page::Sleep,
            "energy" => Page::Energy,
            "energy-run" => {
                if self.demo {
                    self.energy.job = Job::Idle;
                    self.energy.demo_started =
                        Some(Instant::now() - std::time::Duration::from_secs(37));
                }
                Page::Energy
            }
            "apps" => Page::Apps,
            "settings" => Page::Settings,
            "about" => {
                self.about_open = true;
                Page::Overview
            }
            "rename" => {
                if let Some(p) = self.plan_list.iter().find(|p| p.name == "Quiet work") {
                    self.plan_sel = Some(p.guid);
                    self.dialog = Some(Dialog::Rename(p.guid, "Quiet work".into()));
                }
                Page::Plans
            }
            _ => Page::Overview,
        };
        if self.page == Page::Plans {
            if let Some(p) = self.plan_list.iter().find(|p| p.name == "Quiet work") {
                self.plan_sel = Some(p.guid);
                self.load_plan_values();
            }
        }
        let _ = ctx;
    }

    // ------------------------------------------------------------------------------
    // Data
    // ------------------------------------------------------------------------------

    pub(super) fn now(&self) -> i64 {
        util::now()
    }

    fn refresh_live(&mut self) {
        if self.demo {
            let now = self.now();
            if self.live.rate.is_empty() {
                self.live.rate = demo_rate(now);
            }
            self.live.battery = Some(crate::battery::demo());
            self.live.on_ac = Some(false);
            self.live.awake = self.demo_awake.tick(
                &crate::awake::Inputs {
                    now,
                    ..Default::default()
                },
                &mut NoExec,
            );
            self.live.active_plan = self.plans.active().ok();
            self.live.active_plan_name = self
                .plan_list
                .iter()
                .find(|p| Some(p.guid) == self.live.active_plan)
                .map(|p| p.name.clone())
                .unwrap_or_else(|| "Balanced".into());
            self.live.last_switch = Some((now - 47 * 60, "Power saver: Running on battery".into()));
        } else {
            self.live = self.shared.live();
        }
    }

    pub(super) fn refresh_plans(&mut self) {
        match self.plans.list() {
            Ok(list) => {
                self.plan_list = list;
                if self
                    .plan_sel
                    .is_none_or(|g| !self.plan_list.iter().any(|p| p.guid == g))
                {
                    self.plan_sel = self.plan_list.iter().find(|p| p.active).map(|p| p.guid);
                }
            }
            Err(e) => self.toast(format!("Power plans could not be read: {e}"), true),
        }
        self.load_plan_values();
        if self.demo {
            self.live.active_plan = self.plans.active().ok();
        }
    }

    pub(super) fn load_plan_values(&mut self) {
        self.plan_values.clear();
        if let Some(g) = self.plan_sel {
            for k in SettingKey::ALL {
                self.plan_values.insert(k, self.plans.read(g, k).ok());
            }
        }
    }

    pub(super) fn plan_name(&self, g: Guid) -> String {
        self.plan_list
            .iter()
            .find(|p| p.guid == g)
            .map(|p| p.name.clone())
            .unwrap_or_else(|| "a deleted plan".into())
    }

    /// Makes a plan active and tells the engine.
    pub(super) fn activate_plan(&mut self, g: Guid) {
        match self.plans.set_active(g) {
            Ok(()) => {
                self.send(Cmd::PlanChanged);
                self.refresh_plans();
                self.toast(format!("Now using {}", self.plan_name(g)), false);
            }
            Err(e) => self.toast(format!("Could not switch: {e}"), true),
        }
    }

    pub(super) fn toast(&mut self, text: impl Into<String>, error: bool) {
        self.toasts.push(Toast {
            text: text.into(),
            error,
            born: Instant::now(),
        });
    }

    pub(super) fn send(&mut self, cmd: Cmd) {
        if self.demo {
            let now = self.now();
            match cmd {
                Cmd::StartAwake(req) => self.demo_awake.start(req, now, &mut NoExec),
                Cmd::StopAwake => self.demo_awake.stop(&mut NoExec),
                _ => {}
            }
            self.refresh_live();
            return;
        }
        if let Some(c) = &self.control {
            c.send(cmd);
        }
    }

    pub(super) fn start_awake(&mut self, mode: AwakeMode) {
        let req = AwakeRequest {
            mode,
            keep_display: self.settings.awake_display,
        };
        self.send(Cmd::StartAwake(req));
        if !self.demo && self.control.is_none() {
            self.toast(
                "Keep awake needs PowerForge running normally (not from --demo).",
                true,
            );
        }
    }

    /// Saves settings when they changed and tells the engine.
    fn commit_settings(&mut self, ctx: &egui::Context) {
        if self.settings == self.saved_settings {
            return;
        }
        if self.settings.theme != self.saved_settings.theme {
            theme::apply_choice(ctx, self.settings.theme);
        }
        self.saved_settings = self.settings.clone();
        if self.demo {
            return;
        }
        if let Err(e) = self.settings.save(&self.shared.dir) {
            self.toast(format!("Settings could not be saved: {e}"), true);
        }
        self.send(Cmd::Settings(Box::new(self.settings.clone())));
    }

    pub(super) fn history(&self) -> History {
        self.shared
            .history
            .lock()
            .map(|h| h.clone())
            .unwrap_or_default()
    }

    /// Starts the battery report the first time a page needs it.
    pub(super) fn ensure_report(&mut self, ctx: &egui::Context) {
        if self.report.is_idle() && !self.demo {
            self.report = Job::start(ctx, crate::batteryreport::generate);
        }
    }

    fn poll_jobs(&mut self) {
        if self.report.poll() {
            if let Some(r) = self.report.value() {
                self.diag.snapshot.standby = crate::batteryreport::standby_summary(r);
            }
        }
        self.poll_diag();
        if self.energy.job.poll() {
            match &self.energy.job {
                Job::Done(_) => self.toast("The energy check is finished.", false),
                Job::Failed(e) => {
                    let e = e.clone();
                    self.toast(format!("The energy check did not run: {e}"), true)
                }
                _ => {}
            }
        }
        if let Some(rx) = &self.impact.rx {
            let mut latest = None;
            while let Ok(apps) = rx.try_recv() {
                latest = Some(apps);
            }
            if let Some(apps) = latest {
                if !apps.is_empty() || self.impact.updated.is_none() {
                    self.impact.apps = apps;
                    self.impact.updated = Some(Instant::now());
                }
            }
        }
    }

    /// Runs the impact sampler on a worker while the App power page is open.
    pub(super) fn ensure_impact(&mut self, ctx: &egui::Context) {
        if self.demo || self.impact.rx.is_some() {
            return;
        }
        let (tx, rx) = channel();
        let stop = Arc::new(AtomicBool::new(false));
        let stop2 = stop.clone();
        let ctx = ctx.clone();
        let shared = self.shared.clone();
        std::thread::spawn(move || {
            let mut sampler = crate::impact::Sampler::new();
            while !stop2.load(Ordering::Relaxed) {
                let watts = shared
                    .live()
                    .battery
                    .and_then(|b| b.rate_mw)
                    .filter(|r| *r < -500)
                    .map(|r| -r as f32 / 1000.0)
                    .unwrap_or(crate::impact::TYPICAL_WATTS);
                let apps = sampler.sample(watts);
                if tx.send(apps).is_err() {
                    break;
                }
                ctx.request_repaint();
                for _ in 0..30 {
                    if stop2.load(Ordering::Relaxed) {
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
            }
        });
        self.impact.rx = Some(rx);
        self.impact.stop = stop;
    }

    fn stop_impact(&mut self) {
        if self.impact.rx.take().is_some() {
            self.impact.stop.store(true, Ordering::Relaxed);
        }
    }

    // ------------------------------------------------------------------------------
    // Frame
    // ------------------------------------------------------------------------------

    fn handle_requests(&mut self, ctx: &egui::Context) {
        if self.shared.quit.load(Ordering::SeqCst) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        let show = self.shared.show.lock().ok().and_then(|mut s| s.take());
        if let Some(page) = show {
            if !page.is_empty() {
                self.apply_view(&page, ctx);
            }
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        }
    }

    fn handle_screenshot(&mut self, ctx: &egui::Context) {
        let Some(shot) = &mut self.shot else { return };
        ctx.request_repaint();
        shot.frames += 1;
        if shot.frames == 1 {
            ctx.set_pixels_per_point(1.0);
        }
        if shot.frames == 3 {
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(vec2(1280.0, 800.0)));
        }
        let image = ctx.input(|i| {
            i.events.iter().find_map(|e| match e {
                egui::Event::Screenshot { image, .. } => Some(image.clone()),
                _ => None,
            })
        });
        if let Some(image) = image {
            let [w, h] = image.size;
            if let Some(parent) = shot.path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if let Err(e) = image::save_buffer(
                &shot.path,
                image.as_raw(),
                w as u32,
                h as u32,
                image::ColorType::Rgba8,
            ) {
                eprintln!("screenshot failed: {e}");
            }
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            self.shot = None;
            return;
        }
        if !shot.requested && shot.frames > 40 {
            shot.requested = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::default()));
        }
    }

    fn pill(ui: &mut Ui, color: Color32, text: &str, tooltip: &str) -> egui::Response {
        let galley = widgets::elided(ui, text, semibold(12.5), color, 260.0);
        let size = vec2(galley.size().x + 34.0, 26.0);
        let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
        ui.painter()
            .rect_filled(rect, CornerRadius::same(13), color.gamma_multiply(0.14));
        ui.painter()
            .circle_filled(rect.left_center() + vec2(13.0, 0.0), 4.0, color);
        ui.painter().galley(
            egui::pos2(rect.left() + 23.0, rect.center().y - galley.size().y / 2.0),
            galley,
            color,
        );
        resp.on_hover_text(tooltip)
    }

    fn header(&mut self, ctx: &egui::Context) {
        let p = pal_ctx(ctx);
        egui::TopBottomPanel::top("header")
            .exact_height(64.0)
            .frame(
                Frame::new()
                    .fill(p.rail)
                    .inner_margin(Margin::symmetric(18, 0)),
            )
            .show(ctx, |ui| {
                let full = ui.max_rect();
                ui.painter().hline(
                    full.x_range().expand(18.0),
                    full.bottom(),
                    Stroke::new(1.0_f32, p.border),
                );
                ui.horizontal_centered(|ui| {
                    let (rect, _) = ui.allocate_exact_size(vec2(32.0, 32.0), Sense::hover());
                    if let Some(logo) = &self.logo {
                        ui.painter().image(
                            logo.id(),
                            rect,
                            Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                            Color32::WHITE,
                        );
                    }
                    ui.add_space(2.0);
                    ui.label(RichText::new("PowerForge").font(semibold(19.0)));
                    if self.demo {
                        widgets::badge(ui, "DEMO", p.info).on_hover_text(
                            "An invented laptop. Nothing on this PC is read or changed.",
                        );
                    }
                    ui.add_space(190.0 - ui.min_rect().width().min(190.0));
                    // Power source.
                    let (color, text) = match &self.live.battery {
                        Some(b) => {
                            let color = if b.ac_online {
                                p.info
                            } else if b.percent <= self.settings.alerts.low_pct as f32 {
                                p.danger
                            } else {
                                p.good
                            };
                            (
                                color,
                                format!("{:.0} %  \u{b7}  {}", b.percent, b.state.label()),
                            )
                        }
                        None => (p.info, "Mains power (no battery)".to_string()),
                    };
                    if Self::pill(
                        ui,
                        color,
                        &text,
                        "Charge and power source. Click for details.",
                    )
                    .clicked()
                    {
                        self.page = Page::Overview;
                    }
                    ui.add_space(4.0);
                    let plan = if self.live.active_plan_name.is_empty() {
                        "Power plan".to_string()
                    } else {
                        self.live.active_plan_name.clone()
                    };
                    if Self::pill(
                        ui,
                        p.accent,
                        &plan,
                        "The active power plan. Click to change it.",
                    )
                    .clicked()
                    {
                        self.page = Page::Plans;
                    }
                    ui.add_space(4.0);
                    if self.live.awake.active
                        && Self::pill(ui, p.warn, "Keeping awake", &self.live.awake.text.clone())
                            .clicked()
                    {
                        self.page = Page::Awake;
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if widgets::icon_button(ui, Icon::INFO, "About PowerForge").clicked() {
                            self.about_open = true;
                        }
                        let awake = self.live.awake.active;
                        let label = if awake {
                            "Stop keeping awake"
                        } else {
                            "Keep awake"
                        };
                        let b = if awake {
                            widgets::ghost_button(ui, &Icon::MOON.with(label))
                        } else {
                            widgets::ghost_button(ui, &Icon::SUNNY.with(label))
                        };
                        let tip = format!(
                            "Stops the PC from going to sleep on its own until you turn it off.{}",
                            crate::tray::parse_hotkey(&self.settings.hotkey)
                                .map(|_| format!(" Hotkey: {}.", self.settings.hotkey))
                                .unwrap_or_default()
                        );
                        if b.on_hover_text(tip).clicked() {
                            if awake {
                                self.send(Cmd::StopAwake);
                            } else {
                                self.start_awake(AwakeMode::Indefinite);
                            }
                        }
                    });
                });
            });
    }

    fn rail(&mut self, ctx: &egui::Context) {
        let p = pal_ctx(ctx);
        egui::SidePanel::left("rail")
            .exact_width(208.0)
            .resizable(false)
            .frame(Frame::new().fill(p.rail).inner_margin(Margin::symmetric(10, 14)))
            .show(ctx, |ui| {
                let full = ui.max_rect();
                ui.painter().vline(
                    full.right() + 10.0,
                    full.y_range().expand(14.0),
                    Stroke::new(1.0_f32, p.border),
                );
                ui.spacing_mut().item_spacing.y = 4.0;
                let items: [(Page, Icon, &str); 9] = [
                    (Page::Overview, Icon::HOME, "Overview"),
                    (Page::Health, Icon::HEART, "Battery health"),
                    (Page::Plans, Icon::SLIDERS, "Power plans"),
                    (Page::Auto, Icon::REFRESH, "Auto switch"),
                    (Page::Awake, Icon::SUNNY, "Keep awake"),
                    (Page::Sleep, Icon::MOON, "Sleep & wake"),
                    (Page::Energy, Icon::LEAF, "Energy check"),
                    (Page::Apps, Icon::APPS, "App power"),
                    (Page::Settings, Icon::SETTINGS, "Settings"),
                ];
                for (page, icon, label) in items {
                    if matches!(page, Page::Awake | Page::Settings) {
                        ui.add_space(8.0);
                        widgets::divider(ui);
                        ui.add_space(8.0);
                    }
                    let active = self.page == page;
                    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 40.0), Sense::click());
                    if active {
                        ui.painter().rect_filled(rect, CornerRadius::same(9), p.selected);
                        let bar = Rect::from_min_size(rect.left_center() - vec2(0.0, 9.0), vec2(3.0, 18.0));
                        ui.painter().rect_filled(bar, CornerRadius::same(2), p.accent);
                    } else if resp.hovered() {
                        ui.painter().rect_filled(rect, CornerRadius::same(9), p.hover);
                    }
                    let fg = if active { p.text } else { p.muted };
                    ui.painter().text(
                        rect.left_center() + vec2(24.0, 0.0),
                        Align2::CENTER_CENTER,
                        icon.glyph(),
                        regular(16.0),
                        if active { p.accent } else { fg },
                    );
                    ui.painter().text(
                        rect.left_center() + vec2(46.0, -1.0),
                        Align2::LEFT_CENTER,
                        label,
                        if active { semibold(14.5) } else { regular(14.5) },
                        fg,
                    );
                    if resp.clicked() {
                        self.page = page;
                    }
                }
                ui.with_layout(Layout::bottom_up(Align::Min), |ui| {
                    ui.add_space(2.0);
                    ui.horizontal(|ui| {
                        ui.add_space(12.0);
                        ui.label(
                            RichText::new(format!("Version {}", env!("CARGO_PKG_VERSION")))
                                .size(12.0)
                                .color(p.faint),
                        );
                    });
                    ui.horizontal(|ui| {
                        ui.add_space(12.0);
                        ui.label(RichText::new(Icon::LOCK.with("Nothing leaves this PC")).size(12.0).color(p.good))
                            .on_hover_text("PowerForge makes no network requests. What it records stays in a folder on this PC.");
                    });
                });
            });
    }

    fn about_dialog(&mut self, ctx: &egui::Context) {
        if !self.about_open {
            return;
        }
        let p = pal_ctx(ctx);
        let mut close = false;
        let modal = egui::Modal::new(egui::Id::new("about"))
            .frame(
                Frame::new()
                    .fill(p.card)
                    .stroke(Stroke::new(1.0_f32, p.border))
                    .corner_radius(CornerRadius::same(16))
                    .inner_margin(Margin::same(28)),
            )
            .show(ctx, |ui| {
                ui.set_width(400.0);
                ui.vertical_centered(|ui| {
                    let (rect, _) = ui.allocate_exact_size(vec2(84.0, 84.0), Sense::hover());
                    if let Some(logo) = &self.logo {
                        ui.painter().image(
                            logo.id(),
                            rect,
                            Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                            Color32::WHITE,
                        );
                    }
                    ui.add_space(6.0);
                    ui.label(RichText::new("PowerForge").font(semibold(26.0)));
                    ui.label(RichText::new(format!("Version {}", env!("CARGO_PKG_VERSION"))).color(p.muted));
                    ui.add_space(10.0);
                    ui.label("Battery health, power plans, keep-awake and sleep diagnostics, in plain language.");
                    ui.add_space(10.0);
                    ui.label(RichText::new("Developed by Zheer Khalid").font(semibold(15.0)));
                    ui.add_space(2.0);
                    ui.hyperlink_to(RichText::new("zheer.dev").color(p.accent), "https://zheer.dev");
                    ui.add_space(10.0);
                    ui.label(
                        RichText::new("Runs with your normal rights. A few checks ask for administrator rights, only when you start them.")
                            .size(12.0)
                            .color(p.faint),
                    );
                    ui.label(RichText::new("No telemetry, no network requests. Free and open source under the MIT License.").size(12.0).color(p.faint));
                    ui.add_space(14.0);
                    if widgets::primary_button(ui, "Close", true).clicked() {
                        close = true;
                    }
                });
            });
        if close || modal.should_close() {
            self.about_open = false;
        }
    }

    fn toasts_ui(&mut self, ctx: &egui::Context) {
        self.toasts
            .retain(|t| t.born.elapsed().as_secs_f32() < if t.error { 8.0 } else { 3.5 });
        if self.toasts.is_empty() {
            return;
        }
        ctx.request_repaint_after(std::time::Duration::from_millis(250));
        let p = pal_ctx(ctx);
        egui::Area::new(egui::Id::new("toasts"))
            .anchor(Align2::RIGHT_BOTTOM, vec2(-18.0, -18.0))
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                ui.with_layout(Layout::bottom_up(Align::Max), |ui| {
                    for t in self.toasts.iter().rev().take(4) {
                        let color = if t.error { p.danger } else { p.good };
                        Frame::new()
                            .fill(p.card)
                            .stroke(Stroke::new(1.0_f32, color.gamma_multiply(0.7)))
                            .corner_radius(CornerRadius::same(10))
                            .inner_margin(Margin::symmetric(14, 10))
                            .show(ui, |ui| {
                                ui.set_max_width(420.0);
                                ui.horizontal(|ui| {
                                    let icon = if t.error { Icon::ERROR } else { Icon::DONE };
                                    ui.label(RichText::new(icon.glyph()).color(color));
                                    ui.add(egui::Label::new(t.text.as_str()).wrap());
                                });
                            });
                    }
                });
            });
    }
}

/// The standard page header: title, subtitle and right-aligned actions.
pub fn page_header(ui: &mut Ui, title: &str, subtitle: &str, actions: impl FnOnce(&mut Ui)) {
    let p = pal(ui);
    ui.allocate_ui_with_layout(
        vec2(ui.available_width(), 56.0),
        Layout::left_to_right(Align::Center),
        |ui| {
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                ui.label(RichText::new(title).font(semibold(24.0)));
                ui.label(RichText::new(subtitle).color(p.muted));
            });
            ui.with_layout(Layout::right_to_left(Align::Center), actions);
        },
    );
    ui.add_space(6.0);
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.handle_requests(ctx);
        self.handle_screenshot(ctx);
        self.refresh_live();
        self.poll_jobs();
        if self.page != Page::Apps {
            self.stop_impact();
        }

        self.header(ctx);
        self.rail(ctx);
        let p = pal_ctx(ctx);
        egui::CentralPanel::default()
            .frame(
                Frame::new()
                    .fill(p.bg)
                    .inner_margin(Margin::symmetric(24, 18)),
            )
            .show(ctx, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt(("page", self.page as u8))
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width() - 6.0);
                        match self.page {
                            Page::Overview => self.overview_ui(ui),
                            Page::Health => self.health_ui(ui),
                            Page::Plans => self.plans_ui(ui),
                            Page::Auto => self.auto_ui(ui),
                            Page::Awake => self.awake_ui(ui),
                            Page::Sleep => self.sleep_ui(ui),
                            Page::Energy => self.energy_ui(ui),
                            Page::Apps => self.apps_ui(ui),
                            Page::Settings => self.settings_ui(ui),
                        }
                    });
            });
        self.commit_settings(ctx);
        self.dialogs_ui(ctx);
        self.about_dialog(ctx);
        self.toasts_ui(ctx);
        if self.shot.is_none() {
            ctx.request_repaint_after(std::time::Duration::from_secs(if self.demo {
                1
            } else {
                5
            }));
        }
    }

    fn raw_input_hook(&mut self, _ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        // Screenshots must not depend on where the mouse is.
        if self.shot.is_some() {
            raw_input.events.retain(|e| {
                !matches!(
                    e,
                    egui::Event::PointerMoved(_)
                        | egui::Event::MouseMoved(_)
                        | egui::Event::PointerButton { .. }
                        | egui::Event::MouseWheel { .. }
                )
            });
            raw_input.events.push(egui::Event::PointerGone);
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.stop_impact();
        if let Ok(mut waker) = self.shared.waker.lock() {
            *waker = None;
        }
        self.send(Cmd::Flush);
    }
}
