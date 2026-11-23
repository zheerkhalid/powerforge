//! Power plans: listing, switching, duplicating, renaming, deleting, key
//! settings in plain language, restoring defaults.
//!
//! Everything goes through the [`PowerBackend`] trait: [`SystemPlans`] talks
//! to Windows (powrprof, locale independent), [`MockPlans`] is an in-memory
//! fake used by tests, the selftest and `--demo`. Tests never change the
//! real plans.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;

/// A power scheme / subgroup / setting GUID.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default, PartialOrd, Ord)]
pub struct Guid(pub u128);

impl Guid {
    pub const BALANCED: Guid = Guid(0x381b4222_f694_41f0_9685_ff5bb260df2e);
    pub const HIGH_PERFORMANCE: Guid = Guid(0x8c5e7fda_e8bf_4a96_9a85_a6e23a8c635c);
    pub const POWER_SAVER: Guid = Guid(0xa1841308_3541_4fab_bc81_f71556f20b4a);
    pub const ULTIMATE: Guid = Guid(0xe9a42b02_d5df_448d_aa00_03f14749eb61);

    /// Parses "381b4222-f694-41f0-9685-ff5bb260df2e", with or without braces,
    /// any case.
    pub fn parse(text: &str) -> Option<Guid> {
        let t = text.trim();
        let t = t
            .strip_prefix('{')
            .and_then(|r| r.strip_suffix('}'))
            .unwrap_or(t);
        let groups: Vec<&str> = t.split('-').collect();
        let lens = [8, 4, 4, 4, 12];
        if groups.len() != 5 || groups.iter().zip(lens).any(|(g, l)| g.len() != l) {
            return None;
        }
        let hex: String = groups.concat();
        if !hex.chars().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        u128::from_str_radix(&hex, 16).ok().map(Guid)
    }

    /// One of the plans Windows ships with.
    pub fn is_builtin(self) -> bool {
        matches!(
            self,
            Guid::BALANCED | Guid::HIGH_PERFORMANCE | Guid::POWER_SAVER | Guid::ULTIMATE
        )
    }

    pub fn to_win(self) -> windows::core::GUID {
        windows::core::GUID::from_u128(self.0)
    }

    pub fn from_win(g: &windows::core::GUID) -> Guid {
        Guid(g.to_u128())
    }
}

impl fmt::Display for Guid {
    /// Lower-case hyphenated form, no braces.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let v = self.0;
        write!(
            f,
            "{:08x}-{:04x}-{:04x}-{:04x}-{:012x}",
            (v >> 96) as u32,
            (v >> 80) as u16,
            (v >> 64) as u16,
            (v >> 48) as u16,
            v & 0xffff_ffff_ffff
        )
    }
}

impl Serialize for Guid {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for Guid {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Guid, D::Error> {
        let text = String::deserialize(d)?;
        Guid::parse(&text).ok_or_else(|| serde::de::Error::custom("bad GUID"))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Plan {
    pub guid: Guid,
    pub name: String,
    pub active: bool,
}

/// The settings PowerForge shows, in plain language.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SettingKey {
    /// Turn off the display after (seconds, 0 = never).
    DisplayOff,
    /// Sleep after (seconds, 0 = never).
    SleepAfter,
    /// Hibernate after (seconds, 0 = never).
    HibernateAfter,
    /// Closing the lid (action index).
    LidAction,
    /// Power button (action index).
    PowerButton,
    /// Sleep button (action index).
    SleepButton,
    /// Minimum processor state (percent).
    CpuMin,
    /// Maximum processor state (percent).
    CpuMax,
    /// Allow wake timers (0 disable, 1 enable, 2 important only).
    WakeTimers,
    /// USB selective suspend (0 disabled, 1 enabled).
    UsbSuspend,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ValueKind {
    /// Seconds; shown as minutes, 0 = Never.
    Timeout,
    /// Percent 0..=100.
    Percent,
    /// 0 Do nothing, 1 Sleep, 2 Hibernate, 3 Shut down, 4 Turn off the display.
    Action,
    /// Named choices.
    Choice(&'static [&'static str]),
}

#[derive(Clone, Copy, Debug)]
pub struct SettingInfo {
    pub key: SettingKey,
    pub subgroup: Guid,
    pub setting: Guid,
    /// "Turn off the screen after"
    pub title: &'static str,
    /// One plain sentence.
    pub hint: &'static str,
    pub kind: ValueKind,
}

const SUB_VIDEO: Guid = Guid(0x7516b95f_f776_4464_8c53_06167f40cc99);
const SUB_SLEEP: Guid = Guid(0x238c9fa8_0aad_41ed_83f4_97be242c8f20);
const SUB_BUTTONS: Guid = Guid(0x4f971e89_eebd_4455_a8de_9e59040e7347);
const SUB_PROCESSOR: Guid = Guid(0x54533251_82be_4824_96c1_47b60b740d00);
const SUB_USB: Guid = Guid(0x2a737441_1930_4402_8d77_b2bebba308a3);

/// The action names of the lid and button settings, by index.
pub const ACTIONS: &[&str] = &[
    "Do nothing",
    "Sleep",
    "Hibernate",
    "Shut down",
    "Turn off the display",
];

impl SettingKey {
    pub const ALL: [SettingKey; 10] = [
        SettingKey::DisplayOff,
        SettingKey::SleepAfter,
        SettingKey::HibernateAfter,
        SettingKey::LidAction,
        SettingKey::PowerButton,
        SettingKey::SleepButton,
        SettingKey::CpuMin,
        SettingKey::CpuMax,
        SettingKey::WakeTimers,
        SettingKey::UsbSuspend,
    ];

    pub fn info(self) -> SettingInfo {
        let (subgroup, setting, title, hint, kind) = match self {
            SettingKey::DisplayOff => (
                SUB_VIDEO,
                Guid(0x3c0bc021_c8a8_4e07_a973_6b14cbcb2b7e),
                "Turn off the screen after",
                "How long the PC can sit unused before the screen goes dark.",
                ValueKind::Timeout,
            ),
            SettingKey::SleepAfter => (
                SUB_SLEEP,
                Guid(0x29f6c1db_86da_48c5_9fdb_f2b67b1f44da),
                "Put the PC to sleep after",
                "How long the PC can sit unused before it goes to sleep.",
                ValueKind::Timeout,
            ),
            SettingKey::HibernateAfter => (
                SUB_SLEEP,
                Guid(0x9d7815a6_7ee4_497e_8888_515a05f02364),
                "Hibernate after",
                "After this long asleep, the PC saves everything to disk and turns off, using no power.",
                ValueKind::Timeout,
            ),
            SettingKey::LidAction => (
                SUB_BUTTONS,
                Guid(0x5ca83367_6e45_459f_a27b_476b1d01c936),
                "When I close the lid",
                "What happens when you close a laptop's lid.",
                ValueKind::Action,
            ),
            SettingKey::PowerButton => (
                SUB_BUTTONS,
                Guid(0x7648efa3_dd9c_4e3e_b566_50f929386280),
                "When I press the power button",
                "What a short press of the power button does.",
                ValueKind::Action,
            ),
            SettingKey::SleepButton => (
                SUB_BUTTONS,
                Guid(0x96996bc0_ad50_47ec_923b_6f41874dd9eb),
                "When I press the sleep button",
                "Only keyboards and PCs with a sleep key have one.",
                ValueKind::Action,
            ),
            SettingKey::CpuMin => (
                SUB_PROCESSOR,
                Guid(0x893dee8e_2bef_41e0_89c6_b55d0929964c),
                "Lowest processor speed",
                "How far the processor may slow down when idle. Low values save power.",
                ValueKind::Percent,
            ),
            SettingKey::CpuMax => (
                SUB_PROCESSOR,
                Guid(0xbc5038f7_23e0_4960_96da_33abaf5935ec),
                "Highest processor speed",
                "Lowering it (for example to 99 %) turns off turbo boost: cooler and quieter, a little slower.",
                ValueKind::Percent,
            ),
            SettingKey::WakeTimers => (
                SUB_SLEEP,
                Guid(0xbd3b718a_0680_4d9d_8ab2_e1d2b4ac806d),
                "Let scheduled tasks wake the PC",
                "Wake timers let updates and tasks wake the PC from sleep at a set time.",
                ValueKind::Choice(&["No", "Yes", "Only important ones"]),
            ),
            SettingKey::UsbSuspend => (
                SUB_USB,
                Guid(0x48e6b7a6_50f5_4782_a5d4_53bb8f07e226),
                "Let idle USB devices power down",
                "Saves power; turn it off if a USB device keeps disconnecting.",
                ValueKind::Choice(&["No", "Yes"]),
            ),
        };
        SettingInfo {
            key: self,
            subgroup,
            setting,
            title,
            hint,
            kind,
        }
    }
}

/// A setting's value plugged in (AC) and on battery (DC).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct AcDc {
    pub ac: u32,
    pub dc: u32,
}

/// Plain text for a value: "Never", "15 minutes", "1 hour", "Sleep", "100 %".
pub fn value_label(kind: ValueKind, value: u32) -> String {
    match kind {
        ValueKind::Timeout => timeout_label(value),
        ValueKind::Percent => format!("{value} %"),
        ValueKind::Action => ACTIONS
            .get(value as usize)
            .map(|s| s.to_string())
            .unwrap_or_else(|| format!("Option {value}")),
        ValueKind::Choice(names) => names
            .get(value as usize)
            .map(|s| s.to_string())
            .unwrap_or_else(|| format!("Option {value}")),
    }
}

fn timeout_label(secs: u32) -> String {
    if secs == 0 {
        return "Never".into();
    }
    let plural = |n: u32, unit: &str| {
        if n == 1 {
            format!("1 {unit}")
        } else {
            format!("{n} {unit}s")
        }
    };
    if secs.is_multiple_of(3600) {
        plural(secs / 3600, "hour")
    } else if secs.is_multiple_of(60) {
        plural(secs / 60, "minute")
    } else if secs < 60 {
        plural(secs, "second")
    } else {
        format!("{} min {} s", secs / 60, secs % 60)
    }
}

/// Timeouts offered in the UI, in seconds (0 = never).
pub const TIMEOUT_CHOICES: &[u32] = &[
    60, 120, 180, 300, 600, 900, 1200, 1500, 1800, 2700, 3600, 7200, 10800, 18000, 0,
];

#[derive(Clone, Debug, PartialEq)]
pub enum PlanError {
    /// The plan does not exist (any more).
    NotFound,
    /// The active plan or a built-in plan cannot be deleted.
    NotAllowed(String),
    /// Windows refused (error code and text).
    System(String),
}

impl fmt::Display for PlanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PlanError::NotFound => write!(f, "that power plan no longer exists"),
            PlanError::NotAllowed(why) => write!(f, "{why}"),
            PlanError::System(e) => write!(f, "Windows refused: {e}"),
        }
    }
}

pub type PlanResult<T> = Result<T, PlanError>;

/// Everything PowerForge does with power plans.
pub trait PowerBackend: Send {
    fn list(&self) -> PlanResult<Vec<Plan>>;
    fn active(&self) -> PlanResult<Guid>;
    fn set_active(&mut self, plan: Guid) -> PlanResult<()>;
    /// Copies a plan under a new name; returns the new plan's GUID.
    fn duplicate(&mut self, plan: Guid, name: &str) -> PlanResult<Guid>;
    fn rename(&mut self, plan: Guid, name: &str) -> PlanResult<()>;
    /// Not the active plan, not a built-in plan.
    fn delete(&mut self, plan: Guid) -> PlanResult<()>;
    fn read(&self, plan: Guid, key: SettingKey) -> PlanResult<AcDc>;
    /// Writes one side (`ac` true = plugged in). Re-applies the plan when it
    /// is the active one so the change takes effect.
    fn write(&mut self, plan: Guid, key: SettingKey, ac: bool, value: u32) -> PlanResult<()>;
    /// Restores one plan's settings to the Windows defaults.
    fn restore_defaults(&mut self, plan: Guid) -> PlanResult<()>;
}

/// The checks both backends share.
fn check_name(name: &str) -> PlanResult<String> {
    let name = name.trim();
    if name.is_empty() {
        return Err(PlanError::NotAllowed("A plan needs a name.".into()));
    }
    if name.chars().count() > 120 {
        return Err(PlanError::NotAllowed("That name is too long.".into()));
    }
    Ok(name.to_string())
}

fn check_delete(plan: Guid, active: Guid) -> PlanResult<()> {
    if plan == active {
        return Err(PlanError::NotAllowed(
            "Switch to another plan before deleting this one.".into(),
        ));
    }
    if plan.is_builtin() {
        return Err(PlanError::NotAllowed(
            "Plans that come with Windows can't be deleted.".into(),
        ));
    }
    Ok(())
}

fn check_value(key: SettingKey, value: u32) -> PlanResult<()> {
    let ok = match key.info().kind {
        ValueKind::Timeout => value <= 7 * 86_400,
        ValueKind::Percent => value <= 100,
        ValueKind::Action => (value as usize) < ACTIONS.len(),
        ValueKind::Choice(names) => (value as usize) < names.len(),
    };
    if ok {
        Ok(())
    } else {
        Err(PlanError::NotAllowed(format!(
            "{value} is not a valid value."
        )))
    }
}

// ----------------------------------------------------------------------------------
// The real plans
// ----------------------------------------------------------------------------------

/// The real plans, through powrprof.
#[derive(Default)]
pub struct SystemPlans;

mod sys {
    use super::{Guid, PlanError, PlanResult};
    use windows::Win32::Foundation::{LocalFree, ERROR_FILE_NOT_FOUND, HLOCAL, WIN32_ERROR};
    use windows::Win32::System::Power::*;
    use windows::Win32::System::Registry::HKEY;

    pub fn check(e: WIN32_ERROR) -> PlanResult<()> {
        if e.0 == 0 {
            Ok(())
        } else if e == ERROR_FILE_NOT_FOUND {
            Err(PlanError::NotFound)
        } else {
            Err(PlanError::System(format!(
                "error {} ({})",
                e.0,
                windows::core::Error::from(e.to_hresult()).message()
            )))
        }
    }

    pub fn enumerate() -> Vec<Guid> {
        let mut out = Vec::new();
        for index in 0..256u32 {
            let mut g = windows::core::GUID::zeroed();
            let mut size = std::mem::size_of::<windows::core::GUID>() as u32;
            // SAFETY: the buffer is a GUID of the size passed.
            let e = unsafe {
                PowerEnumerate(
                    HKEY::default(),
                    None,
                    None,
                    ACCESS_SCHEME,
                    index,
                    Some(&mut g as *mut _ as *mut u8),
                    &mut size,
                )
            };
            if e.0 != 0 {
                break;
            }
            out.push(Guid::from_win(&g));
        }
        out
    }

    pub fn friendly_name(plan: Guid) -> PlanResult<String> {
        let g = plan.to_win();
        let mut size = 0u32;
        // SAFETY: a size query with no buffer.
        let e = unsafe {
            PowerReadFriendlyName(HKEY::default(), Some(&g), None, None, None, &mut size)
        };
        check(e)?;
        if size == 0 || size > 64 * 1024 {
            return Ok(String::new());
        }
        let mut buf = vec![0u16; (size as usize).div_ceil(2)];
        // SAFETY: the buffer holds `size` bytes.
        let e = unsafe {
            PowerReadFriendlyName(
                HKEY::default(),
                Some(&g),
                None,
                None,
                Some(buf.as_mut_ptr() as *mut u8),
                &mut size,
            )
        };
        check(e)?;
        Ok(crate::sys::from_wide(&buf))
    }

    pub fn write_name(plan: Guid, name: &str) -> PlanResult<()> {
        let g = plan.to_win();
        let bytes: Vec<u8> = crate::sys::wide(name)
            .iter()
            .flat_map(|u| u.to_le_bytes())
            .collect();
        // SAFETY: plain call with a NUL-terminated UTF-16 buffer.
        check(unsafe { PowerWriteFriendlyName(HKEY::default(), &g, None, None, &bytes) })
    }

    pub fn active() -> PlanResult<Guid> {
        let mut ptr: *mut windows::core::GUID = std::ptr::null_mut();
        // SAFETY: Windows allocates the GUID; it is freed with LocalFree.
        unsafe {
            check(PowerGetActiveScheme(HKEY::default(), &mut ptr))?;
            if ptr.is_null() {
                return Err(PlanError::NotFound);
            }
            let g = Guid::from_win(&*ptr);
            let _ = LocalFree(HLOCAL(ptr as _));
            Ok(g)
        }
    }

    pub fn set_active(plan: Guid) -> PlanResult<()> {
        let g = plan.to_win();
        // SAFETY: plain call.
        check(unsafe { PowerSetActiveScheme(HKEY::default(), Some(&g)) })
    }

    pub fn duplicate(plan: Guid) -> PlanResult<Guid> {
        let g = plan.to_win();
        let mut ptr: *mut windows::core::GUID = std::ptr::null_mut();
        // SAFETY: Windows allocates the new GUID; it is freed with LocalFree.
        unsafe {
            check(PowerDuplicateScheme(HKEY::default(), &g, &mut ptr))?;
            if ptr.is_null() {
                return Err(PlanError::System("no GUID returned".into()));
            }
            let new = Guid::from_win(&*ptr);
            let _ = LocalFree(HLOCAL(ptr as _));
            Ok(new)
        }
    }

    pub fn delete(plan: Guid) -> PlanResult<()> {
        let g = plan.to_win();
        // SAFETY: plain call.
        check(unsafe { PowerDeleteScheme(HKEY::default(), &g) })
    }

    pub fn read(plan: Guid, sub: Guid, setting: Guid, ac: bool) -> PlanResult<u32> {
        let (p, s, k) = (plan.to_win(), sub.to_win(), setting.to_win());
        let mut v = 0u32;
        // SAFETY: plain calls with an out-parameter.
        let e = unsafe {
            if ac {
                PowerReadACValueIndex(HKEY::default(), Some(&p), Some(&s), Some(&k), &mut v)
            } else {
                WIN32_ERROR(PowerReadDCValueIndex(
                    HKEY::default(),
                    Some(&p),
                    Some(&s),
                    Some(&k),
                    &mut v,
                ))
            }
        };
        check(e).map(|_| v)
    }

    pub fn write(plan: Guid, sub: Guid, setting: Guid, ac: bool, value: u32) -> PlanResult<()> {
        let (p, s, k) = (plan.to_win(), sub.to_win(), setting.to_win());
        // SAFETY: plain calls.
        let e = unsafe {
            if ac {
                PowerWriteACValueIndex(HKEY::default(), &p, Some(&s), Some(&k), value)
            } else {
                WIN32_ERROR(PowerWriteDCValueIndex(
                    HKEY::default(),
                    &p,
                    Some(&s),
                    Some(&k),
                    value,
                ))
            }
        };
        check(e)
    }

    pub fn restore(plan: Guid) -> PlanResult<()> {
        let g = plan.to_win();
        // SAFETY: plain call.
        check(unsafe { PowerRestoreIndividualDefaultPowerScheme(&g) })
    }
}

impl PowerBackend for SystemPlans {
    fn list(&self) -> PlanResult<Vec<Plan>> {
        let active = sys::active().ok();
        let mut plans: Vec<Plan> = sys::enumerate()
            .into_iter()
            .map(|guid| Plan {
                guid,
                name: sys::friendly_name(guid).unwrap_or_default(),
                active: Some(guid) == active,
            })
            .collect();
        for p in &mut plans {
            if p.name.trim().is_empty() {
                p.name = format!("Plan {}", &p.guid.to_string()[..8]);
            }
        }
        Ok(plans)
    }

    fn active(&self) -> PlanResult<Guid> {
        sys::active()
    }

    fn set_active(&mut self, plan: Guid) -> PlanResult<()> {
        if !sys::enumerate().contains(&plan) {
            return Err(PlanError::NotFound);
        }
        sys::set_active(plan)
    }

    fn duplicate(&mut self, plan: Guid, name: &str) -> PlanResult<Guid> {
        let name = check_name(name)?;
        let new = sys::duplicate(plan)?;
        if let Err(e) = sys::write_name(new, &name) {
            let _ = sys::delete(new);
            return Err(e);
        }
        Ok(new)
    }

    fn rename(&mut self, plan: Guid, name: &str) -> PlanResult<()> {
        let name = check_name(name)?;
        sys::write_name(plan, &name)
    }

    fn delete(&mut self, plan: Guid) -> PlanResult<()> {
        check_delete(plan, sys::active()?)?;
        sys::delete(plan)
    }

    fn read(&self, plan: Guid, key: SettingKey) -> PlanResult<AcDc> {
        let i = key.info();
        Ok(AcDc {
            ac: sys::read(plan, i.subgroup, i.setting, true)?,
            dc: sys::read(plan, i.subgroup, i.setting, false)?,
        })
    }

    fn write(&mut self, plan: Guid, key: SettingKey, ac: bool, value: u32) -> PlanResult<()> {
        check_value(key, value)?;
        let i = key.info();
        sys::write(plan, i.subgroup, i.setting, ac, value)?;
        if sys::active().ok() == Some(plan) {
            sys::set_active(plan)?;
        }
        Ok(())
    }

    fn restore_defaults(&mut self, plan: Guid) -> PlanResult<()> {
        sys::restore(plan)?;
        if sys::active().ok() == Some(plan) {
            sys::set_active(plan)?;
        }
        Ok(())
    }
}

// ----------------------------------------------------------------------------------
// The in-memory plans
// ----------------------------------------------------------------------------------

#[derive(Clone, Debug)]
struct MockPlan {
    guid: Guid,
    name: String,
    values: HashMap<SettingKey, AcDc>,
    defaults: HashMap<SettingKey, AcDc>,
}

/// An in-memory set of plans with Windows-like rules.
#[derive(Clone, Debug)]
pub struct MockPlans {
    plans: Vec<MockPlan>,
    active: Guid,
    next: u128,
    /// How many times a plan was made active (tests count switches).
    pub activations: u32,
}

impl Default for MockPlans {
    fn default() -> Self {
        MockPlans::new()
    }
}

fn typical_values(plan: Guid) -> HashMap<SettingKey, AcDc> {
    use SettingKey::*;
    let v = |ac, dc| AcDc { ac, dc };
    let (display, sleep, cpu_min, cpu_max) = match plan {
        Guid::HIGH_PERFORMANCE => (v(900, 600), v(0, 1800), v(100, 100), v(100, 100)),
        Guid::POWER_SAVER => (v(300, 120), v(900, 600), v(5, 5), v(100, 70)),
        _ => (v(600, 300), v(1800, 900), v(5, 5), v(100, 100)),
    };
    [
        (DisplayOff, display),
        (SleepAfter, sleep),
        (HibernateAfter, v(10800, 10800)),
        (LidAction, v(1, 1)),
        (PowerButton, v(1, 1)),
        (SleepButton, v(1, 1)),
        (CpuMin, cpu_min),
        (CpuMax, cpu_max),
        (WakeTimers, v(2, 2)),
        (UsbSuspend, v(1, 1)),
    ]
    .into_iter()
    .collect()
}

impl MockPlans {
    /// Balanced (active), High performance and Power saver with typical
    /// default values.
    pub fn new() -> MockPlans {
        let mut m = MockPlans {
            plans: Vec::new(),
            active: Guid::BALANCED,
            next: 1,
            activations: 0,
        };
        for (guid, name) in [
            (Guid::BALANCED, "Balanced"),
            (Guid::HIGH_PERFORMANCE, "High performance"),
            (Guid::POWER_SAVER, "Power saver"),
        ] {
            let values = typical_values(guid);
            m.plans.push(MockPlan {
                guid,
                name: name.into(),
                defaults: values.clone(),
                values,
            });
        }
        m
    }

    fn find(&self, plan: Guid) -> PlanResult<&MockPlan> {
        self.plans
            .iter()
            .find(|p| p.guid == plan)
            .ok_or(PlanError::NotFound)
    }

    fn find_mut(&mut self, plan: Guid) -> PlanResult<&mut MockPlan> {
        self.plans
            .iter_mut()
            .find(|p| p.guid == plan)
            .ok_or(PlanError::NotFound)
    }
}

impl PowerBackend for MockPlans {
    fn list(&self) -> PlanResult<Vec<Plan>> {
        Ok(self
            .plans
            .iter()
            .map(|p| Plan {
                guid: p.guid,
                name: p.name.clone(),
                active: p.guid == self.active,
            })
            .collect())
    }

    fn active(&self) -> PlanResult<Guid> {
        Ok(self.active)
    }

    fn set_active(&mut self, plan: Guid) -> PlanResult<()> {
        self.find(plan)?;
        self.active = plan;
        self.activations += 1;
        Ok(())
    }

    fn duplicate(&mut self, plan: Guid, name: &str) -> PlanResult<Guid> {
        let name = check_name(name)?;
        let source = self.find(plan)?.clone();
        let guid = Guid(0x5f0e_0000_0000_4000_8000_0000_0000_0000 | self.next);
        self.next += 1;
        self.plans.push(MockPlan {
            guid,
            name,
            ..source
        });
        Ok(guid)
    }

    fn rename(&mut self, plan: Guid, name: &str) -> PlanResult<()> {
        let name = check_name(name)?;
        self.find_mut(plan)?.name = name;
        Ok(())
    }

    fn delete(&mut self, plan: Guid) -> PlanResult<()> {
        self.find(plan)?;
        check_delete(plan, self.active)?;
        self.plans.retain(|p| p.guid != plan);
        Ok(())
    }

    fn read(&self, plan: Guid, key: SettingKey) -> PlanResult<AcDc> {
        Ok(self
            .find(plan)?
            .values
            .get(&key)
            .copied()
            .unwrap_or_default())
    }

    fn write(&mut self, plan: Guid, key: SettingKey, ac: bool, value: u32) -> PlanResult<()> {
        check_value(key, value)?;
        let p = self.find_mut(plan)?;
        let v = p.values.entry(key).or_default();
        if ac {
            v.ac = value;
        } else {
            v.dc = value;
        }
        Ok(())
    }

    fn restore_defaults(&mut self, plan: Guid) -> PlanResult<()> {
        let p = self.find_mut(plan)?;
        p.values = p.defaults.clone();
        Ok(())
    }
}

/// Finds a plan by GUID text or by name (case-insensitive, exact name first,
/// then unique prefix). For `--plan "Balanced"`.
pub fn find_plan(plans: &[Plan], query: &str) -> Option<Guid> {
    let q = query.trim();
    if let Some(g) = Guid::parse(q) {
        return plans.iter().find(|p| p.guid == g).map(|p| p.guid);
    }
    let lower = q.to_lowercase();
    if lower.is_empty() {
        return None;
    }
    if let Some(p) = plans.iter().find(|p| p.name.to_lowercase() == lower) {
        return Some(p.guid);
    }
    let matches: Vec<&Plan> = plans
        .iter()
        .filter(|p| p.name.to_lowercase().starts_with(&lower))
        .collect();
    (matches.len() == 1).then(|| matches[0].guid)
}

/// Demo plans: the three Windows plans plus one invented custom plan
/// ("Quiet work").
pub fn demo() -> MockPlans {
    let mut m = MockPlans::new();
    if let Ok(g) = m.duplicate(Guid::BALANCED, "Quiet work") {
        let _ = m.write(g, SettingKey::CpuMax, false, 80);
        let _ = m.write(g, SettingKey::CpuMax, true, 99);
        let _ = m.write(g, SettingKey::DisplayOff, false, 180);
        if let Ok(p) = m.find_mut(g) {
            p.defaults = typical_values(Guid::BALANCED);
        }
    }
    m.activations = 0;
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guid_parse_and_format() {
        let text = "381b4222-f694-41f0-9685-ff5bb260df2e";
        assert_eq!(Guid::parse(text), Some(Guid::BALANCED));
        assert_eq!(
            Guid::parse("{381B4222-F694-41F0-9685-FF5BB260DF2E}"),
            Some(Guid::BALANCED)
        );
        assert_eq!(Guid::BALANCED.to_string(), text);
        assert_eq!(Guid::parse("381b4222f69441f09685ff5bb260df2e"), None);
        assert_eq!(Guid::parse("381b4222-f694-41f0-9685-ff5bb260df2"), None);
        assert_eq!(Guid::parse("zzzzzzzz-f694-41f0-9685-ff5bb260df2e"), None);
        assert_eq!(Guid::parse("{381b4222-f694-41f0-9685-ff5bb260df2e"), None);
        let win = Guid::HIGH_PERFORMANCE.to_win();
        assert_eq!(Guid::from_win(&win), Guid::HIGH_PERFORMANCE);
        assert_eq!(
            format!("{win:?}").to_lowercase(),
            Guid::HIGH_PERFORMANCE.to_string()
        );
        assert!(Guid::POWER_SAVER.is_builtin());
        assert!(!Guid(42).is_builtin());
        let json = serde_json::to_string(&Guid::BALANCED).unwrap();
        assert_eq!(json, format!("\"{text}\""));
        assert_eq!(serde_json::from_str::<Guid>(&json).unwrap(), Guid::BALANCED);
        assert!(serde_json::from_str::<Guid>("\"nope\"").is_err());
    }

    #[test]
    fn labels() {
        assert_eq!(value_label(ValueKind::Timeout, 0), "Never");
        assert_eq!(value_label(ValueKind::Timeout, 60), "1 minute");
        assert_eq!(value_label(ValueKind::Timeout, 900), "15 minutes");
        assert_eq!(value_label(ValueKind::Timeout, 3600), "1 hour");
        assert_eq!(value_label(ValueKind::Timeout, 7200), "2 hours");
        assert_eq!(value_label(ValueKind::Timeout, 90), "1 min 30 s");
        assert_eq!(value_label(ValueKind::Action, 1), "Sleep");
        assert_eq!(value_label(ValueKind::Action, 9), "Option 9");
        assert_eq!(value_label(ValueKind::Percent, 99), "99 %");
        assert_eq!(
            SettingKey::WakeTimers.info().kind,
            ValueKind::Choice(&["No", "Yes", "Only important ones"])
        );
        assert_eq!(
            value_label(SettingKey::WakeTimers.info().kind, 2),
            "Only important ones"
        );
        // Every setting has a distinct GUID and a title.
        let mut seen = std::collections::HashSet::new();
        for k in SettingKey::ALL {
            let i = k.info();
            assert!(!i.title.is_empty() && !i.hint.is_empty());
            assert!(seen.insert(i.setting), "{k:?}");
        }
    }

    #[test]
    fn mock_plan_rules() {
        let mut m = MockPlans::new();
        assert_eq!(m.list().unwrap().len(), 3);
        assert_eq!(m.active().unwrap(), Guid::BALANCED);
        // Duplicate, rename, switch, delete.
        let g = m.duplicate(Guid::BALANCED, "  Travel  ").unwrap();
        let list = m.list().unwrap();
        assert_eq!(list.iter().find(|p| p.guid == g).unwrap().name, "Travel");
        assert_eq!(
            m.read(g, SettingKey::SleepAfter).unwrap(),
            m.read(Guid::BALANCED, SettingKey::SleepAfter).unwrap()
        );
        m.rename(g, "Trip").unwrap();
        assert!(m.rename(g, "   ").is_err());
        m.set_active(g).unwrap();
        assert!(matches!(m.delete(g), Err(PlanError::NotAllowed(_))));
        assert!(matches!(
            m.delete(Guid::POWER_SAVER),
            Err(PlanError::NotAllowed(_))
        ));
        m.set_active(Guid::BALANCED).unwrap();
        m.delete(g).unwrap();
        assert_eq!(m.delete(g), Err(PlanError::NotFound));
        assert_eq!(m.set_active(g), Err(PlanError::NotFound));
        assert_eq!(m.activations, 2);
    }

    #[test]
    fn mock_settings_and_restore() {
        let mut m = MockPlans::new();
        let before = m.read(Guid::BALANCED, SettingKey::DisplayOff).unwrap();
        m.write(Guid::BALANCED, SettingKey::DisplayOff, false, 60)
            .unwrap();
        let after = m.read(Guid::BALANCED, SettingKey::DisplayOff).unwrap();
        assert_eq!(
            after,
            AcDc {
                ac: before.ac,
                dc: 60
            }
        );
        assert!(m
            .write(Guid::BALANCED, SettingKey::CpuMax, true, 101)
            .is_err());
        assert!(m
            .write(Guid::BALANCED, SettingKey::LidAction, true, 7)
            .is_err());
        m.restore_defaults(Guid::BALANCED).unwrap();
        assert_eq!(
            m.read(Guid::BALANCED, SettingKey::DisplayOff).unwrap(),
            before
        );
        assert_eq!(
            m.read(Guid(7), SettingKey::DisplayOff),
            Err(PlanError::NotFound)
        );
    }

    #[test]
    fn finding_plans() {
        let m = demo();
        let plans = m.list().unwrap();
        assert_eq!(plans.len(), 4);
        assert_eq!(find_plan(&plans, "balanced"), Some(Guid::BALANCED));
        assert_eq!(find_plan(&plans, "High"), Some(Guid::HIGH_PERFORMANCE));
        assert_eq!(
            find_plan(&plans, Guid::POWER_SAVER.to_string().as_str()),
            Some(Guid::POWER_SAVER)
        );
        assert!(find_plan(&plans, "Quiet").is_some());
        assert_eq!(find_plan(&plans, ""), None);
        assert_eq!(find_plan(&plans, "nothing like it"), None);
        assert_eq!(find_plan(&plans, &Guid(5).to_string()), None);
    }

    #[test]
    fn real_plans_read_only() {
        // Read-only: lists the real plans and reads one setting. Nothing is changed.
        let sys = SystemPlans;
        if let Ok(active) = sys.active() {
            let plans = sys.list().unwrap();
            assert!(plans.iter().any(|p| p.guid == active && p.active));
            assert!(plans.iter().all(|p| !p.name.is_empty()));
            let _ = sys.read(active, SettingKey::SleepAfter);
        }
    }
}
