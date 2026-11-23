//! Battery health: health over time (local records plus the Windows battery
//! report), wear trend, runtime estimates, recent use, charge reminders and
//! exports.

use super::app::{page_header, App, Job};
use super::charts::{self, Axes, Series};
use super::theme::{pal, semibold, Icon};
use super::widgets;
use crate::batteryreport;
use crate::export::{self, ExportInput};
use crate::health;
use crate::util;
use eframe::egui::{self, vec2, RichText, Ui};

impl App {
    pub(super) fn health_ui(&mut self, ui: &mut Ui) {
        let p = pal(ui);
        let ctx = ui.ctx().clone();
        self.ensure_report(&ctx);
        let mut export_html = false;
        let mut export_csv = false;
        page_header(
            ui,
            "Battery health",
            "How much charge your battery still holds, and how fast it wears.",
            |ui| {
                if widgets::ghost_button(ui, &Icon::EXPORT.with("CSV"))
                    .on_hover_text("Capacity history as CSV")
                    .clicked()
                {
                    export_csv = true;
                }
                if widgets::ghost_button(ui, &Icon::DOCUMENT.with("HTML report")).clicked() {
                    export_html = true;
                }
            },
        );
        let Some(b) = self.live.battery.clone() else {
            widgets::card(ui, |ui| {
                widgets::empty_state(ui, Icon::PLUG, p.info, "No battery", "This PC has no battery, so there is no battery health to show. Everything else in PowerForge works.");
            });
            return;
        };
        let history = self.history();
        let report = self.report.value().cloned();
        let points = history.health_points(report.as_ref());
        let trend = health::wear_trend(&points);
        let h = health::health_percent(b.design_mwh, b.full_mwh);

        ui.columns(2, |cols| {
            widgets::card(&mut cols[0], |ui| {
                ui.set_min_height(170.0);
                match h {
                    Some(h) => {
                        let g = health::grade(h);
                        let color = match g {
                            health::Grade::Excellent | health::Grade::Good => p.good,
                            health::Grade::Fair => p.warn,
                            _ => p.danger,
                        };
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(format!("{h:.0} %")).font(semibold(40.0)).color(color));
                            ui.add_space(6.0);
                            ui.vertical(|ui| {
                                ui.add_space(8.0);
                                widgets::badge(ui, g.label(), color);
                                ui.label(RichText::new("of the original capacity").color(p.muted));
                            });
                        });
                        widgets::progress(ui, h / 100.0, color, 8.0);
                        ui.add_space(4.0);
                        ui.add(egui::Label::new(health::explain(h, b.cycle_count)).wrap());
                    }
                    None => {
                        ui.label(RichText::new("Health unknown").font(semibold(22.0)));
                        ui.label(RichText::new("The battery does not report its capacities in milliwatt-hours.").color(p.muted));
                    }
                }
            });
            widgets::card(&mut cols[1], |ui| {
                ui.set_min_height(170.0);
                widgets::section(ui, "Wear, and how long a full charge lasts");
                match &trend {
                    Some(t) => {
                        ui.add(egui::Label::new(health::explain_trend(t)).wrap());
                    }
                    None => {
                        ui.label(RichText::new("PowerForge records the capacity once a day. A trend appears after two weeks, or right away when the Windows battery report has history.").color(p.muted));
                    }
                }
                ui.add_space(6.0);
                egui::Grid::new("life").num_columns(3).spacing(vec2(24.0, 8.0)).show(ui, |ui| {
                    let rt = report.as_ref().and_then(|r| r.runtime_full_secs);
                    widgets::stat(ui, &rt.map(util::fmt_duration).unwrap_or_else(|| "-".into()), "Windows' estimate", None);
                    let drain = history.average_drain_mw(self.now(), 7);
                    let mine = drain.and_then(|d| health::runtime_secs(b.full_mwh.unwrap_or(0), d));
                    widgets::stat(ui, &mine.map(util::fmt_duration).unwrap_or_else(|| "-".into()), "At your use (7 days)", None);
                    let new = report.as_ref().and_then(|r| r.runtime_design_secs);
                    widgets::stat(ui, &new.map(util::fmt_duration).unwrap_or_else(|| "-".into()), "When new", None);
                    ui.end_row();
                });
            });
        });
        ui.add_space(8.0);

        // Health over time.
        widgets::card(ui, |ui| {
            ui.horizontal(|ui| {
                widgets::section(ui, "Health over time");
                match &self.report {
                    Job::Running(..) => {
                        ui.spinner();
                        ui.label(
                            RichText::new("Reading the Windows battery report...")
                                .color(p.muted)
                                .size(12.5),
                        );
                    }
                    Job::Failed(e) => {
                        ui.label(
                            RichText::new(format!("Battery report unavailable: {e}"))
                                .color(p.warn)
                                .size(12.5),
                        );
                    }
                    _ => {}
                }
            });
            if points.len() < 2 {
                ui.label(RichText::new("Not enough history yet.").color(p.muted));
                return;
            }
            let pts: Vec<(f64, f32)> = points.iter().map(|q| (q.t as f64, q.health_pct)).collect();
            let lo = pts.iter().map(|x| x.1).fold(100.0f32, f32::min).min(80.0);
            let lo = ((lo - 2.0) / 5.0).floor() * 5.0;
            let hi = pts.iter().map(|x| x.1).fold(100.0f32, f32::max).max(100.0);
            let (x0, x1) = (pts[0].0, pts[pts.len() - 1].0);
            let y_ticks = charts::ticks(lo, hi, 4);
            let x_ticks: Vec<(f64, String)> = (0..=4)
                .map(|i| {
                    let t = x0 + (x1 - x0) * i as f64 / 4.0;
                    (t, util::fmt_month(t as i64))
                })
                .collect();
            let cycles = points.clone();
            let hover = move |t: f64, v: f32| {
                let c = cycles
                    .iter()
                    .min_by_key(|q| (q.t - t as i64).abs())
                    .and_then(|q| q.cycles)
                    .map(|c| format!(", {c} cycles"))
                    .unwrap_or_default();
                format!("{}  {v:.1} %{c}", util::fmt_day(t as i64))
            };
            let label = |v: f32| format!("{v:.0} %");
            charts::line_chart(
                ui,
                "health",
                210.0,
                &[Series {
                    points: &pts,
                    color: p.good,
                    fill: true,
                }],
                &Axes {
                    x_range: (x0, x1),
                    y_range: (lo, hi),
                    y_ticks: &y_ticks,
                    y_label: &label,
                    x_ticks: &x_ticks,
                    hover: &hover,
                    reference: Some((80.0, "80 %")),
                },
            );
            ui.label(RichText::new(format!("{} measurements: PowerForge's daily records plus the history Windows keeps in its battery report.", points.len())).size(12.0).color(p.faint));
        });
        ui.add_space(8.0);

        ui.columns(2, |cols| {
            // Recent use.
            widgets::card(&mut cols[0], |ui| {
                ui.set_min_height(190.0);
                widgets::section(ui, "The last 7 days");
                match &report {
                    Some(r) => {
                        let (dc, ac, asleep) = batteryreport::usage_split(r, self.now(), 7);
                        let total = (dc + ac + asleep).max(1) as f32;
                        charts::stacked_bar(ui, &[(dc as f32 / total, p.accent), (ac as f32 / total, p.info), (asleep as f32 / total, p.faint)], 14.0);
                        ui.add_space(4.0);
                        charts::legend(ui, p.accent, "In use on battery", &util::fmt_duration(dc));
                        charts::legend(ui, p.info, "In use plugged in", &util::fmt_duration(ac));
                        charts::legend(ui, p.faint, "Asleep or in standby", &util::fmt_duration(asleep));
                        if let Some(s) = batteryreport::standby_summary(r) {
                            ui.add_space(4.0);
                            ui.add(egui::Label::new(RichText::new(s.verdict).size(13.0).color(p.muted)).wrap());
                        }
                    }
                    None => {
                        ui.label(RichText::new("Appears when the Windows battery report has been read.").color(p.muted));
                    }
                }
            });
            // Charge reminders.
            widgets::card(&mut cols[1], |ui| {
                ui.set_min_height(190.0);
                widgets::section(ui, "Charge reminders");
                let a = &mut self.settings.alerts;
                reminder(ui, "Low battery", "Warn me at", &mut a.low_enabled, &mut a.low_pct, 5..=50);
                reminder(ui, "Unplug reminder", "Remind me to unplug at", &mut a.limit_enabled, &mut a.limit_pct, 50..=100);
                reminder(ui, "Plug-in reminder", "Remind me to plug in at", &mut a.plug_enabled, &mut a.plug_pct, 5..=60);
                ui.horizontal(|ui| {
                    if widgets::toggle(ui, a.full_enabled, true).clicked() {
                        a.full_enabled = !a.full_enabled;
                    }
                    ui.label("Tell me when fully charged");
                });
                ui.label(RichText::new("Keeping the charge between about 20 and 80 % slows battery wear. Reminders appear near the clock.").size(12.0).color(p.faint));
            });
        });

        // Charge history.
        let samples = history.recent_charge(self.now(), 7 * 86_400);
        if samples.len() >= 2 {
            ui.add_space(8.0);
            widgets::card(ui, |ui| {
                widgets::section(ui, "Charge level, last 7 days");
                let pts: Vec<(f64, f32)> =
                    samples.iter().map(|s| (s.t as f64, s.percent)).collect();
                let now = self.now();
                let x_ticks: Vec<(f64, String)> = (0..=7)
                    .map(|d| {
                        let t = now - (7 - d) * 86_400;
                        (
                            t as f64,
                            util::fmt_day(t)
                                .split(' ')
                                .take(2)
                                .collect::<Vec<_>>()
                                .join(" "),
                        )
                    })
                    .collect();
                let label = |v: f32| format!("{v:.0} %");
                let hover = |t: f64, v: f32| format!("{}  {v:.0} %", util::fmt_datetime(t as i64));
                charts::line_chart(
                    ui,
                    "charge7",
                    170.0,
                    &[Series {
                        points: &pts,
                        color: p.info,
                        fill: true,
                    }],
                    &Axes {
                        x_range: ((now - 7 * 86_400) as f64, now as f64),
                        y_range: (0.0, 100.0),
                        y_ticks: &[0.0, 50.0, 100.0],
                        y_label: &label,
                        x_ticks: &x_ticks,
                        hover: &hover,
                        reference: self
                            .settings
                            .alerts
                            .limit_enabled
                            .then_some((self.settings.alerts.limit_pct as f32, "unplug")),
                    },
                );
            });
        }

        if export_html || export_csv {
            let input = ExportInput {
                now: self.now(),
                battery: Some(&b),
                report: report.as_ref(),
                history: &history,
                diag: Some(&self.diag.snapshot),
                energy: self.energy.job.value(),
                plan: Some(&self.live.active_plan_name),
            };
            let (text, ext, name) = if export_html {
                (export::html(&input), "html", "PowerForge report")
            } else {
                (
                    export::csv_capacity(&input),
                    "csv",
                    "PowerForge capacity history",
                )
            };
            self.save_file(&text, ext, name);
        }
    }

    /// Asks where to save and writes the file (the demo writes nothing).
    pub(super) fn save_file(&mut self, text: &str, ext: &str, name: &str) {
        if self.demo {
            self.toast("The demo does not write files.", false);
            return;
        }
        let file = rfd::FileDialog::new()
            .set_file_name(format!("{name} {}.{ext}", util::fmt_date(self.now())))
            .add_filter(ext.to_uppercase(), &[ext])
            .save_file();
        if let Some(path) = file {
            match std::fs::write(&path, text) {
                Ok(()) => {
                    self.toast(format!("Saved {}", path.display()), false);
                    if ext == "html" {
                        crate::sys::open_path(&path);
                    }
                }
                Err(e) => self.toast(format!("Could not save: {e}"), true),
            }
        }
    }
}

fn reminder(
    ui: &mut Ui,
    title: &str,
    label: &str,
    on: &mut bool,
    pct: &mut u8,
    range: std::ops::RangeInclusive<u8>,
) {
    let p = pal(ui);
    ui.horizontal(|ui| {
        if widgets::toggle(ui, *on, true)
            .on_hover_text(title)
            .clicked()
        {
            *on = !*on;
        }
        ui.label(RichText::new(label).color(if *on { p.text } else { p.muted }));
        ui.add_enabled(
            *on,
            egui::DragValue::new(pct)
                .range(range)
                .suffix(" %")
                .speed(0.3),
        );
    });
}
