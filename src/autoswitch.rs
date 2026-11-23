//! Automatic plan switching by power source and by running / foreground
//! app. The decision logic is pure and tested; the engine feeds it the
//! context and applies the result through a `PowerBackend`.

use crate::plans::{Guid, PowerBackend};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Trigger {
    OnBattery,
    PluggedIn,
    /// The app (exe name, case-insensitive, e.g. "game.exe") is running.
    AppRunning(String),
    /// The app is the foreground window.
    AppForeground(String),
}

impl Trigger {
    /// "When on battery", "When game.exe is running", ...
    pub fn label(&self) -> String {
        match self {
            Trigger::OnBattery => "When running on battery".into(),
            Trigger::PluggedIn => "When plugged in".into(),
            Trigger::AppRunning(exe) => format!("While {exe} is running"),
            Trigger::AppForeground(exe) => format!("While {exe} is in front"),
        }
    }

    fn reason(&self) -> String {
        match self {
            Trigger::OnBattery => "Running on battery".into(),
            Trigger::PluggedIn => "Plugged in".into(),
            Trigger::AppRunning(exe) => format!("{exe} is running"),
            Trigger::AppForeground(exe) => format!("{exe} is in front"),
        }
    }

    fn group(&self) -> u8 {
        match self {
            Trigger::AppForeground(_) => 0,
            Trigger::AppRunning(_) => 1,
            Trigger::OnBattery | Trigger::PluggedIn => 2,
        }
    }

    fn matches(&self, ctx: &Context) -> bool {
        match self {
            Trigger::OnBattery => ctx.on_ac == Some(false),
            Trigger::PluggedIn => ctx.on_ac == Some(true),
            Trigger::AppRunning(exe) => ctx.running.contains(&normalize(exe)),
            Trigger::AppForeground(exe) => {
                ctx.foreground.as_deref() == Some(normalize(exe).as_str())
            }
        }
    }
}

/// "Game.EXE " -> "game.exe"; "game" -> "game.exe".
pub fn normalize(exe: &str) -> String {
    let e = exe.trim().to_lowercase();
    let e = e.rsplit(['\\', '/']).next().unwrap_or("").to_string();
    if e.is_empty() || e.ends_with(".exe") {
        e
    } else {
        format!("{e}.exe")
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Rule {
    pub enabled: bool,
    pub trigger: Trigger,
    pub plan: Guid,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AutoSettings {
    /// Off by default: nothing is switched until the user turns it on.
    pub enabled: bool,
    pub rules: Vec<Rule>,
}

/// What the rules can see.
#[derive(Clone, Debug, Default)]
pub struct Context {
    pub on_ac: Option<bool>,
    /// Lower-case exe name of the foreground window's process.
    pub foreground: Option<String>,
    /// Lower-case exe names of running processes.
    pub running: HashSet<String>,
}

fn matching<'a>(rules: &'a [Rule], ctx: &Context) -> Option<&'a Rule> {
    (0..3u8).find_map(|group| {
        rules
            .iter()
            .filter(|r| r.enabled && r.trigger.group() == group)
            .find(|r| r.trigger.matches(ctx))
    })
}

/// The plan the rules want, if any. Priority: foreground-app rules, then
/// running-app rules, then power-source rules; within a group, list order.
pub fn decide(rules: &[Rule], ctx: &Context) -> Option<Guid> {
    matching(rules, ctx).map(|r| r.plan)
}

/// A switch that was made.
#[derive(Clone, Debug, PartialEq)]
pub struct Switched {
    pub to: Guid,
    /// Plain reason: "game.exe is running", "Running on battery", "Back to your plan".
    pub reason: String,
}

/// Applies decisions with memory: remembers the user's own plan before the
/// first automatic switch and returns to it when no rule matches any more.
/// If the user changes the plan by hand while a rule holds, that choice is
/// respected until the matching rule changes.
#[derive(Clone, Debug, Default)]
pub struct Switcher {
    /// The user's plan from before the first automatic switch.
    baseline: Option<Guid>,
    /// The plan we last switched to.
    applied: Option<Guid>,
    /// The decision of the previous step (debounce).
    pending: Option<Option<Guid>>,
    /// The decision currently in force (what the rules wanted last time we acted).
    current: Option<Option<Guid>>,
}

impl Switcher {
    pub fn new() -> Switcher {
        Switcher::default()
    }

    /// One step (the engine calls it every few seconds).
    pub fn step(
        &mut self,
        settings: &AutoSettings,
        ctx: &Context,
        backend: &mut dyn PowerBackend,
    ) -> Option<Switched> {
        if !settings.enabled {
            return self.release(backend);
        }
        let rule = matching(&settings.rules, ctx);
        let want = rule.map(|r| r.plan);
        let active = backend.active().ok()?;

        // The user picked a plan by hand since our last switch.
        if let Some(applied) = self.applied {
            if active != applied {
                self.applied = None;
                if want.is_none() {
                    self.baseline = None;
                } else {
                    // Respect it while the same rule holds; it becomes the plan
                    // to return to.
                    self.baseline = Some(active);
                    self.current = Some(want);
                    self.pending = Some(want);
                    return None;
                }
            }
        }

        // Debounce: act only on the same decision twice in a row (the very
        // first decision acts at once).
        let first = self.current.is_none();
        let stable = self.pending == Some(want);
        self.pending = Some(want);
        if !first && !stable {
            return None;
        }
        if self.current == Some(want) {
            return None;
        }
        self.current = Some(want);

        match (want, rule) {
            (Some(target), Some(rule)) => {
                let exists = backend
                    .list()
                    .map(|l| l.iter().any(|p| p.guid == target))
                    .unwrap_or(false);
                if !exists {
                    return None;
                }
                if self.baseline.is_none() {
                    self.baseline = Some(active);
                }
                if active == target {
                    self.applied = Some(target);
                    return None;
                }
                backend.set_active(target).ok()?;
                self.applied = Some(target);
                Some(Switched {
                    to: target,
                    reason: rule.trigger.reason(),
                })
            }
            _ => self.go_back(backend),
        }
    }

    fn go_back(&mut self, backend: &mut dyn PowerBackend) -> Option<Switched> {
        let base = self.baseline.take();
        let applied = self.applied.take();
        let base = base?;
        applied?;
        if backend.active().ok() == Some(base) {
            return None;
        }
        backend.set_active(base).ok()?;
        Some(Switched {
            to: base,
            reason: "Back to your plan".into(),
        })
    }

    /// Called when auto-switching is turned off: returns to the user's plan
    /// if a rule had switched away from it.
    pub fn release(&mut self, backend: &mut dyn PowerBackend) -> Option<Switched> {
        self.pending = None;
        self.current = None;
        let applied = self.applied;
        if let Some(applied) = applied {
            if backend.active().ok() != Some(applied) {
                // The user changed it meanwhile: leave their choice.
                self.applied = None;
                self.baseline = None;
                return None;
            }
        }
        self.go_back(backend)
    }
}

/// The foreground process exe (lower case) and the set of running exes.
pub fn current_context(on_ac: Option<bool>) -> Context {
    Context {
        on_ac,
        foreground: foreground_exe(),
        running: running_exes(),
    }
}

/// Lower-case exe names of all running processes.
pub fn running_exes() -> HashSet<String> {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };
    let mut out = HashSet::new();
    // SAFETY: a process snapshot walked with correctly sized entries and closed.
    unsafe {
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else {
            return out;
        };
        let mut entry = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        let mut ok = Process32FirstW(snap, &mut entry).is_ok();
        while ok {
            let name = crate::sys::from_wide(&entry.szExeFile).to_lowercase();
            if !name.is_empty() {
                out.insert(name);
            }
            ok = Process32NextW(snap, &mut entry).is_ok();
        }
        let _ = CloseHandle(snap);
    }
    out
}

/// Lower-case exe name of the foreground window's process.
pub fn foreground_exe() -> Option<String> {
    use windows::core::PWSTR;
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
        PROCESS_QUERY_LIMITED_INFORMATION,
    };
    use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};
    // SAFETY: plain calls; the process handle is closed.
    unsafe {
        let window = GetForegroundWindow();
        if window.0.is_null() {
            return None;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(window, Some(&mut pid));
        if pid == 0 {
            return None;
        }
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let ok =
            QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len)
                .is_ok();
        let _ = CloseHandle(h);
        if !ok {
            return None;
        }
        let path = String::from_utf16_lossy(&buf[..len as usize]);
        Some(normalize(&path))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plans::MockPlans;

    fn ctx(on_ac: Option<bool>, fg: Option<&str>, running: &[&str]) -> Context {
        Context {
            on_ac,
            foreground: fg.map(|s| s.to_string()),
            running: running.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn rule(trigger: Trigger, plan: Guid) -> Rule {
        Rule {
            enabled: true,
            trigger,
            plan,
        }
    }

    fn settings(rules: Vec<Rule>) -> AutoSettings {
        AutoSettings {
            enabled: true,
            rules,
        }
    }

    #[test]
    fn priorities() {
        let rules = vec![
            rule(Trigger::OnBattery, Guid::POWER_SAVER),
            rule(Trigger::PluggedIn, Guid::BALANCED),
            rule(
                Trigger::AppRunning("Render.exe".into()),
                Guid::HIGH_PERFORMANCE,
            ),
            rule(Trigger::AppForeground("game".into()), Guid::ULTIMATE),
        ];
        assert_eq!(
            decide(&rules, &ctx(Some(false), None, &[])),
            Some(Guid::POWER_SAVER)
        );
        assert_eq!(
            decide(&rules, &ctx(Some(true), None, &[])),
            Some(Guid::BALANCED)
        );
        assert_eq!(decide(&rules, &ctx(None, None, &[])), None);
        // A running app beats the power source.
        assert_eq!(
            decide(&rules, &ctx(Some(false), None, &["render.exe"])),
            Some(Guid::HIGH_PERFORMANCE)
        );
        // The foreground app beats a running app.
        assert_eq!(
            decide(
                &rules,
                &ctx(Some(false), Some("game.exe"), &["render.exe", "game.exe"])
            ),
            Some(Guid::ULTIMATE)
        );
        // Disabled rules are ignored.
        let mut off = rules.clone();
        off[0].enabled = false;
        assert_eq!(decide(&off, &ctx(Some(false), None, &[])), None);
        assert_eq!(normalize(" C:\\Apps\\Game.EXE "), "game.exe");
        assert_eq!(normalize("game"), "game.exe");
        assert!(Trigger::AppRunning("x.exe".into())
            .label()
            .contains("x.exe"));
    }

    #[test]
    fn switch_and_return_to_baseline() {
        let mut m = MockPlans::new();
        let s = settings(vec![rule(Trigger::OnBattery, Guid::POWER_SAVER)]);
        let mut sw = Switcher::new();
        // First decision acts at once.
        let r = sw.step(&s, &ctx(Some(false), None, &[]), &mut m).unwrap();
        assert_eq!(r.to, Guid::POWER_SAVER);
        assert_eq!(r.reason, "Running on battery");
        assert_eq!(m.active().unwrap(), Guid::POWER_SAVER);
        assert!(sw.step(&s, &ctx(Some(false), None, &[]), &mut m).is_none());
        // Plugged in: debounced, then back to Balanced.
        assert!(sw.step(&s, &ctx(Some(true), None, &[]), &mut m).is_none());
        let r = sw.step(&s, &ctx(Some(true), None, &[]), &mut m).unwrap();
        assert_eq!(r.to, Guid::BALANCED);
        assert_eq!(r.reason, "Back to your plan");
        assert_eq!(m.activations, 2);
        // A flap that lasts one step does nothing.
        assert!(sw.step(&s, &ctx(Some(false), None, &[]), &mut m).is_none());
        assert!(sw.step(&s, &ctx(Some(true), None, &[]), &mut m).is_none());
        assert_eq!(m.activations, 2);
    }

    #[test]
    fn manual_change_is_respected() {
        let mut m = MockPlans::new();
        let s = settings(vec![rule(
            Trigger::AppRunning("game.exe".into()),
            Guid::HIGH_PERFORMANCE,
        )]);
        let mut sw = Switcher::new();
        assert!(sw
            .step(&s, &ctx(None, None, &["game.exe"]), &mut m)
            .is_some());
        // The user picks Power saver while the game runs.
        m.set_active(Guid::POWER_SAVER).unwrap();
        for _ in 0..3 {
            assert!(sw
                .step(&s, &ctx(None, None, &["game.exe"]), &mut m)
                .is_none());
        }
        assert_eq!(m.active().unwrap(), Guid::POWER_SAVER);
        // The game ends: nothing to undo, the user's choice stays.
        sw.step(&s, &ctx(None, None, &[]), &mut m);
        sw.step(&s, &ctx(None, None, &[]), &mut m);
        assert_eq!(m.active().unwrap(), Guid::POWER_SAVER);
    }

    #[test]
    fn missing_plan_and_release() {
        let mut m = MockPlans::new();
        let ghost = Guid(0x1234);
        let s = settings(vec![rule(Trigger::PluggedIn, ghost)]);
        let mut sw = Switcher::new();
        assert!(sw.step(&s, &ctx(Some(true), None, &[]), &mut m).is_none());
        assert_eq!(m.active().unwrap(), Guid::BALANCED);

        let s = settings(vec![rule(Trigger::PluggedIn, Guid::HIGH_PERFORMANCE)]);
        let mut sw = Switcher::new();
        assert!(sw.step(&s, &ctx(Some(true), None, &[]), &mut m).is_some());
        // Turning the feature off returns to the user's plan.
        let off = AutoSettings {
            enabled: false,
            ..s.clone()
        };
        let r = sw.step(&off, &ctx(Some(true), None, &[]), &mut m).unwrap();
        assert_eq!(r.to, Guid::BALANCED);
        assert!(sw.release(&mut m).is_none());
        // Already on the target: no switch, but release still has nothing to undo.
        let mut sw = Switcher::new();
        m.set_active(Guid::HIGH_PERFORMANCE).unwrap();
        assert!(sw.step(&s, &ctx(Some(true), None, &[]), &mut m).is_none());
        assert!(sw.release(&mut m).is_none());
    }

    #[test]
    fn reads_processes() {
        let running = running_exes();
        assert!(!running.is_empty());
        assert!(running.iter().all(|n| n == &n.to_lowercase()));
    }
}
