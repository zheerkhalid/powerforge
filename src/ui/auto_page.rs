//! Automatic plan switching: rules by power source and by running or
//! foreground app.

use super::app::{page_header, App};
use super::theme::{pal, semibold, Icon};
use super::widgets;
use crate::autoswitch::{normalize, Rule, Trigger};
use crate::plans::Guid;
use eframe::egui::{self, vec2, Align, Layout, RichText, Ui};

#[derive(Default)]
pub struct RuleForm {
    /// 0 on battery, 1 plugged in, 2 app running, 3 app in front.
    pub kind: usize,
    pub exe: String,
    pub plan: Option<Guid>,
    pub running: Vec<String>,
}

const KINDS: [&str; 4] = [
    "On battery",
    "Plugged in",
    "While an app runs",
    "While an app is in front",
];

impl App {
    pub(super) fn auto_ui(&mut self, ui: &mut Ui) {
        let p = pal(ui);
        page_header(
            ui,
            "Auto switch",
            "Change the power plan by itself when you unplug or start an app.",
            |_| {},
        );
        widgets::card(ui, |ui| {
            widgets::setting_row(
                ui,
                "Switch plans automatically",
                "When no rule applies any more, PowerForge goes back to the plan you chose yourself. If you pick a plan by hand, that choice wins until the situation changes.",
                |ui| {
                    if widgets::toggle(ui, self.settings.auto.enabled, true).clicked() {
                        self.settings.auto.enabled = !self.settings.auto.enabled;
                    }
                },
            );
        });
        ui.add_space(8.0);
        let plans: Vec<(Guid, String)> = self
            .plan_list
            .iter()
            .map(|x| (x.guid, x.name.clone()))
            .collect();
        widgets::card(ui, |ui| {
            widgets::section(ui, "Rules");
            ui.label(RichText::new("An app in front wins over an app running, which wins over the power source. Within a group, the first rule wins.").size(12.5).color(p.muted));
            ui.add_space(6.0);
            if self.settings.auto.rules.is_empty() {
                ui.label(RichText::new("No rules yet. Add one below.").color(p.faint));
            }
            let mut remove = None;
            let mut swap = None;
            let n = self.settings.auto.rules.len();
            #[allow(clippy::needless_range_loop)]
            for i in 0..n {
                let rule = self.settings.auto.rules[i].clone();
                ui.horizontal(|ui| {
                    if widgets::toggle(ui, rule.enabled, true)
                        .on_hover_text("Turn this rule on or off")
                        .clicked()
                    {
                        self.settings.auto.rules[i].enabled = !rule.enabled;
                    }
                    let icon = match rule.trigger {
                        Trigger::OnBattery => Icon::BATTERY,
                        Trigger::PluggedIn => Icon::PLUG,
                        _ => Icon::APPS,
                    };
                    ui.label(RichText::new(icon.glyph()).color(p.accent));
                    ui.allocate_ui_with_layout(
                        vec2(300.0, 30.0),
                        Layout::left_to_right(Align::Center),
                        |ui| {
                            ui.set_width(300.0);
                            ui.label(
                                RichText::new(rule.trigger.label())
                                    .font(semibold(14.0))
                                    .color(if rule.enabled { p.text } else { p.muted }),
                            );
                        },
                    );
                    ui.label(RichText::new("use").color(p.muted));
                    let current = plans
                        .iter()
                        .find(|x| x.0 == rule.plan)
                        .map(|x| x.1.clone())
                        .unwrap_or_else(|| "a deleted plan".into());
                    egui::ComboBox::from_id_salt(("rule-plan", i))
                        .width(190.0)
                        .selected_text(current)
                        .show_ui(ui, |ui| {
                            for (g, name) in &plans {
                                if ui.selectable_label(*g == rule.plan, name).clicked() {
                                    self.settings.auto.rules[i].plan = *g;
                                }
                            }
                        });
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if widgets::icon_button(ui, Icon::DELETE, "Remove this rule").clicked() {
                            remove = Some(i);
                        }
                        if i + 1 < n && widgets::icon_button(ui, Icon::DOWN, "Move down").clicked()
                        {
                            swap = Some((i, i + 1));
                        }
                        if i > 0 && widgets::icon_button(ui, Icon::UP, "Move up").clicked() {
                            swap = Some((i, i - 1));
                        }
                    });
                });
                if i + 1 < n {
                    widgets::divider(ui);
                }
            }
            if let Some(i) = remove {
                self.settings.auto.rules.remove(i);
            }
            if let Some((a, b)) = swap {
                self.settings.auto.rules.swap(a, b);
            }
        });
        ui.add_space(8.0);
        widgets::card(ui, |ui| {
            widgets::section(ui, "Add a rule");
            let demo = self.demo;
            let mut added: Option<Rule> = None;
            let form = &mut self.rule_form;
            if form.plan.is_none() {
                form.plan = plans.first().map(|x| x.0);
            }
            ui.horizontal(|ui| {
                egui::ComboBox::from_id_salt("rule-kind")
                    .width(200.0)
                    .selected_text(KINDS[form.kind])
                    .show_ui(ui, |ui| {
                        for (i, k) in KINDS.iter().enumerate() {
                            ui.selectable_value(&mut form.kind, i, *k);
                        }
                    });
                if form.kind >= 2 {
                    ui.add(
                        egui::TextEdit::singleline(&mut form.exe)
                            .hint_text("app.exe")
                            .desired_width(170.0),
                    );
                    let menu = egui::ComboBox::from_id_salt("running")
                        .width(130.0)
                        .selected_text("Running apps");
                    let resp = menu.show_ui(ui, |ui| {
                        for name in &form.running {
                            if ui.selectable_label(false, name).clicked() {
                                form.exe = name.clone();
                            }
                        }
                    });
                    if resp.response.clicked() || form.running.is_empty() {
                        if demo {
                            form.running = [
                                "blender.exe",
                                "game.exe",
                                "obs64.exe",
                                "teams.exe",
                                "zoom.exe",
                            ]
                            .iter()
                            .map(|s| s.to_string())
                            .collect();
                        } else if resp.response.clicked() {
                            let mut v: Vec<String> =
                                crate::autoswitch::running_exes().into_iter().collect();
                            v.sort();
                            form.running = v;
                        }
                    }
                }
                ui.label(RichText::new("use").color(p.muted));
                let current = plans
                    .iter()
                    .find(|x| Some(x.0) == form.plan)
                    .map(|x| x.1.clone())
                    .unwrap_or_default();
                egui::ComboBox::from_id_salt("rule-new-plan")
                    .width(180.0)
                    .selected_text(current)
                    .show_ui(ui, |ui| {
                        for (g, name) in &plans {
                            if ui.selectable_label(Some(*g) == form.plan, name).clicked() {
                                form.plan = Some(*g);
                            }
                        }
                    });
                let exe = normalize(&form.exe);
                let ok = form.plan.is_some() && (form.kind < 2 || exe.len() > 4);
                if widgets::primary_button(ui, "Add", ok).clicked() {
                    let trigger = match form.kind {
                        0 => Trigger::OnBattery,
                        1 => Trigger::PluggedIn,
                        2 => Trigger::AppRunning(exe),
                        _ => Trigger::AppForeground(exe),
                    };
                    if let Some(plan) = form.plan {
                        added = Some(Rule {
                            enabled: true,
                            trigger,
                            plan,
                        });
                        form.exe.clear();
                    }
                }
            });
            if let Some(rule) = added {
                self.settings.auto.rules.push(rule);
                self.settings.auto.enabled = true;
            }
            ui.label(RichText::new("Tip: a rule for \u{201c}On battery\u{201d} with Power saver and one for \u{201c}Plugged in\u{201d} with Balanced covers most laptops.").size(12.0).color(p.faint));
        });
    }
}
