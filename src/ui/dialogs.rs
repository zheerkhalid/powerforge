//! Modal dialogs: rename, duplicate, delete and restore plans, and the
//! confirmation before an action that needs administrator rights. Each
//! modal has its own id and a fixed-height button row.

use super::app::{App, Dialog};
use super::theme::{pal, semibold};
use super::widgets;
use eframe::egui::{self, RichText};

impl App {
    pub(super) fn dialogs_ui(&mut self, ctx: &egui::Context) {
        let Some(dialog) = self.dialog.take() else {
            return;
        };
        let mut keep = true;
        let mut ok = false;
        let (id, title) = match &dialog {
            Dialog::Rename(..) => ("dlg-rename", "Rename plan".to_string()),
            Dialog::Duplicate(..) => ("dlg-duplicate", "Duplicate plan".to_string()),
            Dialog::Delete(_, n) => ("dlg-delete", format!("Delete \u{201c}{n}\u{201d}?")),
            Dialog::Restore(_, n) => ("dlg-restore", format!("Restore \u{201c}{n}\u{201d}?")),
            Dialog::Admin(t, ..) => ("dlg-admin", t.clone()),
        };
        let mut dialog = dialog;
        let modal = egui::Modal::new(egui::Id::new(id))
            .frame(widgets::modal_frame(ctx))
            .show(ctx, |ui| {
                let p = pal(ui);
                ui.set_width(440.0);
                ui.label(RichText::new(&title).font(semibold(19.0)));
                ui.add_space(6.0);
                let mut enter = false;
                let valid = match &mut dialog {
                    Dialog::Rename(_, name) | Dialog::Duplicate(_, name) => {
                        let r = ui.add(egui::TextEdit::singleline(name).desired_width(f32::INFINITY));
                        r.request_focus();
                        enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                        !name.trim().is_empty()
                    }
                    Dialog::Delete(..) => {
                        ui.label(RichText::new("The plan and its settings are removed. This can't be undone.").color(p.muted));
                        true
                    }
                    Dialog::Restore(..) => {
                        ui.label(RichText::new("All of this plan's settings go back to the values Windows uses for it.").color(p.muted));
                        true
                    }
                    Dialog::Admin(_, text, _) => {
                        ui.add(egui::Label::new(RichText::new(text.as_str()).color(p.muted)).wrap());
                        ui.add_space(4.0);
                        if !self.demo && !crate::elevate::is_elevated() {
                            ui.label(RichText::new("Windows will ask for administrator rights for this one action.").size(12.5).color(p.warn));
                        }
                        true
                    }
                };
                ui.add_space(10.0);
                widgets::button_row(ui, |ui| {
                    let label = match &dialog {
                        Dialog::Rename(..) => "Rename",
                        Dialog::Duplicate(..) => "Duplicate",
                        Dialog::Delete(..) => "Delete",
                        Dialog::Restore(..) => "Restore",
                        Dialog::Admin(..) => "Continue",
                    };
                    let primary = if matches!(dialog, Dialog::Delete(..)) {
                        widgets::danger_button(ui, label, valid)
                    } else {
                        widgets::primary_button(ui, label, valid)
                    };
                    if primary.clicked() || (enter && valid) {
                        ok = true;
                    }
                    if widgets::ghost_button(ui, "Cancel").clicked() {
                        keep = false;
                    }
                });
            });
        if modal.should_close() {
            keep = false;
        }
        if ok {
            match dialog {
                Dialog::Rename(g, name) => match self.plans.rename(g, &name) {
                    Ok(()) => {
                        self.refresh_plans();
                        self.send(crate::engine::Cmd::PlanChanged);
                    }
                    Err(e) => self.toast(format!("Could not rename: {e}"), true),
                },
                Dialog::Duplicate(g, name) => match self.plans.duplicate(g, &name) {
                    Ok(new) => {
                        self.plan_sel = Some(new);
                        self.refresh_plans();
                        self.toast(format!("Made \u{201c}{}\u{201d}", name.trim()), false);
                    }
                    Err(e) => self.toast(format!("Could not duplicate: {e}"), true),
                },
                Dialog::Delete(g, _) => match self.plans.delete(g) {
                    Ok(()) => {
                        self.plan_sel = None;
                        self.refresh_plans();
                    }
                    Err(e) => self.toast(format!("Could not delete: {e}"), true),
                },
                Dialog::Restore(g, _) => match self.plans.restore_defaults(g) {
                    Ok(()) => {
                        self.load_plan_values();
                        self.toast("Restored the Windows defaults.", false);
                    }
                    Err(e) => self.toast(format!("Could not restore: {e}"), true),
                },
                Dialog::Admin(_, _, action) => self.run_admin(ctx, action),
            }
            return;
        }
        if keep {
            self.dialog = Some(dialog);
        }
    }
}
