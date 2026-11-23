//! Settings: theme, tray, start with Windows, hotkey, history, data folder.

use super::app::{page_header, App};
use super::theme::{pal, Icon};
use super::widgets;
use crate::settings::ThemeChoice;
use eframe::egui::{self, RichText, Ui};

impl App {
    pub(super) fn settings_ui(&mut self, ui: &mut Ui) {
        let p = pal(ui);
        page_header(ui, "Settings", "How PowerForge looks and starts.", |_| {});
        widgets::card(ui, |ui| {
            widgets::section(ui, "Appearance");
            widgets::setting_row(ui, "Theme", "", |ui| {
                let current = match self.settings.theme {
                    ThemeChoice::System => 0,
                    ThemeChoice::Dark => 1,
                    ThemeChoice::Light => 2,
                };
                if let Some(i) = widgets::segmented(ui, &["System", "Dark", "Light"], current) {
                    self.settings.theme =
                        [ThemeChoice::System, ThemeChoice::Dark, ThemeChoice::Light][i];
                }
            });
            widgets::setting_row(
                ui,
                "Show the charge in the tray icon",
                "Off: the PowerForge icon is shown instead.",
                |ui| {
                    if widgets::toggle(ui, self.settings.tray_percent, true).clicked() {
                        self.settings.tray_percent = !self.settings.tray_percent;
                    }
                },
            );
        });
        ui.add_space(8.0);
        widgets::card(ui, |ui| {
            widgets::section(ui, "Starting and closing");
            widgets::setting_row(ui, "Start with Windows", "Starts in the notification area when you sign in, so reminders, auto switch and keep-awake work.", |ui| {
                if widgets::toggle(ui, self.startup, true).clicked() {
                    let on = !self.startup;
                    if self.demo {
                        self.startup = on;
                    } else {
                        match crate::sys::set_startup(crate::sys::RUN_VALUE, on) {
                            Ok(()) => self.startup = crate::sys::startup_enabled(crate::sys::RUN_VALUE),
                            Err(e) => self.toast(format!("Could not change it: {e}"), true),
                        }
                    }
                }
            });
            widgets::setting_row(ui, "Keep running when the window is closed", "PowerForge stays in the notification area. Choose Exit from its icon menu to quit.", |ui| {
                if widgets::toggle(ui, self.settings.close_to_tray, true).clicked() {
                    self.settings.close_to_tray = !self.settings.close_to_tray;
                }
            });
            widgets::setting_row(
                ui,
                "Keep-awake hotkey",
                "Turns keep-awake on and off from anywhere, e.g. Ctrl+Alt+K. Leave empty for none.",
                |ui| {
                    let valid = self.settings.hotkey.trim().is_empty()
                        || crate::tray::parse_hotkey(&self.settings.hotkey).is_some();
                    ui.add(
                        egui::TextEdit::singleline(&mut self.settings.hotkey)
                            .desired_width(140.0)
                            .hint_text("Ctrl+Alt+K"),
                    );
                    if !valid {
                        ui.label(
                            RichText::new("Needs Ctrl, Alt or Win plus a letter, digit or F-key")
                                .color(p.warn)
                                .size(12.0),
                        );
                    }
                },
            );
        });
        ui.add_space(8.0);
        widgets::card(ui, |ui| {
            widgets::section(ui, "History");
            widgets::setting_row(ui, "Record battery history", "Once a day the capacity, and every 5 minutes the charge, kept in the data folder for the health charts.", |ui| {
                if widgets::toggle(ui, self.settings.record_history, true).clicked() {
                    self.settings.record_history = !self.settings.record_history;
                }
            });
            let dir = self.shared.dir.clone();
            widgets::setting_row(ui, "Data folder", &dir.display().to_string(), |ui| {
                if widgets::ghost_button(ui, &Icon::FOLDER_OPEN.with("Open")).clicked()
                    && !self.demo
                {
                    let _ = std::fs::create_dir_all(&dir);
                    crate::sys::open_path(&dir);
                }
                if widgets::ghost_button(ui, &Icon::EXPORT.with("Charge CSV")).clicked() {
                    let text = crate::export::csv_charge(&self.history());
                    self.save_file(&text, "csv", "PowerForge charge history");
                }
            });
        });
        ui.add_space(8.0);
        widgets::card(ui, |ui| {
            widgets::section(ui, "Command line");
            for (cmd, what) in [
                (
                    "PowerForge.exe --awake 30m",
                    "keep the PC awake for 30 minutes (also 2h, 18:30, on, off)",
                ),
                (
                    "PowerForge.exe --plan \"Balanced\"",
                    "switch to a power plan",
                ),
                (
                    "PowerForge.exe --report",
                    "write an HTML battery and power report",
                ),
            ] {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(cmd).monospace().color(p.accent));
                    ui.label(RichText::new(what).color(p.muted));
                });
            }
        });
    }
}
