//! The background engine: one thread with a hidden window that owns the
//! tray icon, the hotkey, keep-awake (SetThreadExecutionState must be held
//! by a living thread), automatic plan switching, alerts and history
//! recording. The window talks to it through [`Shared`] and [`Cmd`].

use crate::alerts::AlertState;
use crate::autoswitch::{self, Switcher};
use crate::awake::{
    ActivityMeter, Awake, AwakeMode, AwakeRequest, AwakeStatus, Inputs, SystemExecutor,
};
use crate::battery::{self, BatteryInfo};
use crate::history::History;
use crate::plans::{Guid, PowerBackend, SystemPlans};
use crate::settings::Settings;
use crate::tray::{self, copy_wide};
use crate::util;
use std::cell::RefCell;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Mutex;
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    RegisterHotKey, UnregisterHotKey, HOT_KEY_MODIFIERS, MOD_NOREPEAT,
};
use windows::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_TIP, NIIF_INFO, NIIF_WARNING, NIM_ADD,
    NIM_DELETE, NIM_MODIFY, NOTIFYICONDATAW,
};
use windows::Win32::UI::WindowsAndMessaging::*;

/// One point of the live graph.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RatePoint {
    pub t: i64,
    /// Watts: + charging, - discharging.
    pub watts: f32,
    pub percent: f32,
}

/// What the engine knows right now.
#[derive(Clone, Debug, Default)]
pub struct LiveState {
    pub battery: Option<BatteryInfo>,
    /// Mains power (also known on desktops).
    pub on_ac: Option<bool>,
    /// The last few hours, oldest first, one point per sample.
    pub rate: VecDeque<RatePoint>,
    pub awake: AwakeStatus,
    pub active_plan: Option<Guid>,
    pub active_plan_name: String,
    /// The last automatic switch: (time, plain text).
    pub last_switch: Option<(i64, String)>,
    /// The last alert shown: (time, title).
    pub last_alert: Option<(i64, String)>,
    /// Bumped when the history changed.
    pub history_rev: u64,
}

/// Commands from the window (or the command line) to the engine.
#[derive(Clone, Debug)]
pub enum Cmd {
    StartAwake(AwakeRequest),
    StopAwake,
    /// New settings were saved.
    Settings(Box<Settings>),
    /// The window changed the active plan: refresh, and treat it as the
    /// user's own choice for auto-switching.
    PlanChanged,
    /// Write history now.
    Flush,
}

/// Where commands go. The demo uses a no-op that pretends.
pub trait Control: Send + Sync {
    fn send(&self, cmd: Cmd);
}

/// State shared between the engine thread and the window.
pub struct Shared {
    pub dir: PathBuf,
    pub live: Mutex<LiveState>,
    pub history: Mutex<History>,
    /// The program should exit.
    pub quit: AtomicBool,
    /// A page the window should show (set by the tray), "" = just show.
    pub show: Mutex<Option<String>>,
    /// Repaints the window when the engine changed something.
    pub waker: Mutex<Option<Box<dyn Fn() + Send>>>,
}

impl Shared {
    pub fn new(dir: PathBuf, history: History) -> Shared {
        Shared {
            dir,
            live: Mutex::new(LiveState::default()),
            history: Mutex::new(history),
            quit: AtomicBool::new(false),
            show: Mutex::new(None),
            waker: Mutex::new(None),
        }
    }

    /// A copy of the live state.
    pub fn live(&self) -> LiveState {
        self.live.lock().map(|l| l.clone()).unwrap_or_default()
    }

    /// Asks the window to repaint.
    pub fn wake(&self) {
        if let Ok(w) = self.waker.lock() {
            if let Some(f) = w.as_ref() {
                f();
            }
        }
    }
}

/// Seconds between samples.
pub const TICK_SECS: u32 = 5;
/// How much of the live graph is kept.
pub const RATE_KEEP_SECS: i64 = 6 * 3600;

/// Adds a reading to the live graph and drops what is too old.
pub fn push_rate(rate: &mut VecDeque<RatePoint>, info: &BatteryInfo, now: i64) {
    rate.push_back(RatePoint {
        t: now,
        watts: info.rate_mw.unwrap_or(0) as f32 / 1000.0,
        percent: info.percent,
    });
    while rate.front().is_some_and(|p| now - p.t > RATE_KEEP_SECS) {
        rate.pop_front();
    }
}

/// What the user asked the running engine for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UiRequest {
    /// Open (or bring to front) the window, optionally on a page.
    Open(Option<String>),
    /// The user chose Exit.
    Quit,
}

const WINDOW_CLASS: PCWSTR = w!("PowerForgeEngine");
const WM_TRAY: u32 = WM_APP + 1;
const WM_COMMANDS: u32 = WM_APP + 2;
const WM_QUIT_THREAD: u32 = WM_APP + 3;
const TIMER_TICK: usize = 1;
const HOTKEY_ID: i32 = 1;
const NIN_BALLOONUSERCLICK: u32 = 0x0405;

const MENU_OPEN: usize = 1;
const MENU_AWAKE_OFF: usize = 2;
const MENU_AWAKE_ON: usize = 3;
const MENU_AWAKE_30: usize = 4;
const MENU_AWAKE_60: usize = 5;
const MENU_AWAKE_120: usize = 6;
const MENU_DISPLAY: usize = 7;
const MENU_EXIT: usize = 8;
const MENU_SLEEP_PAGE: usize = 9;
const MENU_PLAN_BASE: usize = 100;

/// The message a second copy sends to the running one.
pub const OPEN_MESSAGE: PCWSTR = w!("PowerForge.Open");
/// wParam values of [`OPEN_MESSAGE`].
pub const SIGNAL_OPEN: usize = 0;
pub const SIGNAL_QUIT: usize = 1;
/// lParam = seconds (0 = indefinitely).
pub const SIGNAL_AWAKE: usize = 2;
pub const SIGNAL_AWAKE_OFF: usize = 3;

struct State {
    window: isize,
    shared: std::sync::Arc<Shared>,
    settings: Settings,
    commands: Receiver<Cmd>,
    requests: Sender<UiRequest>,
    awake: Awake,
    exec: SystemExecutor,
    activity: ActivityMeter,
    alerts: AlertState,
    switcher: Switcher,
    plans: SystemPlans,
    tray: bool,
    icon: isize,
    icon_key: String,
    history_dirty: bool,
    last_save: i64,
    plan_names: Vec<(Guid, String)>,
    taskbar_created: u32,
    open_message: u32,
}

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
}

fn with_state<R>(f: impl FnOnce(&mut State) -> R) -> Option<R> {
    STATE.with(|cell| {
        cell.try_borrow_mut()
            .ok()
            .and_then(|mut s| s.as_mut().map(f))
    })
}

fn hwnd(raw: isize) -> HWND {
    HWND(raw as *mut _)
}

impl State {
    fn tick(&mut self) {
        let now = util::now();
        let info = battery::read();
        let on_ac = info
            .as_ref()
            .map(|b| b.ac_online)
            .or_else(battery::ac_online);

        // Keep awake.
        let running = self.awake.needs_processes().then(autoswitch::running_exes);
        let (net, disk) = if self.awake.needs_activity() {
            self.activity.sample()
        } else {
            (None, None)
        };
        let status = self.awake.tick(
            &Inputs {
                now,
                running,
                net_bps: net,
                disk_bps: disk,
            },
            &mut self.exec,
        );
        if let Some(why) = &status.ended {
            let was = self.shared.live().awake.active;
            if was {
                self.notify("Keep awake ended", why, false);
            }
        }

        // Automatic plan switching.
        let mut switched = None;
        if self.settings.auto.enabled && !self.settings.auto.rules.is_empty() {
            let ctx = autoswitch::current_context(on_ac);
            switched = self
                .switcher
                .step(&self.settings.auto, &ctx, &mut self.plans);
        }
        let active = self.plans.active().ok();
        if let Some(a) = active {
            if !self.plan_names.iter().any(|(g, _)| *g == a) {
                self.refresh_plan_names();
            }
        }
        let plan_name = active
            .and_then(|a| self.plan_names.iter().find(|(g, _)| *g == a))
            .map(|(_, n)| n.clone())
            .unwrap_or_default();
        if let Some(s) = &switched {
            self.notify(
                &format!("Switched to {plan_name}"),
                &format!("{}.", s.reason),
                false,
            );
        }

        // Alerts and history.
        let mut alert = None;
        if let Some(b) = &info {
            for a in self.alerts.check(&self.settings.alerts, b) {
                self.notify(
                    &a.title(),
                    &a.text(),
                    matches!(a, crate::alerts::Alert::Low(_)),
                );
                alert = Some((now, a.title()));
            }
            if self.settings.record_history {
                if let Ok(mut h) = self.shared.history.lock() {
                    if h.record(b, now) {
                        self.history_dirty = true;
                    }
                }
            }
        }
        if self.history_dirty && now - self.last_save >= 600 {
            self.save_history(now);
        }

        if let Ok(mut live) = self.shared.live.lock() {
            if let Some(b) = &info {
                push_rate(&mut live.rate, b, now);
            }
            live.battery = info.clone();
            live.on_ac = on_ac;
            live.awake = status.clone();
            live.active_plan = active;
            live.active_plan_name = plan_name.clone();
            if let Some(s) = switched {
                live.last_switch = Some((now, format!("{plan_name}: {}", s.reason)));
            }
            if alert.is_some() {
                live.last_alert = alert;
            }
            live.history_rev += 1;
        }
        self.update_tray(info.as_ref(), on_ac, status.active, &plan_name);
        self.shared.wake();
    }

    fn save_history(&mut self, now: i64) {
        if let Ok(h) = self.shared.history.lock() {
            let _ = h.save(&self.shared.dir);
        }
        self.history_dirty = false;
        self.last_save = now;
    }

    fn refresh_plan_names(&mut self) {
        self.plan_names = self
            .plans
            .list()
            .map(|l| l.into_iter().map(|p| (p.guid, p.name)).collect())
            .unwrap_or_default();
    }

    fn tray_data(&self) -> NOTIFYICONDATAW {
        NOTIFYICONDATAW {
            cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: hwnd(self.window),
            uID: 1,
            uFlags: NIF_ICON | NIF_MESSAGE | NIF_TIP,
            uCallbackMessage: WM_TRAY,
            hIcon: if self.icon != 0 {
                HICON(self.icon as _)
            } else {
                tray::app_icon()
            },
            ..Default::default()
        }
    }

    fn update_tray(
        &mut self,
        info: Option<&BatteryInfo>,
        on_ac: Option<bool>,
        awake: bool,
        plan: &str,
    ) {
        let text = if self.settings.tray_percent {
            tray::icon_text(info)
        } else {
            String::new()
        };
        let style = info
            .map(|b| tray::style_for(b, self.settings.alerts.low_pct))
            .unwrap_or(tray::IconStyle::Charging);
        let key = format!("{text}|{style:?}|{awake}");
        if key != self.icon_key {
            let old = self.icon;
            self.icon = if text.is_empty() {
                0
            } else {
                tray::render_icon(&text, style, awake)
            };
            self.icon_key = key;
            if old != 0 {
                // SAFETY: an icon this engine created and no longer shows.
                let _ = unsafe { DestroyIcon(HICON(old as _)) };
            }
        }
        let mut data = self.tray_data();
        copy_wide(&mut data.szTip, &tray::tip_text(info, on_ac, awake, plan));
        // SAFETY: a fully initialised structure that outlives the call.
        unsafe {
            if !self.tray {
                self.tray = Shell_NotifyIconW(NIM_ADD, &data).as_bool();
            } else {
                let _ = Shell_NotifyIconW(NIM_MODIFY, &data);
            }
        }
    }

    fn remove_tray(&mut self) {
        if self.tray {
            let data = self.tray_data();
            // SAFETY: as above.
            unsafe {
                let _ = Shell_NotifyIconW(NIM_DELETE, &data);
            }
            self.tray = false;
        }
        if self.icon != 0 {
            // SAFETY: our own icon, no longer shown.
            let _ = unsafe { DestroyIcon(HICON(self.icon as _)) };
            self.icon = 0;
        }
    }

    fn notify(&mut self, title: &str, text: &str, warning: bool) {
        if !self.tray {
            return;
        }
        let mut data = self.tray_data();
        data.uFlags |= NIF_INFO;
        data.dwInfoFlags = if warning { NIIF_WARNING } else { NIIF_INFO };
        copy_wide(&mut data.szInfoTitle, title);
        copy_wide(&mut data.szInfo, text);
        // SAFETY: as above.
        unsafe {
            let _ = Shell_NotifyIconW(NIM_MODIFY, &data);
        }
    }

    fn register_hotkey(&mut self) {
        // SAFETY: registering a hotkey on our own window.
        unsafe {
            let _ = UnregisterHotKey(hwnd(self.window), HOTKEY_ID);
            if let Some(h) = tray::parse_hotkey(&self.settings.hotkey) {
                let _ = RegisterHotKey(
                    hwnd(self.window),
                    HOTKEY_ID,
                    HOT_KEY_MODIFIERS(h.modifiers) | MOD_NOREPEAT,
                    h.vk,
                );
            }
        }
    }

    fn start_awake(&mut self, req: AwakeRequest) {
        let now = util::now();
        self.awake.start(req, now, &mut self.exec);
        if self.awake.needs_activity() {
            self.activity.sample();
        }
        let status = self.awake.status(now);
        self.notify("Keeping the PC awake", &status.text, false);
        self.tick();
    }

    fn stop_awake(&mut self) {
        self.awake.stop(&mut self.exec);
        self.tick();
    }

    fn toggle_awake(&mut self) {
        if self.awake.is_active() {
            self.stop_awake();
            self.notify("Keep awake is off", "The PC sleeps as usual again.", false);
        } else {
            self.start_awake(AwakeRequest {
                mode: AwakeMode::Indefinite,
                keep_display: self.settings.awake_display,
            });
        }
    }

    fn handle_commands(&mut self) {
        while let Ok(cmd) = self.commands.try_recv() {
            match cmd {
                Cmd::StartAwake(req) => self.start_awake(req),
                Cmd::StopAwake => self.stop_awake(),
                Cmd::Settings(s) => {
                    let was_auto = self.settings.auto.enabled;
                    let hotkey_changed = s.hotkey != self.settings.hotkey;
                    self.settings = *s;
                    if was_auto && !self.settings.auto.enabled {
                        self.switcher.release(&mut self.plans);
                    }
                    if hotkey_changed {
                        self.register_hotkey();
                    }
                    self.icon_key.clear();
                    self.tick();
                }
                Cmd::PlanChanged => {
                    self.refresh_plan_names();
                    self.tick();
                }
                Cmd::Flush => {
                    let now = util::now();
                    self.save_history(now);
                }
            }
        }
    }

    fn request_open(&self, page: Option<&str>) {
        if let Ok(mut show) = self.shared.show.lock() {
            *show = Some(page.unwrap_or("").to_string());
        }
        let _ = self
            .requests
            .send(UiRequest::Open(page.map(|p| p.to_string())));
        self.shared.wake();
    }

    fn request_quit(&self) {
        self.shared.quit.store(true, Ordering::SeqCst);
        let _ = self.requests.send(UiRequest::Quit);
        self.shared.wake();
    }
}

unsafe fn tray_menu(window: HWND) {
    // Gather what the menu shows first: no state borrow may be held while
    // the menu runs its own message loop.
    let Some((header, awake, display, plans, active)) = with_state(|s| {
        s.refresh_plan_names();
        let live = s.shared.live();
        let header = match &live.battery {
            Some(b) => format!("Battery {:.0} %  \u{b7}  {}", b.percent, b.state.label()),
            None => "No battery: mains power".into(),
        };
        (
            header,
            s.awake.is_active(),
            s.settings.awake_display,
            s.plan_names.clone(),
            live.active_plan,
        )
    }) else {
        return;
    };
    let Ok(menu) = CreatePopupMenu() else { return };
    let header = crate::sys::wide(&header);
    let _ = AppendMenuW(menu, MF_STRING | MF_GRAYED, 0, PCWSTR(header.as_ptr()));
    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
    let _ = AppendMenuW(
        menu,
        MF_STRING | MF_DEFAULT,
        MENU_OPEN,
        w!("Open PowerForge"),
    );
    if let Ok(sub) = CreatePopupMenu() {
        let check = |on: bool| if on { MF_CHECKED } else { MF_UNCHECKED };
        let _ = AppendMenuW(sub, MF_STRING | check(!awake), MENU_AWAKE_OFF, w!("Off"));
        let _ = AppendMenuW(
            sub,
            MF_STRING | check(awake),
            MENU_AWAKE_ON,
            w!("Until I turn it off"),
        );
        let _ = AppendMenuW(sub, MF_STRING, MENU_AWAKE_30, w!("For 30 minutes"));
        let _ = AppendMenuW(sub, MF_STRING, MENU_AWAKE_60, w!("For 1 hour"));
        let _ = AppendMenuW(sub, MF_STRING, MENU_AWAKE_120, w!("For 2 hours"));
        let _ = AppendMenuW(sub, MF_SEPARATOR, 0, PCWSTR::null());
        let _ = AppendMenuW(
            sub,
            MF_STRING | check(display),
            MENU_DISPLAY,
            w!("Keep the screen on too"),
        );
        let _ = AppendMenuW(menu, MF_POPUP, sub.0 as usize, w!("Keep awake"));
    }
    if !plans.is_empty() {
        if let Ok(sub) = CreatePopupMenu() {
            for (i, (guid, name)) in plans.iter().enumerate().take(50) {
                let label = crate::sys::wide(name);
                let flags = if Some(*guid) == active {
                    MF_STRING | MF_CHECKED
                } else {
                    MF_STRING
                };
                let _ = AppendMenuW(sub, flags, MENU_PLAN_BASE + i, PCWSTR(label.as_ptr()));
            }
            let _ = AppendMenuW(menu, MF_POPUP, sub.0 as usize, w!("Power plan"));
        }
    }
    let _ = AppendMenuW(
        menu,
        MF_STRING,
        MENU_SLEEP_PAGE,
        w!("Why won't my PC sleep?"),
    );
    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
    let _ = AppendMenuW(menu, MF_STRING, MENU_EXIT, w!("Exit"));
    let mut pt = POINT::default();
    let _ = GetCursorPos(&mut pt);
    let _ = SetForegroundWindow(window);
    let choice = TrackPopupMenu(
        menu,
        TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON,
        pt.x,
        pt.y,
        0,
        window,
        None,
    );
    let _ = DestroyMenu(menu);
    let minutes = |m: i64| AwakeMode::Until(util::now() + m * 60);
    with_state(|s| match choice.0 as usize {
        MENU_OPEN => s.request_open(None),
        MENU_SLEEP_PAGE => s.request_open(Some("sleep")),
        MENU_AWAKE_OFF => s.stop_awake(),
        MENU_AWAKE_ON => s.start_awake(AwakeRequest {
            mode: AwakeMode::Indefinite,
            keep_display: s.settings.awake_display,
        }),
        MENU_AWAKE_30 | MENU_AWAKE_60 | MENU_AWAKE_120 => {
            let m = match choice.0 as usize {
                MENU_AWAKE_30 => 30,
                MENU_AWAKE_60 => 60,
                _ => 120,
            };
            s.start_awake(AwakeRequest {
                mode: minutes(m),
                keep_display: s.settings.awake_display,
            })
        }
        MENU_DISPLAY => {
            s.settings.awake_display = !s.settings.awake_display;
            let _ = s.settings.save(&s.shared.dir);
            if let Some(req) = s.awake.status(util::now()).request {
                s.start_awake(AwakeRequest {
                    keep_display: s.settings.awake_display,
                    ..req
                });
            }
        }
        MENU_EXIT => s.request_quit(),
        n if n >= MENU_PLAN_BASE && n < MENU_PLAN_BASE + plans.len() => {
            let guid = plans[n - MENU_PLAN_BASE].0;
            if s.plans.set_active(guid).is_ok() {
                s.tick();
            }
        }
        _ => {}
    });
}

unsafe extern "system" fn wnd_proc(
    window: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_TIMER => {
            if wparam.0 == TIMER_TICK {
                with_state(|s| s.tick());
            }
            LRESULT(0)
        }
        WM_COMMANDS => {
            with_state(|s| s.handle_commands());
            LRESULT(0)
        }
        WM_POWERBROADCAST => {
            // PBT_APMPOWERSTATUSCHANGE: plugged in or out.
            if wparam.0 == 0x000A {
                with_state(|s| s.tick());
            }
            LRESULT(1)
        }
        WM_HOTKEY => {
            with_state(|s| s.toggle_awake());
            LRESULT(0)
        }
        WM_TRAY => {
            match lparam.0 as u32 {
                WM_LBUTTONUP | WM_LBUTTONDBLCLK | NIN_BALLOONUSERCLICK => {
                    with_state(|s| s.request_open(None));
                }
                WM_RBUTTONUP | WM_CONTEXTMENU => tray_menu(window),
                _ => {}
            }
            LRESULT(0)
        }
        WM_ENDSESSION => {
            if wparam.0 != 0 {
                with_state(|s| s.save_history(util::now()));
            }
            LRESULT(0)
        }
        WM_QUIT_THREAD => {
            let _ = DestroyWindow(window);
            LRESULT(0)
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => {
            let (open, taskbar) =
                with_state(|s| (s.open_message, s.taskbar_created)).unwrap_or((0, 0));
            if open != 0 && msg == open {
                with_state(|s| match wparam.0 {
                    SIGNAL_QUIT => s.request_quit(),
                    SIGNAL_AWAKE => {
                        let secs = lparam.0 as i64;
                        let mode = if secs <= 0 {
                            AwakeMode::Indefinite
                        } else {
                            AwakeMode::Until(util::now() + secs)
                        };
                        s.start_awake(AwakeRequest {
                            mode,
                            keep_display: s.settings.awake_display,
                        });
                    }
                    SIGNAL_AWAKE_OFF => s.stop_awake(),
                    _ => s.request_open(None),
                });
                return LRESULT(0);
            }
            if taskbar != 0 && msg == taskbar {
                with_state(|s| {
                    s.tray = false;
                    s.icon_key.clear();
                    s.tick();
                });
                return LRESULT(0);
            }
            DefWindowProcW(window, msg, wparam, lparam)
        }
    }
}

/// Handle to the running engine thread.
pub struct Engine {
    window: isize,
    commands: Mutex<Sender<Cmd>>,
    requests: Mutex<Receiver<UiRequest>>,
    thread: Mutex<Option<std::thread::JoinHandle<()>>>,
}

impl Control for Engine {
    fn send(&self, cmd: Cmd) {
        if let Ok(tx) = self.commands.lock() {
            let _ = tx.send(cmd);
        }
        // SAFETY: posting to our own window; a stale handle just fails.
        unsafe {
            let _ = PostMessageW(hwnd(self.window), WM_COMMANDS, WPARAM(0), LPARAM(0));
        }
    }
}

/// Sends a signal to an already running PowerForge. Returns whether one
/// was found.
pub fn signal_running_instance(signal: usize, value: isize) -> bool {
    // SAFETY: looking up a window by class and posting a registered message.
    unsafe {
        let Ok(window) = FindWindowW(WINDOW_CLASS, PCWSTR::null()) else {
            return false;
        };
        if window.0.is_null() {
            return false;
        }
        let message = RegisterWindowMessageW(OPEN_MESSAGE);
        PostMessageW(window, message, WPARAM(signal), LPARAM(value)).is_ok()
    }
}

impl Engine {
    /// Starts the engine. `None` when the hidden window could not be created.
    pub fn start(
        shared: std::sync::Arc<Shared>,
        settings: Settings,
        awake: Option<AwakeRequest>,
    ) -> Option<Engine> {
        let (cmd_tx, cmd_rx) = channel::<Cmd>();
        let (req_tx, req_rx) = channel::<UiRequest>();
        let (ready_tx, ready_rx) = channel::<isize>();
        let thread = std::thread::Builder::new()
            .name("powerforge-engine".into())
            .spawn(move || thread_main(shared, settings, awake, cmd_rx, req_tx, ready_tx))
            .ok()?;
        let window = ready_rx.recv().ok().filter(|w| *w != 0)?;
        Some(Engine {
            window,
            commands: Mutex::new(cmd_tx),
            requests: Mutex::new(req_rx),
            thread: Mutex::new(Some(thread)),
        })
    }

    /// Blocks until the engine wants the window opened or the program ended.
    pub fn wait_request(&self) -> UiRequest {
        match self.requests.lock() {
            Ok(rx) => rx.recv().unwrap_or(UiRequest::Quit),
            Err(_) => UiRequest::Quit,
        }
    }

    /// Forgets requests that piled up while the window was open. Returns
    /// whether one of them was Quit.
    pub fn drain_requests(&self) -> bool {
        let mut quit = false;
        if let Ok(rx) = self.requests.lock() {
            while let Ok(r) = rx.try_recv() {
                quit |= r == UiRequest::Quit;
            }
        }
        quit
    }

    /// Stops the engine: releases keep-awake, removes the icon and the
    /// hotkey, writes the history, and waits for the thread to end.
    pub fn stop(&self) {
        // SAFETY: posting to our own window.
        unsafe {
            let _ = PostMessageW(hwnd(self.window), WM_QUIT_THREAD, WPARAM(0), LPARAM(0));
        }
        if let Ok(mut guard) = self.thread.lock() {
            if let Some(t) = guard.take() {
                let _ = t.join();
            }
        }
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.stop();
    }
}

fn thread_main(
    shared: std::sync::Arc<Shared>,
    settings: Settings,
    awake: Option<AwakeRequest>,
    commands: Receiver<Cmd>,
    requests: Sender<UiRequest>,
    ready: Sender<isize>,
) {
    // SAFETY: a standard window class, window and message loop, all owned by
    // this thread and torn down before it ends.
    unsafe {
        let module = GetModuleHandleW(PCWSTR::null()).unwrap_or_default();
        let class = WNDCLASSW {
            lpfnWndProc: Some(wnd_proc),
            hInstance: HINSTANCE(module.0),
            lpszClassName: WINDOW_CLASS,
            ..Default::default()
        };
        RegisterClassW(&class);
        // A hidden top-level window: power broadcasts only reach top-level windows.
        let Ok(window) = CreateWindowExW(
            WS_EX_TOOLWINDOW,
            WINDOW_CLASS,
            w!("PowerForge engine"),
            WS_POPUP,
            0,
            0,
            0,
            0,
            HWND::default(),
            HMENU::default(),
            HINSTANCE(module.0),
            None,
        ) else {
            let _ = ready.send(0);
            return;
        };
        let state = State {
            window: window.0 as isize,
            shared,
            settings,
            commands,
            requests,
            awake: Awake::new(),
            exec: SystemExecutor,
            activity: ActivityMeter::new(),
            alerts: AlertState::default(),
            switcher: Switcher::new(),
            plans: SystemPlans,
            tray: false,
            icon: 0,
            icon_key: String::new(),
            history_dirty: false,
            last_save: util::now(),
            plan_names: Vec::new(),
            taskbar_created: RegisterWindowMessageW(w!("TaskbarCreated")),
            open_message: RegisterWindowMessageW(OPEN_MESSAGE),
        };
        STATE.with(|cell| *cell.borrow_mut() = Some(state));
        with_state(|s| {
            s.refresh_plan_names();
            s.register_hotkey();
            match awake {
                Some(req) => s.start_awake(req),
                None => s.tick(),
            }
        });
        SetTimer(window, TIMER_TICK, TICK_SECS * 1000, None);
        let _ = ready.send(window.0 as isize);

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, HWND::default(), 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }

        // Tear down. The window is already destroyed.
        let _ = KillTimer(window, TIMER_TICK);
        if let Some(mut s) = STATE.with(|cell| cell.borrow_mut().take()) {
            let _ = UnregisterHotKey(hwnd(s.window), HOTKEY_ID);
            s.awake.stop(&mut s.exec);
            if s.settings.auto.enabled {
                s.switcher.release(&mut s.plans);
            }
            s.remove_tray();
            s.save_history(util::now());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_graph_keeps_six_hours() {
        let mut rate = VecDeque::new();
        let b = BatteryInfo {
            percent: 50.0,
            rate_mw: Some(-9_500),
            ..Default::default()
        };
        for i in 0..(8 * 3600 / 60) {
            push_rate(&mut rate, &b, i * 60);
        }
        assert_eq!(rate.back().unwrap().watts, -9.5);
        assert!(rate.back().unwrap().t - rate.front().unwrap().t <= RATE_KEEP_SECS);
        assert_eq!(rate.len() as i64, RATE_KEEP_SECS / 60 + 1);
    }

    #[test]
    fn shared_state() {
        let s = Shared::new(std::env::temp_dir(), History::default());
        assert!(s.live().battery.is_none());
        s.wake();
        let hit = std::sync::Arc::new(AtomicBool::new(false));
        let h = hit.clone();
        *s.waker.lock().unwrap() = Some(Box::new(move || h.store(true, Ordering::SeqCst)));
        s.wake();
        assert!(hit.load(Ordering::SeqCst));
    }
}
