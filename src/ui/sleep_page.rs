//! Sleep and wake: what keeps the PC awake, what woke it, wake timers,
//! devices allowed to wake it, how it sleeps, standby drain and history.

use super::app::{page_header, AdminAction, App, Dialog, Job};
use super::theme::{pal, semibold, Icon};
use super::widgets;
use crate::diag::{self, DiagError};
use crate::events::EventKind;
use crate::util;
use eframe::egui::{self, vec2, Align, Layout, RichText, Ui};

fn err(e: DiagError) -> String {
    e.to_string()
}

impl App {
    /// Starts the checks that need no administrator rights.
    fn start_diag(&mut self, ctx: &egui::Context) {
        if self.diag.started || self.demo {
            return;
        }
        self.diag.started = true;
        self.diag.lastwake = Some(Job::start(ctx, || diag::query_lastwake().map_err(err)));
        self.diag.devices = Some(Job::start(ctx, || diag::query_wake_devices().map_err(err)));
        self.diag.events = Some(Job::start(ctx, || crate::events::read_recent(14, 300)));
        self.diag.snapshot.caps = Some(diag::capabilities());
        if crate::elevate::is_elevated() {
            self.diag.requests = Some(Job::start(ctx, || diag::query_requests(false).map_err(err)));
            self.diag.timers = Some(Job::start(ctx, || {
                diag::query_waketimers(false).map_err(err)
            }));
        }
    }

    pub(super) fn run_admin(&mut self, ctx: &egui::Context, action: AdminAction) {
        match action {
            AdminAction::Requests => {
                if self.demo {
                    self.diag.snapshot.requests = crate::diag::demo(self.now()).requests;
                } else {
                    self.diag.requests =
                        Some(Job::start(ctx, || diag::query_requests(true).map_err(err)));
                }
            }
            AdminAction::WakeTimers => {
                if self.demo {
                    self.diag.snapshot.wake_timers = crate::diag::demo(self.now()).wake_timers;
                } else {
                    self.diag.timers = Some(Job::start(ctx, || {
                        diag::query_waketimers(true).map_err(err)
                    }));
                }
            }
            AdminAction::DeviceWake(name, enable) => {
                if self.demo {
                    if let Some(devs) = &mut self.diag.snapshot.wake_devices {
                        for d in devs.iter_mut().filter(|d| d.name == name) {
                            d.armed = enable;
                        }
                    }
                } else {
                    self.diag.action = Some(Job::start(ctx, move || {
                        diag::set_device_wake(&name, enable).map_err(err)?;
                        Ok(if enable {
                            format!("{name} can wake the PC again.")
                        } else {
                            format!("{name} can no longer wake the PC.")
                        })
                    }));
                }
            }
            AdminAction::Hibernate(on) => {
                if self.demo {
                    if let Some(c) = &mut self.diag.snapshot.caps {
                        c.hibernate_enabled = on;
                        c.notes = diag::caps_notes(c);
                    }
                } else {
                    self.diag.action = Some(Job::start(ctx, move || {
                        diag::set_hibernate(on).map_err(err)?;
                        Ok(if on {
                            "Hibernation is on.".into()
                        } else {
                            "Hibernation is off.".into()
                        })
                    }));
                }
            }
            AdminAction::Energy => self.start_energy(ctx),
        }
    }

    pub(super) fn poll_diag(&mut self) {
        let d = &mut self.diag;
        if let Some(j) = &mut d.lastwake {
            if j.poll() {
                if let Job::Done(v) = j {
                    d.snapshot.last_wake = Some(v.clone());
                }
            }
        }
        if let Some(j) = &mut d.devices {
            if j.poll() {
                if let Job::Done(v) = j {
                    d.snapshot.wake_devices = Some(v.clone());
                }
            }
        }
        if let Some(j) = &mut d.events {
            if j.poll() {
                if let Job::Done(v) = j {
                    d.snapshot.events = Some(v.clone());
                }
            }
        }
        if let Some(j) = &mut d.requests {
            if j.poll() {
                if let Job::Done(v) = j {
                    d.snapshot.requests = Some(v.clone());
                }
            }
        }
        if let Some(j) = &mut d.timers {
            if j.poll() {
                if let Job::Done(v) = j {
                    d.snapshot.wake_timers = Some(v.clone());
                }
            }
        }
        let mut message = None;
        if let Some(j) = &mut d.action {
            if j.poll() {
                message = Some(match j {
                    Job::Done(m) => (m.clone(), false),
                    Job::Failed(e) => (e.clone(), true),
                    _ => (String::new(), false),
                });
            }
        }
        if let Some((m, error)) = message {
            self.diag.action = None;
            if !m.is_empty() {
                self.toast(m, error);
            }
            // Read the devices and sleep states again.
            self.diag.snapshot.caps = Some(diag::capabilities());
            self.diag.started = false;
        }
    }

    pub(super) fn sleep_ui(&mut self, ui: &mut Ui) {
        let p = pal(ui);
        let ctx = ui.ctx().clone();
        self.start_diag(&ctx);
        self.ensure_report(&ctx);
        let mut refresh = false;
        page_header(
            ui,
            "Sleep & wake",
            "Why the PC won't sleep, and why it woke up, in plain words.",
            |ui| {
                if widgets::icon_button(ui, Icon::REFRESH, "Check again").clicked() {
                    refresh = true;
                }
            },
        );
        if refresh && !self.demo {
            self.diag.started = false;
            self.start_diag(&ctx);
        }
        let snap = self.diag.snapshot.clone();
        let admin_note = if crate::elevate::is_elevated() || self.demo {
            ""
        } else {
            "Windows asks for administrator rights for this one check."
        };

        // What keeps the PC awake.
        widgets::card(ui, |ui| {
            ui.horizontal(|ui| {
                widgets::section(ui, &Icon::SUNNY.with("What keeps the PC awake right now"));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let running = self.diag.requests.as_ref().is_some_and(|j| j.running());
                    if running {
                        ui.spinner();
                    } else if widgets::ghost_button(
                        ui,
                        if snap.requests.is_some() {
                            "Check again"
                        } else {
                            "Check now"
                        },
                    )
                    .on_hover_text(admin_note)
                    .clicked()
                    {
                        self.run_admin(&ctx, AdminAction::Requests);
                    }
                });
            });
            if let Some(Job::Failed(e)) = &self.diag.requests {
                ui.label(RichText::new(format!("Not checked: {e}.")).color(p.warn));
            }
            match &snap.requests {
                None => {
                    ui.label(RichText::new(format!("Lists the programs, services and drivers that ask Windows not to sleep or not to turn off the screen. {admin_note}")).color(p.muted));
                }
                Some(r) if r.is_empty() => {
                    ui.label(RichText::new(Icon::CHECK.with("Nothing is keeping the PC awake. It will sleep after the time set in the power plan.")).color(p.good));
                }
                Some(r) => {
                    for q in r {
                        ui.horizontal(|ui| {
                            let color = if q.kind.blocks_sleep() {
                                p.warn
                            } else {
                                p.info
                            };
                            ui.allocate_ui_with_layout(
                                vec2(180.0, 22.0),
                                Layout::left_to_right(Align::Center),
                                |ui| {
                                    widgets::badge(ui, q.kind.label(), color);
                                },
                            );
                            ui.label(RichText::new(&q.name).font(semibold(14.0)));
                        });
                        ui.add(
                            egui::Label::new(RichText::new(&q.plain).color(p.muted).size(13.0))
                                .wrap(),
                        );
                        ui.add_space(4.0);
                    }
                }
            }
        });
        ui.add_space(8.0);

        ui.columns(2, |cols| {
            // Last wake.
            widgets::card(&mut cols[0], |ui| {
                ui.set_min_height(150.0);
                widgets::section(ui, &Icon::ALARM.with("What woke the PC last time"));
                match &snap.last_wake {
                    Some(w) => {
                        if let Some(s) = w.sources.first() {
                            ui.label(RichText::new(if s.name.is_empty() { s.kind.clone() } else { s.name.clone() }).font(semibold(16.0)));
                        }
                        ui.add(egui::Label::new(RichText::new(&w.plain).color(p.muted)).wrap());
                    }
                    None => {
                        if let Some(Job::Failed(e)) = &self.diag.lastwake {
                            ui.label(RichText::new(e).color(p.warn));
                        } else {
                            ui.spinner();
                        }
                    }
                }
            });
            // Wake timers.
            widgets::card(&mut cols[1], |ui| {
                ui.set_min_height(150.0);
                ui.horizontal(|ui| {
                    widgets::section(ui, &Icon::CLOCK.with("Scheduled wake-ups"));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if self.diag.timers.as_ref().is_some_and(|j| j.running()) {
                            ui.spinner();
                        } else if widgets::ghost_button(ui, "Check").on_hover_text(admin_note).clicked() {
                            self.run_admin(&ctx, AdminAction::WakeTimers);
                        }
                    });
                });
                if let Some(Job::Failed(e)) = &self.diag.timers {
                    ui.label(RichText::new(format!("Not checked: {e}.")).color(p.warn));
                }
                match &snap.wake_timers {
                    None => {
                        ui.label(RichText::new(format!("Updates and scheduled tasks can set timers that wake the PC at night. {admin_note}")).color(p.muted));
                    }
                    Some(t) if t.is_empty() => {
                        ui.label(RichText::new(Icon::CHECK.with("No wake timers are set.")).color(p.good));
                    }
                    Some(t) => {
                        for w in t {
                            ui.label(RichText::new(&w.owner).font(semibold(14.0)));
                            ui.add(egui::Label::new(RichText::new(&w.plain).color(p.muted).size(13.0)).wrap());
                        }
                    }
                }
            });
        });
        ui.add_space(8.0);

        // Devices.
        widgets::card(ui, |ui| {
            ui.horizontal(|ui| {
                widgets::section(ui, &Icon::DEVICE.with("Devices allowed to wake the PC"));
                if self.diag.action.as_ref().is_some_and(|j| j.running()) {
                    ui.spinner();
                }
            });
            match &snap.wake_devices {
                None => {
                    ui.spinner();
                }
                Some(devs) if devs.is_empty() => {
                    ui.label(RichText::new("No device can wake this PC.").color(p.muted));
                }
                Some(devs) => {
                    for (i, d) in devs.iter().enumerate() {
                        ui.horizontal(|ui| {
                            let icon = match d.category.as_str() {
                                "Keyboard" => Icon::KEYBOARD,
                                "Mouse or touchpad" => Icon::MOUSE,
                                "Network adapter" => Icon::NETWORK,
                                "Bluetooth" => Icon::BLUETOOTH,
                                "USB controller" => Icon::USB,
                                _ => Icon::DEVICE,
                            };
                            ui.label(RichText::new(icon.glyph()).size(18.0).color(if d.armed { p.accent } else { p.faint }));
                            ui.vertical(|ui| {
                                ui.spacing_mut().item_spacing.y = 1.0;
                                ui.label(RichText::new(&d.name).font(semibold(14.0)));
                                ui.label(RichText::new(format!("{}: {}", d.category, diag::device_advice(&d.category))).size(12.5).color(p.muted));
                            });
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                let r = widgets::toggle(ui, d.armed, true).on_hover_text(if d.armed { "Allowed to wake the PC. Click to stop it." } else { "Not allowed. Click to allow." });
                                ui.label(RichText::new(if d.armed { "Can wake" } else { "Can't wake" }).size(12.5).color(p.muted));
                                if r.clicked() {
                                    let (title, text) = if d.armed {
                                        ("Stop this device from waking the PC?".to_string(), format!("{} will no longer wake the PC from sleep. You can allow it again here.", d.name))
                                    } else {
                                        ("Let this device wake the PC?".to_string(), format!("{} will be able to wake the PC from sleep.", d.name))
                                    };
                                    self.dialog = Some(Dialog::Admin(title, text, AdminAction::DeviceWake(d.name.clone(), !d.armed)));
                                }
                            });
                        });
                        if i + 1 < devs.len() {
                            widgets::divider(ui);
                        }
                    }
                    ui.label(
                        RichText::new(format!(
                            "Changing this needs administrator rights (one prompt per change). {}",
                            if self.demo {
                                ""
                            } else {
                                "Devices that are not listed can't wake the PC at all."
                            }
                        ))
                        .size(12.0)
                        .color(p.faint),
                    );
                }
            }
        });
        ui.add_space(8.0);

        ui.columns(2, |cols| {
            // How this PC sleeps.
            widgets::card(&mut cols[0], |ui| {
                ui.set_min_height(220.0);
                widgets::section(ui, &Icon::MOON.with("How this PC sleeps"));
                if let Some(c) = &snap.caps {
                    ui.horizontal_wrapped(|ui| {
                        widgets::badge(ui, if c.modern_standby { "Modern Standby" } else if c.s3 { "Classic sleep (S3)" } else { "No sleep state" }, p.info);
                        widgets::badge(ui, if c.hibernate_enabled { "Hibernation on" } else { "Hibernation off" }, if c.hibernate_enabled { p.good } else { p.faint });
                        if c.fast_startup {
                            widgets::badge(ui, "Fast startup", p.accent);
                        }
                    });
                    for n in &c.notes {
                        ui.add(egui::Label::new(RichText::new(n).size(13.0).color(p.muted)).wrap());
                    }
                    if c.hibernate_supported || c.hibernate_enabled {
                        let on = c.hibernate_enabled;
                        if widgets::ghost_button(ui, if on { "Turn hibernation off" } else { "Turn hibernation on" }).clicked() {
                            let text = if on {
                                "Turning hibernation off frees the hibernation file's disk space, but the PC can no longer hibernate and fast startup stops working.".to_string()
                            } else {
                                "Turning hibernation on reserves disk space for the hibernation file and allows hibernate and fast startup.".to_string()
                            };
                            self.dialog = Some(Dialog::Admin(if on { "Turn hibernation off?".into() } else { "Turn hibernation on?".into() }, text, AdminAction::Hibernate(!on)));
                        }
                    }
                }
            });
            // Standby drain.
            widgets::card(&mut cols[1], |ui| {
                ui.set_min_height(220.0);
                widgets::section(ui, &Icon::BATTERY.with("Battery use while asleep"));
                match &snap.standby {
                    Some(s) => {
                        if let Some(pph) = s.pct_per_hour {
                            let color = if pph < 1.0 { p.good } else if pph <= 3.0 { p.warn } else { p.danger };
                            ui.label(RichText::new(format!("{pph:.1} % per hour")).font(semibold(26.0)).color(color));
                        }
                        ui.label(RichText::new(format!("{} standby sessions, {} in total, in the last few days.", s.sessions, util::fmt_duration(s.total_secs))).color(p.muted).size(13.0));
                        ui.add(egui::Label::new(RichText::new(&s.verdict).size(13.0)).wrap());
                    }
                    None => {
                        let text = if self.live.battery.is_none() {
                            "Only laptops and tablets on battery have standby drain to measure."
                        } else if self.report.running() {
                            "Reading the Windows battery report..."
                        } else {
                            "No standby on battery in the last few days."
                        };
                        ui.label(RichText::new(text).color(p.muted));
                    }
                }
            });
        });
        ui.add_space(8.0);

        // History.
        widgets::card(ui, |ui| {
            widgets::section(
                ui,
                &Icon::HISTORY.with("Sleep and wake history (last 14 days)"),
            );
            match &snap.events {
                None => {
                    ui.spinner();
                }
                Some(ev) if ev.is_empty() => {
                    ui.label(
                        RichText::new("No sleep or wake events in the System log for this period.")
                            .color(p.muted),
                    );
                }
                Some(ev) => {
                    for e in ev.iter().take(60) {
                        ui.horizontal(|ui| {
                            let (icon, color) = match e.kind {
                                EventKind::Sleep | EventKind::StandbyEnter => (Icon::MOON, p.info),
                                EventKind::Wake | EventKind::StandbyExit => (Icon::SUNNY, p.accent),
                                EventKind::Boot => (Icon::POWER, p.good),
                                EventKind::Shutdown => (Icon::POWER, p.muted),
                                EventKind::Unexpected => (Icon::WARNING, p.danger),
                            };
                            ui.allocate_ui_with_layout(
                                vec2(130.0, 20.0),
                                Layout::left_to_right(Align::Center),
                                |ui| {
                                    ui.label(
                                        RichText::new(util::fmt_datetime(e.t))
                                            .size(13.0)
                                            .color(p.muted),
                                    );
                                },
                            );
                            ui.label(RichText::new(icon.glyph()).color(color));
                            ui.allocate_ui_with_layout(
                                vec2(150.0, 20.0),
                                Layout::left_to_right(Align::Center),
                                |ui| {
                                    ui.label(RichText::new(e.kind.label()).font(semibold(13.5)));
                                },
                            );
                            ui.add(
                                egui::Label::new(RichText::new(&e.text).size(13.0).color(p.muted))
                                    .truncate(),
                            );
                        });
                    }
                }
            }
        });
    }
}
