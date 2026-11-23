//! Power plans: list, switch, duplicate, rename, delete, restore defaults,
//! and the key settings in plain language.

use super::app::{page_header, App, Dialog};
use super::theme::{pal, semibold, Icon};
use super::widgets;
use crate::plans::{value_label, Guid, SettingKey, ValueKind, ACTIONS, TIMEOUT_CHOICES};
use eframe::egui::{self, vec2, Align, CornerRadius, Layout, RichText, Sense, Stroke, Ui};

impl App {
    pub(super) fn plans_ui(&mut self, ui: &mut Ui) {
        let p = pal(ui);
        let mut refresh = false;
        page_header(
            ui,
            "Power plans",
            "Choose how your PC balances speed, battery life and noise.",
            |ui| {
                if widgets::icon_button(ui, Icon::REFRESH, "Read the plans again").clicked() {
                    refresh = true;
                }
            },
        );
        if refresh {
            self.refresh_plans();
        }
        let list = self.plan_list.clone();
        let width = ui.available_width();
        ui.horizontal_top(|ui| {
            // The list.
            ui.allocate_ui_with_layout(vec2(290.0, 10.0), Layout::top_down(Align::Min), |ui| {
                ui.set_width(290.0);
                for plan in &list {
                    let selected = self.plan_sel == Some(plan.guid);
                    let (rect, resp) = ui.allocate_exact_size(vec2(290.0, 62.0), Sense::click());
                    let fill = if selected { p.selected } else if resp.hovered() { p.hover } else { p.card };
                    ui.painter().rect_filled(rect, CornerRadius::same(10), fill);
                    ui.painter().rect_stroke(
                        rect,
                        CornerRadius::same(10),
                        Stroke::new(1.0_f32, if selected { p.accent.gamma_multiply(0.7) } else { p.border }),
                        egui::StrokeKind::Inside,
                    );
                    let radio = egui::pos2(rect.left() + 22.0, rect.center().y);
                    ui.painter().circle_stroke(radio, 8.0, Stroke::new(1.5_f32, if plan.active { p.accent } else { p.faint }));
                    if plan.active {
                        ui.painter().circle_filled(radio, 4.5, p.accent);
                    }
                    let text_rect = egui::Rect::from_min_max(egui::pos2(rect.left() + 42.0, rect.top() + 10.0), egui::pos2(rect.right() - 10.0, rect.top() + 32.0));
                    widgets::text_in(ui, text_rect, &plan.name, semibold(15.0), p.text);
                    let sub = if plan.active {
                        "In use".to_string()
                    } else if plan.guid.is_builtin() {
                        "Comes with Windows".to_string()
                    } else {
                        "Your own plan".to_string()
                    };
                    let sub_rect = egui::Rect::from_min_max(egui::pos2(rect.left() + 42.0, rect.top() + 32.0), egui::pos2(rect.right() - 10.0, rect.bottom() - 8.0));
                    widgets::text_in(ui, sub_rect, &sub, super::theme::regular(12.5), if plan.active { p.accent } else { p.muted });
                    if resp.clicked() {
                        self.plan_sel = Some(plan.guid);
                        self.load_plan_values();
                    }
                    if resp.double_clicked() && !plan.active {
                        self.activate_plan(plan.guid);
                    }
                }
                ui.add_space(6.0);
                ui.label(RichText::new("Modern Standby laptops often have only Balanced. Duplicate it to make your own plans.").size(12.0).color(p.faint));
            });
            ui.add_space(14.0);
            let Some(sel) = self.plan_sel else {
                return;
            };
            let Some(plan) = list.iter().find(|x| x.guid == sel).cloned() else {
                return;
            };
            ui.allocate_ui_with_layout(vec2(width - 312.0, 10.0), Layout::top_down(Align::Min), |ui| {
                widgets::card(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(&plan.name).font(semibold(20.0)));
                        if plan.active {
                            widgets::badge(ui, "In use", p.accent);
                        }
                    });
                    ui.add_space(4.0);
                    ui.horizontal_wrapped(|ui| {
                        if !plan.active && widgets::primary_button(ui, "Use this plan", true).clicked() {
                            self.activate_plan(plan.guid);
                        }
                        if widgets::ghost_button(ui, &Icon::ADD.with("Duplicate")).clicked() {
                            self.dialog = Some(Dialog::Duplicate(plan.guid, format!("{} (copy)", plan.name)));
                        }
                        if widgets::ghost_button(ui, &Icon::EDIT.with("Rename")).clicked() {
                            self.dialog = Some(Dialog::Rename(plan.guid, plan.name.clone()));
                        }
                        if widgets::ghost_button(ui, &Icon::UNDO.with("Restore defaults")).clicked() {
                            self.dialog = Some(Dialog::Restore(plan.guid, plan.name.clone()));
                        }
                        let can_delete = !plan.active && !plan.guid.is_builtin();
                        if widgets::ghost_button_enabled(ui, &Icon::DELETE.with("Delete"), can_delete)
                            .on_disabled_hover_text("The plan in use and the plans that come with Windows can't be deleted.")
                            .clicked()
                        {
                            self.dialog = Some(Dialog::Delete(plan.guid, plan.name.clone()));
                        }
                    });
                    ui.add_space(10.0);
                    // Column headings.
                    let w = ui.available_width();
                    let col = 170.0;
                    ui.horizontal(|ui| {
                        ui.add_space(w - 2.0 * col - 16.0);
                        ui.allocate_ui(vec2(col, 20.0), |ui| {
                            ui.set_width(col - 8.0);
                            ui.label(RichText::new(Icon::PLUG.with("Plugged in")).font(semibold(13.0)).color(p.muted));
                        });
                        ui.allocate_ui(vec2(col, 20.0), |ui| {
                            ui.set_width(col - 8.0);
                            ui.label(RichText::new(Icon::BATTERY.with("On battery")).font(semibold(13.0)).color(p.muted));
                        });
                    });
                    widgets::divider(ui);
                    for key in SettingKey::ALL {
                        self.setting_line(ui, plan.guid, key, col);
                    }
                });
            });
        });
    }

    fn setting_line(&mut self, ui: &mut Ui, plan: Guid, key: SettingKey, col: f32) {
        let p = pal(ui);
        let info = key.info();
        let value = self.plan_values.get(&key).copied().flatten();
        let w = ui.available_width();
        let mut write: Option<(bool, u32)> = None;
        ui.horizontal(|ui| {
            let left = w - 2.0 * col - 16.0;
            ui.allocate_ui_with_layout(vec2(left, 46.0), Layout::top_down(Align::Min), |ui| {
                ui.set_width(left);
                ui.spacing_mut().item_spacing.y = 1.0;
                ui.add_space(4.0);
                ui.label(RichText::new(info.title).size(14.0));
                ui.add(
                    egui::Label::new(RichText::new(info.hint).size(12.0).color(p.muted)).truncate(),
                )
                .on_hover_text(info.hint);
            });
            match value {
                None => {
                    ui.allocate_ui(vec2(2.0 * col, 30.0), |ui| {
                        ui.label(
                            RichText::new("Not available on this PC")
                                .color(p.faint)
                                .size(12.5),
                        );
                    });
                }
                Some(v) => {
                    for (ac, current) in [(true, v.ac), (false, v.dc)] {
                        ui.allocate_ui_with_layout(
                            vec2(col, 34.0),
                            Layout::left_to_right(Align::Center),
                            |ui| {
                                if let Some(new) =
                                    value_picker(ui, (plan, key, ac), info.kind, current)
                                {
                                    write = Some((ac, new));
                                }
                            },
                        );
                    }
                }
            }
        });
        if let Some((ac, new)) = write {
            match self.plans.write(plan, key, ac, new) {
                Ok(()) => {
                    self.load_plan_values();
                    self.send(crate::engine::Cmd::PlanChanged);
                }
                Err(e) => self.toast(format!("Could not change it: {e}"), true),
            }
        }
    }
}

/// A combo box (or a number) for one value. Returns a new value when changed.
fn value_picker(
    ui: &mut Ui,
    id: impl std::hash::Hash,
    kind: ValueKind,
    current: u32,
) -> Option<u32> {
    let mut out = None;
    match kind {
        ValueKind::Percent => {
            let mut v = current;
            let r = ui.add(
                egui::DragValue::new(&mut v)
                    .range(0..=100)
                    .suffix(" %")
                    .speed(0.4),
            );
            if r.drag_stopped() || (r.lost_focus() && v != current) || (r.changed() && !r.dragged())
            {
                out = (v != current).then_some(v);
            }
        }
        _ => {
            let choices: Vec<u32> = match kind {
                ValueKind::Timeout => {
                    let mut c = TIMEOUT_CHOICES.to_vec();
                    if !c.contains(&current) {
                        c.insert(0, current);
                    }
                    c
                }
                ValueKind::Action => (0..ACTIONS.len() as u32).collect(),
                ValueKind::Choice(names) => (0..names.len() as u32).collect(),
                ValueKind::Percent => Vec::new(),
            };
            egui::ComboBox::from_id_salt(id)
                .width(150.0)
                .selected_text(value_label(kind, current))
                .show_ui(ui, |ui| {
                    for c in choices {
                        if ui
                            .selectable_label(c == current, value_label(kind, c))
                            .clicked()
                            && c != current
                        {
                            out = Some(c);
                        }
                    }
                });
        }
    }
    out
}
