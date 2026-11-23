//! Keep awake: one click, for N minutes, until a time, while an app runs,
//! or while a download or copy is active.

use super::app::{page_header, App};
use super::theme::{pal, semibold, Icon};
use super::widgets;
use crate::awake::{parse_duration, parse_until, AwakeMode};
use crate::engine::Cmd;
use crate::util;
use eframe::egui::{self, vec2, RichText, Ui};

impl App {
    pub(super) fn awake_ui(&mut self, ui: &mut Ui) {
        let p = pal(ui);
        page_header(
            ui,
            "Keep awake",
            "Stop the PC from sleeping on its own, for as long as you need.",
            |_| {},
        );
        let status = self.live.awake.clone();
        widgets::card(ui, |ui| {
            ui.horizontal(|ui| {
                let (color, icon) = if status.active {
                    (p.warn, Icon::SUNNY)
                } else {
                    (p.muted, Icon::MOON)
                };
                ui.label(RichText::new(icon.glyph()).size(34.0).color(color));
                ui.add_space(6.0);
                ui.vertical(|ui| {
                    ui.label(
                        RichText::new(if status.active {
                            "The PC stays awake"
                        } else {
                            "Off: the PC sleeps as usual"
                        })
                        .font(semibold(19.0)),
                    );
                    let detail = if status.active {
                        status.text.clone()
                    } else {
                        format!(
                            "It sleeps after the time set in the \u{201c}{}\u{201d} plan.",
                            self.live.active_plan_name
                        )
                    };
                    ui.label(RichText::new(detail).color(p.muted));
                    if let (Some(start), Some(end)) = (status.started, status.ends_at) {
                        let total = (end - start).max(1) as f32;
                        let done = (self.now() - start).clamp(0, end - start) as f32;
                        ui.add_space(4.0);
                        ui.scope(|ui| {
                            ui.set_max_width(420.0);
                            widgets::progress(ui, done / total, p.warn, 6.0);
                        });
                    }
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if status.active {
                        if widgets::danger_button(ui, "Stop", true).clicked() {
                            self.send(Cmd::StopAwake);
                        }
                    } else if widgets::primary_button(ui, "Keep awake now", true).clicked() {
                        self.start_awake(AwakeMode::Indefinite);
                    }
                });
            });
        });
        ui.add_space(8.0);
        widgets::card(ui, |ui| {
            widgets::setting_row(
                ui,
                "Keep the screen on too",
                "Off: the screen may still turn off, but the PC keeps running (downloads, music, remote sessions).",
                |ui| {
                    if widgets::toggle(ui, self.settings.awake_display, true).clicked() {
                        self.settings.awake_display = !self.settings.awake_display;
                    }
                },
            );
        });
        ui.add_space(8.0);
        ui.columns(2, |cols| {
            // For a while / until a time.
            widgets::card(&mut cols[0], |ui| {
                ui.set_min_height(206.0);
                widgets::section(ui, &Icon::CLOCK.with("For a while"));
                ui.horizontal_wrapped(|ui| {
                    for (label, mins) in [
                        ("15 min", 15),
                        ("30 min", 30),
                        ("1 hour", 60),
                        ("2 hours", 120),
                        ("4 hours", 240),
                    ] {
                        if widgets::chip(ui, label, None, false).clicked() {
                            let until = self.now() + mins * 60;
                            self.start_awake(AwakeMode::Until(until));
                        }
                    }
                });
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label("For");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.awake_form.minutes)
                            .desired_width(70.0)
                            .hint_text("90"),
                    );
                    let parsed = parse_duration(&self.awake_form.minutes);
                    ui.label(
                        RichText::new(
                            parsed
                                .map(|s| util::fmt_duration(s as u64))
                                .unwrap_or_else(|| "minutes, or 1h30m".into()),
                        )
                        .color(p.muted),
                    );
                    if widgets::ghost_button_enabled(ui, "Start", parsed.is_some()).clicked() {
                        if let Some(s) = parsed {
                            let until = self.now() + s as i64;
                            self.start_awake(AwakeMode::Until(until));
                            if let Ok(m) = self.awake_form.minutes.trim().parse::<u32>() {
                                self.settings.awake_minutes = m;
                            }
                        }
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("Until");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.awake_form.until)
                            .desired_width(70.0)
                            .hint_text("18:30"),
                    );
                    let at = parse_until(&self.awake_form.until, self.now());
                    ui.label(
                        RichText::new(
                            at.map(|t| format!("today or tomorrow at {}", util::fmt_time(t)))
                                .unwrap_or_else(|| "a time like 18:30 or 6pm".into()),
                        )
                        .color(p.muted),
                    );
                    if widgets::ghost_button_enabled(ui, "Start", at.is_some()).clicked() {
                        if let Some(t) = at {
                            self.start_awake(AwakeMode::Until(t));
                        }
                    }
                });
            });
            // While something happens.
            widgets::card(&mut cols[1], |ui| {
                ui.set_min_height(206.0);
                widgets::section(ui, &Icon::APPS.with("While something is happening"));
                ui.horizontal(|ui| {
                    ui.label("While");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.awake_form.app)
                            .desired_width(150.0)
                            .hint_text("backup.exe"),
                    );
                    ui.label("runs");
                    let exe = crate::autoswitch::normalize(&self.awake_form.app);
                    if widgets::ghost_button_enabled(ui, "Start", exe.len() > 4).clicked() {
                        self.start_awake(AwakeMode::WhileApp(exe));
                    }
                });
                ui.label(
                    RichText::new("Ends by itself when the app closes.")
                        .size(12.0)
                        .color(p.faint),
                );
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.label("While downloading or copying");
                    if widgets::ghost_button(ui, "Start").clicked() {
                        let net = self
                            .awake_form
                            .net_kbps
                            .trim()
                            .parse::<u32>()
                            .unwrap_or(self.settings.activity_net_kbps);
                        let disk = self
                            .awake_form
                            .disk_kbps
                            .trim()
                            .parse::<u32>()
                            .unwrap_or(self.settings.activity_disk_kbps);
                        self.settings.activity_net_kbps = net;
                        self.settings.activity_disk_kbps = disk;
                        self.start_awake(AwakeMode::WhileActivity {
                            net_bps: net as u64 * 1024,
                            disk_bps: disk as u64 * 1024,
                            grace_secs: self.settings.activity_grace_min.max(1) * 60,
                        });
                    }
                });
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Busy means network above").color(p.muted));
                    ui.add(
                        egui::TextEdit::singleline(&mut self.awake_form.net_kbps)
                            .desired_width(56.0),
                    );
                    ui.label(RichText::new("KB/s or disk above").color(p.muted));
                    ui.add(
                        egui::TextEdit::singleline(&mut self.awake_form.disk_kbps)
                            .desired_width(56.0),
                    );
                    ui.label(RichText::new("KB/s").color(p.muted));
                });
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Ends after").color(p.muted));
                    ui.add(
                        egui::DragValue::new(&mut self.settings.activity_grace_min)
                            .range(1..=60)
                            .suffix(" min"),
                    );
                    ui.label(RichText::new("of quiet.").color(p.muted));
                });
            });
        });
        ui.add_space(8.0);
        widgets::card(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(Icon::KEYBOARD.glyph()).size(20.0).color(p.accent));
                ui.add_space(4.0);
                let hotkey = if crate::tray::parse_hotkey(&self.settings.hotkey).is_some() {
                    format!("Press {} anywhere to turn keep-awake on or off, or use the icon near the clock.", self.settings.hotkey)
                } else {
                    "Use the icon near the clock to turn keep-awake on or off. A hotkey can be set in Settings.".to_string()
                };
                ui.label(hotkey);
            });
            ui.label(RichText::new("Keep-awake works like a media player: it asks Windows not to sleep (SetThreadExecutionState). Nothing in your power plan is changed, and the request ends when PowerForge exits.").size(12.0).color(p.faint));
        });
        let _ = vec2(0.0, 0.0);
    }
}
