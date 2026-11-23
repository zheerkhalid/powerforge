//! Overview: charge, state, time left, live rate graph, health at a glance,
//! and quick cards for plan, keep-awake and sleep.

use super::app::{page_header, App, Page};
use super::charts::{self, Axes, Series};
use super::theme::{pal, semibold, Icon};
use super::widgets;
use crate::awake::AwakeMode;
use crate::battery::ChargeState;
use crate::engine::Cmd;
use crate::health;
use crate::util;
use eframe::egui::{self, vec2, RichText, Ui};

impl App {
    pub(super) fn overview_ui(&mut self, ui: &mut Ui) {
        let p = pal(ui);
        page_header(
            ui,
            "Overview",
            "Your battery and power at a glance.",
            |_| {},
        );
        let battery = self.live.battery.clone();
        match &battery {
            Some(b) => {
                ui.columns(2, |cols| {
                    widgets::card(&mut cols[0], |ui| {
                        ui.set_min_height(214.0);
                        ui.horizontal(|ui| {
                            let color = match b.state {
                                ChargeState::Charging | ChargeState::Full | ChargeState::Idle => {
                                    p.info
                                }
                                _ if b.percent <= self.settings.alerts.low_pct as f32 => p.danger,
                                _ => p.good,
                            };
                            charts::ring(
                                ui,
                                180.0,
                                b.percent / 100.0,
                                color,
                                &format!("{:.0} %", b.percent),
                                b.state.label(),
                            );
                            ui.add_space(12.0);
                            ui.vertical(|ui| {
                                ui.add_space(14.0);
                                let left = match (b.state, b.seconds_left) {
                                    (ChargeState::Charging, Some(s)) => {
                                        format!("{} to full", util::fmt_duration(s as u64))
                                    }
                                    (ChargeState::Discharging, Some(s)) => {
                                        format!("{} left", util::fmt_duration(s as u64))
                                    }
                                    (ChargeState::Full, _) => "Full".to_string(),
                                    _ => "Estimating...".to_string(),
                                };
                                ui.label(RichText::new(left).font(semibold(24.0)));
                                ui.label(
                                    RichText::new(if b.ac_online {
                                        "Plugged in"
                                    } else {
                                        "Running on battery"
                                    })
                                    .color(p.muted),
                                );
                                ui.add_space(10.0);
                                if let Some(r) = b.rate_mw {
                                    let (word, color) = if r > 0 {
                                        ("Charging at", p.info)
                                    } else {
                                        ("Using", p.text)
                                    };
                                    ui.label(
                                        RichText::new(format!(
                                            "{word} {}",
                                            util::fmt_watts(r.abs() as i64)
                                        ))
                                        .font(semibold(16.0))
                                        .color(color),
                                    );
                                }
                                if let (Some(rem), Some(full)) = (b.remaining_mwh, b.full_mwh) {
                                    ui.label(
                                        RichText::new(format!(
                                            "{} of {}",
                                            util::fmt_wh(rem as u64),
                                            util::fmt_wh(full as u64)
                                        ))
                                        .color(p.muted),
                                    );
                                }
                                if b.saver_on {
                                    widgets::badge(ui, "Battery saver on", p.warn);
                                }
                            });
                        });
                    });
                    widgets::card(&mut cols[1], |ui| {
                        ui.set_min_height(214.0);
                        let h = health::health_percent(b.design_mwh, b.full_mwh);
                        ui.horizontal(|ui| {
                            widgets::section(ui, "Battery health");
                            if let Some(h) = h {
                                let g = health::grade(h);
                                let color = match g {
                                    health::Grade::Excellent | health::Grade::Good => p.good,
                                    health::Grade::Fair => p.warn,
                                    _ => p.danger,
                                };
                                widgets::badge(ui, g.label(), color);
                            }
                        });
                        ui.add_space(4.0);
                        egui::Grid::new("ov-stats")
                            .num_columns(3)
                            .spacing(vec2(26.0, 10.0))
                            .show(ui, |ui| {
                                widgets::stat(
                                    ui,
                                    &h.map(|h| format!("{h:.0} %")).unwrap_or_else(|| "-".into()),
                                    "Health",
                                    None,
                                );
                                widgets::stat(
                                    ui,
                                    &b.cycle_count
                                        .map(|c| c.to_string())
                                        .unwrap_or_else(|| "-".into()),
                                    "Charge cycles",
                                    None,
                                );
                                widgets::stat(
                                    ui,
                                    &b.full_mwh
                                        .map(|f| util::fmt_wh(f as u64))
                                        .unwrap_or_else(|| "-".into()),
                                    "Holds when full",
                                    None,
                                );
                                ui.end_row();
                                widgets::stat(
                                    ui,
                                    &b.design_mwh
                                        .map(|f| util::fmt_wh(f as u64))
                                        .unwrap_or_else(|| "-".into()),
                                    "When new",
                                    None,
                                );
                                widgets::stat(
                                    ui,
                                    &b.voltage_mv
                                        .map(|v| format!("{:.2} V", v as f32 / 1000.0))
                                        .unwrap_or_else(|| "-".into()),
                                    "Voltage",
                                    None,
                                );
                                widgets::stat(
                                    ui,
                                    &b.temperature_c
                                        .map(|t| format!("{t:.0} \u{b0}C"))
                                        .unwrap_or_else(|| "-".into()),
                                    "Temperature",
                                    None,
                                );
                                ui.end_row();
                            });
                        ui.add_space(4.0);
                        let chem = [
                            b.chemistry.as_str(),
                            b.manufacturer.as_str(),
                            b.name.as_str(),
                        ]
                        .iter()
                        .filter(|s| !s.is_empty())
                        .cloned()
                        .collect::<Vec<_>>()
                        .join("  \u{b7}  ");
                        if !chem.is_empty() {
                            ui.label(RichText::new(chem).color(p.muted).size(13.0));
                        }
                        if ui.link("See health over time").clicked() {
                            self.page = Page::Health;
                        }
                    });
                });
                ui.add_space(8.0);
                self.live_chart(ui);
            }
            None => {
                widgets::card(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(Icon::PLUG.glyph()).size(34.0).color(p.info));
                        ui.add_space(8.0);
                        ui.vertical(|ui| {
                            ui.label(RichText::new("No battery: this PC runs on mains power").font(semibold(18.0)));
                            ui.label(RichText::new("Power plans, keep-awake, sleep and wake diagnostics, the energy check and app power all work without a battery.").color(p.muted));
                        });
                    });
                });
            }
        }
        ui.add_space(8.0);
        ui.columns(3, |cols| {
            // Power plan.
            widgets::card(&mut cols[0], |ui| {
                ui.set_min_height(150.0);
                widgets::section(ui, &Icon::SLIDERS.with("Power plan"));
                ui.label(RichText::new(self.live.active_plan_name.clone()).font(semibold(20.0)));
                if let Some((t, text)) = &self.live.last_switch {
                    ui.label(
                        RichText::new(format!(
                            "Switched automatically at {}: {text}",
                            util::fmt_time(*t)
                        ))
                        .size(12.5)
                        .color(p.muted),
                    );
                } else if self.settings.auto.enabled {
                    ui.label(
                        RichText::new("Automatic switching is on.")
                            .size(12.5)
                            .color(p.muted),
                    );
                }
                ui.add_space(4.0);
                let plans: Vec<(crate::plans::Guid, String, bool)> = self
                    .plan_list
                    .iter()
                    .map(|p| (p.guid, p.name.clone(), p.active))
                    .collect();
                ui.horizontal_wrapped(|ui| {
                    for (g, name, active) in plans.iter().take(4) {
                        if widgets::chip(ui, name, None, *active).clicked() && !active {
                            self.activate_plan(*g);
                        }
                    }
                });
            });
            // Keep awake.
            widgets::card(&mut cols[1], |ui| {
                ui.set_min_height(150.0);
                widgets::section(ui, &Icon::SUNNY.with("Keep awake"));
                let awake = self.live.awake.clone();
                ui.label(
                    RichText::new(if awake.active { "On" } else { "Off" })
                        .font(semibold(20.0))
                        .color(if awake.active { p.warn } else { p.text }),
                );
                ui.label(
                    RichText::new(if awake.active {
                        awake.text.clone()
                    } else {
                        "The PC sleeps as set in the power plan.".into()
                    })
                    .size(12.5)
                    .color(p.muted),
                );
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if awake.active {
                        if widgets::ghost_button(ui, "Stop").clicked() {
                            self.send(Cmd::StopAwake);
                        }
                    } else {
                        if widgets::primary_button(ui, "Keep awake", true).clicked() {
                            self.start_awake(AwakeMode::Indefinite);
                        }
                        if widgets::ghost_button(ui, "1 hour").clicked() {
                            let until = self.now() + 3600;
                            self.start_awake(AwakeMode::Until(until));
                        }
                    }
                });
            });
            // Sleep.
            widgets::card(&mut cols[2], |ui| {
                ui.set_min_height(150.0);
                widgets::section(ui, &Icon::MOON.with("Sleep & wake"));
                let (title, text) = match &self.diag.snapshot.last_wake {
                    Some(w) => ("Last wake-up", w.plain.clone()),
                    None => (
                        "Why won't it sleep?",
                        "Find what keeps the PC awake and what wakes it up.".to_string(),
                    ),
                };
                ui.label(RichText::new(title).font(semibold(16.0)));
                ui.add(
                    egui::Label::new(RichText::new(text).size(12.5).color(p.muted))
                        .wrap()
                        .truncate(),
                );
                ui.add_space(4.0);
                if widgets::ghost_button(ui, "Open sleep diagnostics").clicked() {
                    self.page = Page::Sleep;
                }
            });
        });
    }

    fn live_chart(&mut self, ui: &mut Ui) {
        let p = pal(ui);
        let mode_id = egui::Id::new("live-mode");
        let mut mode: usize = ui.data(|d| d.get_temp(mode_id)).unwrap_or(0);
        widgets::card(ui, |ui| {
            ui.horizontal(|ui| {
                widgets::section(
                    ui,
                    if mode == 0 {
                        "Charge and discharge rate"
                    } else {
                        "Charge level"
                    },
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if let Some(m) = widgets::segmented(ui, &["Watts", "Percent"], mode) {
                        mode = m;
                    }
                });
            });
            let now = self.now();
            let span = 3 * 3600;
            let pts: Vec<(f64, f32)> = self
                .live
                .rate
                .iter()
                .filter(|r| now - r.t <= span)
                .map(|r| (r.t as f64, if mode == 0 { r.watts } else { r.percent }))
                .collect();
            if pts.len() < 2 {
                ui.add_space(30.0);
                ui.label(
                    RichText::new(
                        "The graph fills in while PowerForge runs (one point every 5 seconds).",
                    )
                    .color(p.muted),
                );
                ui.add_space(30.0);
                return;
            }
            let (lo, hi) = if mode == 0 {
                let lo = pts.iter().map(|x| x.1).fold(0.0f32, f32::min);
                let hi = pts.iter().map(|x| x.1).fold(0.0f32, f32::max);
                ((lo - 2.0).floor(), (hi + 2.0).ceil())
            } else {
                (0.0, 100.0)
            };
            let y_ticks = charts::ticks(lo, hi, 4);
            let x0 = (now - span) as f64;
            let x_ticks: Vec<(f64, String)> = (0..=3)
                .map(|i| {
                    let t = now - span + i * 3600;
                    (t as f64, util::fmt_time(t))
                })
                .collect();
            let unit_w = |v: f32| format!("{v:.0} W");
            let unit_p = |v: f32| format!("{v:.0} %");
            let hover_w = |t: f64, v: f32| {
                format!(
                    "{}  {}{:.1} W",
                    util::fmt_time(t as i64),
                    if v > 0.0 { "charging " } else { "using " },
                    v.abs()
                )
            };
            let hover_p = |t: f64, v: f32| format!("{}  {v:.0} %", util::fmt_time(t as i64));
            let color = if mode == 0 { p.accent } else { p.good };
            charts::line_chart(
                ui,
                "live",
                200.0,
                &[Series {
                    points: &pts,
                    color,
                    fill: true,
                }],
                &Axes {
                    x_range: (x0, now as f64),
                    y_range: (lo, hi),
                    y_ticks: &y_ticks,
                    y_label: if mode == 0 { &unit_w } else { &unit_p },
                    x_ticks: &x_ticks,
                    hover: if mode == 0 { &hover_w } else { &hover_p },
                    reference: if mode == 0 {
                        Some((0.0, "0 W"))
                    } else {
                        Some((self.settings.alerts.low_pct as f32, "low"))
                    },
                },
            );
            if mode == 0 {
                let used: Vec<f32> = pts.iter().filter(|x| x.1 < 0.0).map(|x| -x.1).collect();
                if !used.is_empty() {
                    let avg = used.iter().sum::<f32>() / used.len() as f32;
                    let max = used.iter().cloned().fold(0.0, f32::max);
                    ui.horizontal(|ui| {
                        charts::legend(
                            ui,
                            p.accent,
                            "Average use on battery",
                            &format!("{avg:.1} W"),
                        );
                        ui.add_space(18.0);
                        charts::legend(ui, p.danger, "Peak", &format!("{max:.1} W"));
                        if let Some(full) = self.live.battery.as_ref().and_then(|b| b.full_mwh) {
                            if let Some(s) = health::runtime_secs(full, (avg * 1000.0) as u32) {
                                ui.add_space(18.0);
                                charts::legend(
                                    ui,
                                    p.good,
                                    "A full charge at this rate lasts",
                                    &util::fmt_duration(s),
                                );
                            }
                        }
                    });
                }
            }
        });
        ui.data_mut(|d| d.insert_temp(mode_id, mode));
    }
}
