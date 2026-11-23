//! User settings, stored as `settings.json` in the data folder
//! (%LOCALAPPDATA%\PowerForge).

use crate::alerts::AlertSettings;
use crate::autoswitch::AutoSettings;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ThemeChoice {
    #[default]
    System,
    Dark,
    Light,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub theme: ThemeChoice,
    /// Closing the window keeps PowerForge in the notification area.
    pub close_to_tray: bool,
    /// Show the charge in the tray icon (otherwise the app icon).
    pub tray_percent: bool,
    /// Global hotkey that toggles keep-awake, e.g. "Ctrl+Alt+K". Empty = none.
    pub hotkey: String,
    /// Keep-awake from the tray / hotkey also keeps the screen on.
    pub awake_display: bool,
    /// Minutes offered first on the Keep awake page.
    pub awake_minutes: u32,
    /// Keep-awake while activity: thresholds in KB/s and grace in minutes.
    pub activity_net_kbps: u32,
    pub activity_disk_kbps: u32,
    pub activity_grace_min: u32,
    pub alerts: AlertSettings,
    pub auto: AutoSettings,
    /// Record a daily capacity point and charge samples.
    pub record_history: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            theme: ThemeChoice::System,
            close_to_tray: true,
            tray_percent: true,
            hotkey: "Ctrl+Alt+K".into(),
            awake_display: true,
            awake_minutes: 60,
            activity_net_kbps: 200,
            activity_disk_kbps: 1000,
            activity_grace_min: 3,
            alerts: AlertSettings::default(),
            auto: AutoSettings::default(),
            record_history: true,
        }
    }
}

/// %LOCALAPPDATA%\PowerForge, or `POWERFORGE_DATA` when set.
pub fn default_data_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("POWERFORGE_DATA") {
        return PathBuf::from(dir);
    }
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("PowerForge")
}

impl Settings {
    pub fn load(dir: &Path) -> Settings {
        std::fs::read(dir.join("settings.json"))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;
        let tmp = dir.join("settings.json.tmp");
        let text = serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?;
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, dir.join("settings.json"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_defaults() {
        let dir = std::env::temp_dir().join(format!("powerforge-settings-{}", std::process::id()));
        let mut s = Settings::default();
        s.alerts.limit_enabled = true;
        s.hotkey = "Ctrl+Shift+A".into();
        s.save(&dir).unwrap();
        assert_eq!(Settings::load(&dir), s);
        // Unknown and missing fields fall back to defaults.
        std::fs::write(dir.join("settings.json"), br#"{"theme":"Dark","bogus":1}"#).unwrap();
        let loaded = Settings::load(&dir);
        assert_eq!(loaded.theme, ThemeChoice::Dark);
        assert!(loaded.close_to_tray);
        assert!(!loaded.auto.enabled);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
