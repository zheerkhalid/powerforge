//! Energy check: runs `powercfg /energy` (administrator, about a minute)
//! and explains the findings.

use super::app::{page_header, AdminAction, App, Dialog, Job};
use super::theme::{pal, semibold, Icon};
use super::widgets;
use crate::energy::Severity;
use crate::util;
use eframe::egui::{self, RichText, Ui};
use std::time::Instant;

impl App {
    pub(super) fn start_energy(&mut self, ctx: &egui::Context) {
        if self.demo {
            self.energy.job = Job::Idle;
            self.energy.demo_started = Some(Instant::now());
            return;
        }
        let duration = self.energy.duration;
        let progress = self.energy.progress.clone();
        if let Ok(mut p) = progress.lock() {
            *p = 0.0;
        }
        let ctx2 = ctx.clone();
        self.energy.job = Job::start(ctx, move || {
            crate::energy::run(duration, &|f| {
                if let Ok(mut p) = progress.lock() {
                    *p = f;
                }
                ctx2.request_repaint();
            })
        });
    }

    pub(super) fn energy_ui(&mut self, ui: &mut Ui) {
        let p = pal(ui);
        page_header(
            ui,
            "Energy check",
            "Windows watches the PC for a minute and lists what wastes power.",
            |_| {},
        );
        // The demo's pretend run.
        let mut demo_progress = None;
        if let Some(start) = self.energy.demo_started {
            let f = start.elapsed().as_secs_f32() / self.energy.duration as f32;
            if f >= 1.0 {
                self.energy.demo_started = None;
                self.energy.job = Job::Done(crate::energy::demo());
            } else {
                demo_progress = Some(f);
                ui.ctx()
                    .request_repaint_after(std::time::Duration::from_millis(200));
            }
        }
        let running = self.energy.job.running() || demo_progress.is_some();
        widgets::card(ui, |ui| {
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.label(RichText::new("Run the check").font(semibold(17.0)));
                    ui.label(RichText::new("Leave the PC as you normally use it, then start. Windows asks for administrator rights once. Nothing is changed.").color(p.muted));
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if widgets::primary_button(ui, if self.energy.job.value().is_some() { "Run again" } else { "Start" }, !running).clicked() {
                        self.dialog = Some(Dialog::Admin(
                            "Run the energy check?".into(),
                            format!("Windows records the PC's activity for {} seconds and writes a report, which PowerForge reads and then deletes. Windows asks for administrator rights.", self.energy.duration),
                            AdminAction::Energy,
                        ));
                    }
                    ui.add_enabled_ui(!running, |ui| {
                        egui::ComboBox::from_id_salt("energy-dur")
                            .width(100.0)
                            .selected_text(format!("{} s", self.energy.duration))
                            .show_ui(ui, |ui| {
                                for d in [30u32, 60, 120] {
                                    ui.selectable_value(&mut self.energy.duration, d, format!("{d} seconds"));
                                }
                            });
                    });
                });
            });
            if running {
                let f = demo_progress
                    .unwrap_or_else(|| self.energy.progress.lock().map(|v| *v).unwrap_or(0.0));
                ui.add_space(6.0);
                widgets::progress(ui, f, p.accent, 10.0);
                let left = ((1.0 - f) * self.energy.duration as f32).max(0.0) as u64;
                ui.label(
                    RichText::new(format!(
                        "Watching the PC... about {left} seconds left. Keep using it normally."
                    ))
                    .color(p.muted),
                );
            }
            if let Job::Failed(e) = &self.energy.job {
                ui.label(RichText::new(format!("The check did not run: {e}")).color(p.warn));
            }
        });
        ui.add_space(8.0);
        let Some(report) = self.energy.job.value().cloned() else {
            if !running {
                widgets::card(ui, |ui| {
                    widgets::empty_state(ui, Icon::LEAF, p.good, "No results yet", "The check finds things like USB devices that never rest, apps that keep the processor busy, and power settings that waste energy.");
                });
            }
            return;
        };
        widgets::card(ui, |ui| {
            ui.horizontal(|ui| {
                widgets::stat(
                    ui,
                    &report.count(Severity::Error).to_string(),
                    "Problems",
                    Some(p.danger),
                );
                ui.add_space(24.0);
                widgets::stat(
                    ui,
                    &report.count(Severity::Warning).to_string(),
                    "Worth a look",
                    Some(p.warn),
                );
                ui.add_space(24.0);
                widgets::stat(
                    ui,
                    &report.count(Severity::Info).to_string(),
                    "Information",
                    Some(p.muted),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if report.t > 0 {
                        ui.label(
                            RichText::new(format!(
                                "Checked {} for {} s",
                                util::fmt_datetime(report.t),
                                report.duration_secs
                            ))
                            .color(p.faint)
                            .size(12.5),
                        );
                    }
                });
            });
        });
        ui.add_space(8.0);
        for (i, f) in report.findings.iter().enumerate() {
            let color = match f.severity {
                Severity::Error => p.danger,
                Severity::Warning => p.warn,
                Severity::Info => p.info,
            };
            widgets::card(ui, |ui| {
                ui.horizontal(|ui| {
                    widgets::badge(ui, f.severity.label(), color);
                    let title = f.title.split(':').next_back().unwrap_or(&f.title).trim();
                    ui.label(RichText::new(title).font(semibold(15.0)));
                });
                ui.add(egui::Label::new(&f.plain).wrap());
                if !f.advice.is_empty() {
                    ui.label(
                        RichText::new(Icon::FORWARD.with(&f.advice))
                            .color(p.accent)
                            .size(13.5),
                    );
                }
                if !f.details.is_empty() {
                    let open = self.energy.expanded == Some(i);
                    if ui
                        .link(if open { "Hide details" } else { "Show details" })
                        .clicked()
                    {
                        self.energy.expanded = if open { None } else { Some(i) };
                    }
                    if open {
                        egui::Grid::new(("energy-details", i))
                            .num_columns(2)
                            .spacing(egui::vec2(18.0, 4.0))
                            .show(ui, |ui| {
                                for (n, v) in &f.details {
                                    ui.label(RichText::new(n).color(p.muted).size(13.0));
                                    ui.label(RichText::new(v).size(13.0));
                                    ui.end_row();
                                }
                            });
                    }
                }
            });
            ui.add_space(6.0);
        }
    }
}
