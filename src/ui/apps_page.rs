//! App power: an estimate of which apps use the most power, from their
//! processor, graphics and disk use.

use super::app::{page_header, App};
use super::theme::{pal, regular, semibold, Icon};
use super::widgets;
use crate::impact::Level;
use crate::util;
use eframe::egui::{self, pos2, vec2, Rect, RichText, Sense, Ui};

impl App {
    pub(super) fn apps_ui(&mut self, ui: &mut Ui) {
        let p = pal(ui);
        let ctx = ui.ctx().clone();
        self.ensure_impact(&ctx);
        page_header(
            ui,
            "App power",
            "Which apps use the most power right now (estimated).",
            |_| {},
        );
        widgets::card(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(Icon::INFO.glyph()).color(p.info));
                ui.add(egui::Label::new(RichText::new("Windows does not measure power per app. These figures are estimates from each app's share of processor, graphics and disk use, scaled to what the battery reports. Use them to compare apps, not as exact watts.").size(13.0).color(p.muted)).wrap());
            });
        });
        ui.add_space(8.0);
        let apps = self.impact.apps.clone();
        if apps.is_empty() {
            widgets::card(ui, |ui| {
                if self.impact.updated.is_none() {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("Measuring for a few seconds...");
                    });
                } else {
                    ui.label(RichText::new("Nothing measurable is running.").color(p.muted));
                }
            });
            return;
        }
        let total: f32 = apps.iter().map(|a| a.est_watts).sum();
        widgets::card_plain(ui, |ui| {
            let w = ui.available_width();
            let cols = [w - 520.0, 110.0, 90.0, 80.0, 80.0, 120.0];
            let header = ["App", "Impact", "Estimate", "Processor", "Graphics", "Disk"];
            let (rect, _) = ui.allocate_exact_size(vec2(w, 26.0), Sense::hover());
            let mut x = rect.left() + 8.0;
            for (c, h) in cols.iter().zip(header) {
                ui.painter().text(
                    pos2(x, rect.center().y),
                    egui::Align2::LEFT_CENTER,
                    h,
                    semibold(12.5),
                    p.muted,
                );
                x += c;
            }
            widgets::divider(ui);
            for a in apps.iter().take(40) {
                let (rect, resp) = ui.allocate_exact_size(vec2(w, 34.0), Sense::hover());
                if resp.hovered() {
                    ui.painter().rect_filled(rect, 6, p.hover);
                }
                let color = match a.level {
                    Level::VeryHigh => p.danger,
                    Level::High => p.warn,
                    Level::Moderate => p.accent,
                    Level::Low => p.info,
                    Level::VeryLow => p.faint,
                };
                let mut x = rect.left() + 8.0;
                let name = if a.processes > 1 {
                    format!("{}  ({} processes)", a.exe, a.processes)
                } else {
                    a.exe.clone()
                };
                let icon_rect =
                    Rect::from_center_size(pos2(x + 10.0, rect.center().y), vec2(20.0, 20.0));
                widgets::program_icon(ui, icon_rect, &a.exe, None);
                widgets::text_in(
                    ui,
                    Rect::from_min_max(
                        pos2(x + 28.0, rect.top()),
                        pos2(x + cols[0] - 8.0, rect.bottom()),
                    ),
                    &name,
                    regular(14.0),
                    p.text,
                );
                x += cols[0];
                widgets::paint_badge(
                    ui,
                    Rect::from_min_max(pos2(x, rect.top()), pos2(x + cols[1], rect.bottom())),
                    a.level.label(),
                    color,
                );
                x += cols[1];
                let share = if total > 0.0 {
                    a.est_watts / total * 100.0
                } else {
                    0.0
                };
                ui.painter().text(
                    pos2(x, rect.center().y),
                    egui::Align2::LEFT_CENTER,
                    if a.est_watts < 0.05 {
                        "< 0.1 W".to_string()
                    } else {
                        format!("{:.1} W", a.est_watts)
                    },
                    semibold(13.5),
                    p.text,
                );
                let _ = share;
                x += cols[2];
                ui.painter().text(
                    pos2(x, rect.center().y),
                    egui::Align2::LEFT_CENTER,
                    format!("{:.1} %", a.cpu_pct),
                    regular(13.0),
                    p.muted,
                );
                x += cols[3];
                let gpu = a
                    .gpu_pct
                    .map(|g| format!("{g:.0} %"))
                    .unwrap_or_else(|| "-".into());
                ui.painter().text(
                    pos2(x, rect.center().y),
                    egui::Align2::LEFT_CENTER,
                    gpu,
                    regular(13.0),
                    p.muted,
                );
                x += cols[4];
                ui.painter().text(
                    pos2(x, rect.center().y),
                    egui::Align2::LEFT_CENTER,
                    format!("{}/s", util::fmt_bytes(a.disk_bps as u64)),
                    regular(13.0),
                    p.muted,
                );
            }
        });
        if let Some(t) = self.impact.updated {
            ui.add_space(4.0);
            ui.label(
                RichText::new(format!(
                    "Updated {} seconds ago. Refreshes every 3 seconds while this page is open.",
                    t.elapsed().as_secs()
                ))
                .size(12.0)
                .color(p.faint),
            );
        }
    }
}
