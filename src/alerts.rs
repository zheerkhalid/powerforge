//! Battery alerts: low battery, full charge, charge-limit and plug-in
//! reminders. Pure logic with hysteresis so an alert fires once per
//! crossing.

use crate::battery::{BatteryInfo, ChargeState};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AlertSettings {
    pub low_enabled: bool,
    pub low_pct: u8,
    pub full_enabled: bool,
    /// "Unplug at N %" reminder while charging.
    pub limit_enabled: bool,
    pub limit_pct: u8,
    /// "Plug in at N %" reminder (keeps the battery between the two).
    pub plug_enabled: bool,
    pub plug_pct: u8,
}

impl Default for AlertSettings {
    fn default() -> Self {
        AlertSettings {
            low_enabled: true,
            low_pct: 15,
            full_enabled: false,
            limit_enabled: false,
            limit_pct: 80,
            plug_enabled: false,
            plug_pct: 20,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Alert {
    Low(u8),
    Full,
    Limit(u8),
    Plug(u8),
}

impl Alert {
    pub fn title(&self) -> String {
        match self {
            Alert::Low(p) => format!("Battery at {p} %"),
            Alert::Full => "Battery fully charged".into(),
            Alert::Limit(p) => format!("Charged to {p} %: you can unplug"),
            Alert::Plug(p) => format!("Battery at {p} %: plug in"),
        }
    }

    pub fn text(&self) -> String {
        match self {
            Alert::Low(_) => "Plug in soon so you don't lose unsaved work.".into(),
            Alert::Full => "You can unplug the charger.".into(),
            Alert::Limit(_) => {
                "Unplugging now keeps the battery out of the top of its charge range, which slows wear."
                    .into()
            }
            Alert::Plug(_) => {
                "Plugging in now keeps the battery out of the bottom of its charge range, which slows wear."
                    .into()
            }
        }
    }
}

/// How far the charge must move back before the same alert can fire again.
const REARM: f32 = 3.0;

/// Remembers which alerts already fired.
#[derive(Clone, Debug, Default)]
pub struct AlertState {
    low: bool,
    full: bool,
    limit: bool,
    plug: bool,
}

impl AlertState {
    /// Alerts to show now for this reading.
    pub fn check(&mut self, s: &AlertSettings, info: &BatteryInfo) -> Vec<Alert> {
        let mut out = Vec::new();
        let pct = info.percent;
        let shown = pct.round().clamp(0.0, 100.0) as u8;
        let plugged = info.ac_online
            || matches!(
                info.state,
                ChargeState::Charging | ChargeState::Full | ChargeState::Idle
            );
        let discharging = !plugged;

        // Low battery.
        if plugged || pct > s.low_pct as f32 + REARM {
            self.low = false;
        }
        if s.low_enabled && discharging && pct <= s.low_pct as f32 && !self.low {
            self.low = true;
            out.push(Alert::Low(shown));
        }

        // Plug-in reminder (skipped when the low alert just fired).
        if plugged || pct > s.plug_pct as f32 + REARM {
            self.plug = false;
        }
        if s.plug_enabled && discharging && pct <= s.plug_pct as f32 && !self.plug {
            self.plug = true;
            if !out.iter().any(|a| matches!(a, Alert::Low(_))) {
                out.push(Alert::Plug(shown));
            }
        }

        // Charge-limit reminder.
        if !plugged || pct < s.limit_pct as f32 - REARM {
            self.limit = false;
        }
        if s.limit_enabled && plugged && pct >= s.limit_pct as f32 && !self.limit {
            self.limit = true;
            if s.limit_pct < 100 {
                out.push(Alert::Limit(shown));
            }
        }

        // Full charge.
        let full = info.state == ChargeState::Full || (plugged && pct >= 99.5);
        if !plugged || pct < 95.0 {
            self.full = false;
        }
        if s.full_enabled && full && !self.full {
            self.full = true;
            out.push(Alert::Full);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reading(pct: f32, state: ChargeState) -> BatteryInfo {
        BatteryInfo {
            percent: pct,
            state,
            ac_online: matches!(
                state,
                ChargeState::Charging | ChargeState::Full | ChargeState::Idle
            ),
            ..Default::default()
        }
    }

    #[test]
    fn low_fires_once_and_rearms() {
        let s = AlertSettings::default();
        let mut st = AlertState::default();
        assert!(st
            .check(&s, &reading(16.0, ChargeState::Discharging))
            .is_empty());
        assert_eq!(
            st.check(&s, &reading(15.0, ChargeState::Discharging)),
            vec![Alert::Low(15)]
        );
        assert!(st
            .check(&s, &reading(14.0, ChargeState::Discharging))
            .is_empty());
        assert!(st
            .check(&s, &reading(10.0, ChargeState::Discharging))
            .is_empty());
        // Small bounce does not rearm.
        assert!(st
            .check(&s, &reading(17.0, ChargeState::Discharging))
            .is_empty());
        assert!(st
            .check(&s, &reading(15.0, ChargeState::Discharging))
            .is_empty());
        // Plugging in rearms.
        assert!(st
            .check(&s, &reading(15.0, ChargeState::Charging))
            .is_empty());
        assert_eq!(
            st.check(&s, &reading(14.0, ChargeState::Discharging)),
            vec![Alert::Low(14)]
        );
        // Disabled: nothing.
        let off = AlertSettings {
            low_enabled: false,
            ..AlertSettings::default()
        };
        let mut st = AlertState::default();
        assert!(st
            .check(&off, &reading(5.0, ChargeState::Discharging))
            .is_empty());
    }

    #[test]
    fn limit_and_full() {
        let s = AlertSettings {
            limit_enabled: true,
            limit_pct: 80,
            full_enabled: true,
            ..AlertSettings::default()
        };
        let mut st = AlertState::default();
        assert!(st
            .check(&s, &reading(79.0, ChargeState::Charging))
            .is_empty());
        assert_eq!(
            st.check(&s, &reading(80.0, ChargeState::Charging)),
            vec![Alert::Limit(80)]
        );
        assert!(st
            .check(&s, &reading(85.0, ChargeState::Charging))
            .is_empty());
        assert_eq!(
            st.check(&s, &reading(100.0, ChargeState::Full)),
            vec![Alert::Full]
        );
        assert!(st.check(&s, &reading(100.0, ChargeState::Full)).is_empty());
        // Unplug and plug again above the limit: fires again.
        assert!(st
            .check(&s, &reading(99.0, ChargeState::Discharging))
            .is_empty());
        let again = st.check(&s, &reading(99.0, ChargeState::Charging));
        assert_eq!(again, vec![Alert::Limit(99)]);
        // Full fires on 99.5 % while plugged even if the driver says Idle.
        let mut st = AlertState::default();
        let out = st.check(&s, &reading(99.6, ChargeState::Idle));
        assert!(out.contains(&Alert::Full));
    }

    #[test]
    fn plug_reminder_and_texts() {
        let s = AlertSettings {
            plug_enabled: true,
            plug_pct: 20,
            ..AlertSettings::default()
        };
        let mut st = AlertState::default();
        assert_eq!(
            st.check(&s, &reading(20.0, ChargeState::Discharging)),
            vec![Alert::Plug(20)]
        );
        // Low still fires at 15 % even though the plug reminder already did.
        assert_eq!(
            st.check(&s, &reading(15.0, ChargeState::Discharging)),
            vec![Alert::Low(15)]
        );
        // Both thresholds crossed at once: only the low alert.
        let mut st = AlertState::default();
        assert_eq!(
            st.check(&s, &reading(12.0, ChargeState::Discharging)),
            vec![Alert::Low(12)]
        );
        assert_eq!(Alert::Low(15).title(), "Battery at 15 %");
        assert!(Alert::Limit(80).text().contains("wear"));
        assert!(!Alert::Full.text().is_empty());
    }
}
