//! First-run setup: find the PS5, sign in for the account ID, link with the code, test.

use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::Duration;

use eframe::egui::{self, Color32, RichText};

use super::{AccountFlow, GuiApp, Tab};
use crate::regist::{self, FoundConsole};
use crate::runner::{self, Problem, ProblemKind};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Step {
    Console,
    Account,
    Link,
    Test,
    Done,
}

enum Event {
    Found(Vec<FoundConsole>),
    Registered(Result<(Box<regist::Registered>, String), String>),
    Tested(Result<(), Problem>),
}

pub struct Wizard {
    pub step: Step,
    events: Option<Receiver<Event>>,
    busy: bool,
    found: Vec<FoundConsole>,
    searched: bool,
    chosen: Option<usize>,
    manual_ip: String,
    pin: String,
    passcode: String,
    error: String,
    test_problem: Option<Problem>,
}

impl Wizard {
    pub fn new() -> Self {
        Wizard {
            step: Step::Console,
            events: None,
            busy: false,
            found: Vec::new(),
            searched: false,
            chosen: None,
            manual_ip: String::new(),
            pin: String::new(),
            passcode: String::new(),
            error: String::new(),
            test_problem: None,
        }
    }

    fn run(&mut self, f: impl FnOnce() -> Event + Send + 'static) {
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let _ = tx.send(f());
        });
        self.events = Some(rx);
        self.busy = true;
        self.error.clear();
    }

    fn search(&mut self) {
        self.run(|| Event::Found(regist::discover(Duration::from_millis(2500)).unwrap_or_default()));
    }

    /// The address the user picked or typed.
    fn host(&self) -> Option<String> {
        if let Some(i) = self.chosen {
            return self.found.get(i).map(|c| c.ip.to_string());
        }
        let m = self.manual_ip.trim();
        (!m.is_empty()).then(|| m.to_string())
    }

    fn console_name(&self) -> String {
        self.chosen.and_then(|i| self.found.get(i)).map(|c| c.name.clone()).unwrap_or_else(|| "your PS5".into())
    }
}

fn big_button(ui: &mut egui::Ui, text: &str, enabled: bool) -> bool {
    ui.add_enabled(enabled, egui::Button::new(RichText::new(text).size(16.0)).min_size(egui::vec2(150.0, 34.0))).clicked()
}

fn error_label(ui: &mut egui::Ui, text: &str) {
    if !text.is_empty() {
        ui.colored_label(Color32::from_rgb(0xdc, 0x26, 0x26), text);
    }
}

impl GuiApp {
    pub(super) fn open_wizard(&mut self) {
        self.wizard = Some(Wizard::new());
        self.account = AccountFlow::default();
    }

    fn start_test(&mut self, w: &mut Wizard) {
        let cfg = match self.built_config() {
            Ok(c) => c,
            Err(e) => {
                w.error = format!("{e:#}");
                return;
            }
        };
        w.test_problem = None;
        w.run(move || Event::Tested(runner::check_connection(&cfg)));
    }

    /// Draws the wizard; returns false when it should close.
    pub(super) fn ui_wizard(&mut self, ui: &mut egui::Ui) {
        let Some(mut w) = self.wizard.take() else { return };
        let mut keep = true;

        // results from background work
        if let Some(rx) = &w.events {
            if let Ok(ev) = rx.try_recv() {
                w.events = None;
                w.busy = false;
                match ev {
                    Event::Found(list) => {
                        w.searched = true;
                        // keep the pick if the same console shows up again
                        let prev = w.host();
                        w.found = list;
                        w.chosen = prev.and_then(|h| w.found.iter().position(|c| c.ip.to_string() == h));
                        if w.chosen.is_none() && w.found.len() == 1 && w.manual_ip.trim().is_empty() {
                            w.chosen = Some(0);
                        }
                    }
                    Event::Registered(Ok((reg, host))) => {
                        self.cfg.console.host = host;
                        self.cfg.console.regist_key = crate::http::hex(reg.creds.regist_key_trimmed());
                        self.cfg.console.rp_key = crate::http::hex(&reg.creds.morning);
                        if !reg.nickname.is_empty() {
                            self.cfg.console.nickname = reg.nickname.clone();
                        }
                        self.dirty = true;
                        if self.save() {
                            w.step = Step::Test;
                            self.start_test(&mut w);
                        } else {
                            w.error = self.message.clone();
                        }
                    }
                    Event::Registered(Err(e)) => w.error = e,
                    Event::Tested(Ok(())) => w.step = Step::Done,
                    Event::Tested(Err(p)) => w.test_problem = Some(p),
                }
            }
        }

        // header: steps
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            let steps = [(Step::Console, "1  Find PS5"), (Step::Account, "2  Sign in"), (Step::Link, "3  Link"), (Step::Test, "4  Test")];
            let cur = w.step as usize;
            for (i, (st, label)) in steps.iter().enumerate() {
                let done = (*st as usize) < cur || w.step == Step::Done;
                let txt = RichText::new(if done { format!("✔ {}", &label[3..]) } else { label.to_string() });
                let txt = if *st == w.step { txt.strong().color(Color32::from_rgb(0x25, 0x63, 0xeb)) } else { txt.weak() };
                ui.label(txt.size(15.0));
                if i < steps.len() - 1 {
                    ui.weak("  ›  ");
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if w.step != Step::Done && ui.button("Cancel setup").clicked() {
                    keep = false;
                }
            });
        });
        ui.separator();
        ui.add_space(6.0);

        match w.step {
            Step::Console => {
                ui.heading("Find your PS5");
                ui.label("Your PS5 must be switched on (or in rest mode) and connected to the same network as this PC.");
                ui.label("On the PS5, Remote Play must be enabled: Settings › System › Remote Play › Enable Remote Play.");
                ui.add_space(8.0);
                if !w.searched && !w.busy {
                    w.search();
                }
                if w.busy {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("Looking for consoles on your network…");
                    });
                } else {
                    if w.found.is_empty() {
                        ui.label(RichText::new("No PS5 answered.").strong());
                        ui.label("Check that it's on and Remote Play is enabled, then search again, or enter its address below.");
                    }
                    for (i, c) in w.found.clone().iter().enumerate() {
                        let selected = w.chosen == Some(i);
                        let state = if c.awake { "on" } else { "in rest mode" };
                        let text = RichText::new(format!("🎮  {}   ·   {}   ·   {}", c.name, c.ip, state)).size(16.0);
                        if ui.add(egui::SelectableLabel::new(selected, text)).clicked() {
                            w.chosen = Some(i);
                            w.manual_ip.clear();
                        }
                    }
                    ui.add_space(4.0);
                    if ui.button("🔄 Search again").clicked() {
                        w.search();
                    }
                }
                ui.add_space(12.0);
                ui.collapsing("Not listed? Enter the address yourself", |ui| {
                    ui.label("On the PS5: Settings › Network › Connection Status › View Connection Status › IPv4 Address.");
                    ui.horizontal(|ui| {
                        ui.label("Address:");
                        if ui.add(egui::TextEdit::singleline(&mut w.manual_ip).hint_text("e.g. 192.168.1.50")).changed() {
                            w.chosen = None;
                        }
                    });
                });
                ui.add_space(12.0);
                let host = w.host();
                if big_button(ui, "Next ›", host.is_some()) {
                    match runner::resolve_host(host.as_deref().unwrap_or_default()) {
                        Ok(_) => w.step = Step::Account,
                        Err(e) => w.error = format!("{e:#}"),
                    }
                }
                error_label(ui, &w.error);
            }

            Step::Account => {
                ui.heading("Sign in with your PlayStation account");
                ui.label("The PS5 only accepts devices that belong to an account on it. Signing in once gives this app your account ID.");
                ui.label("You sign in on Sony's own page in your browser; this app never sees your password.");
                ui.add_space(8.0);
                if !self.account.active && !self.account.busy {
                    if big_button(ui, "Sign in with PlayStation…", true) {
                        self.account = AccountFlow {
                            active: true,
                            clipboard_baseline: crate::psn::clipboard::text(),
                            ..Default::default()
                        };
                        if let Err(e) = crate::psn::open_login_page() {
                            self.account.error = format!("{e:#}");
                        }
                    }
                } else {
                    ui.group(|ui| self.ui_account_flow(ui));
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.label("Account ID:");
                    ui.add(egui::TextEdit::singleline(&mut self.reg_account).hint_text("filled in after signing in"));
                    if !self.reg_account.trim().is_empty() {
                        let ok = crate::parse_account_id(&self.reg_account).is_ok();
                        if ok {
                            ui.colored_label(Color32::from_rgb(0x16, 0xa3, 0x4a), "✔");
                        } else {
                            ui.colored_label(Color32::from_rgb(0xdc, 0x26, 0x26), "doesn't look right");
                        }
                    }
                });
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if ui.button("‹ Back").clicked() {
                        w.step = Step::Console;
                    }
                    let ok = crate::parse_account_id(&self.reg_account).is_ok();
                    if big_button(ui, "Next ›", ok) {
                        self.account = AccountFlow::default();
                        w.step = Step::Link;
                    }
                });
            }

            Step::Link => {
                ui.heading(format!("Link this PC with {}", w.console_name()));
                ui.label("On the PS5 go to  Settings › System › Remote Play › Link Device.");
                ui.label("A code with 8 digits appears on the TV. Keep that screen open and type the code here:");
                ui.add_space(8.0);
                ui.add(
                    egui::TextEdit::singleline(&mut w.pin)
                        .font(egui::FontId::monospace(28.0))
                        .char_limit(8)
                        .desired_width(200.0)
                        .hint_text("12345678"),
                );
                w.pin.retain(|c| c.is_ascii_digit());
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if ui.add_enabled(!w.busy, egui::Button::new("‹ Back")).clicked() {
                        w.step = Step::Account;
                    }
                    if big_button(ui, "Link", w.pin.len() == 8 && !w.busy) {
                        let host = w.host().unwrap_or_default();
                        let pin = w.pin.clone();
                        let account = self.reg_account.clone();
                        w.run(move || {
                            let r = (|| -> anyhow::Result<regist::Registered> {
                                let ip = runner::resolve_host(&host)?;
                                let pin: u32 = pin.parse()?;
                                let account = crate::parse_account_id(&account)?;
                                regist::register(ip, pin, account)
                            })();
                            Event::Registered(r.map(|reg| (Box::new(reg), host)).map_err(|e| {
                                let m = format!("{e:#}");
                                if m.contains("HTTP 403") || m.contains("rejected") {
                                    format!("The PS5 didn't accept the code. Check it and that the Link Device screen is still open. ({m})")
                                } else {
                                    m
                                }
                            }))
                        });
                    }
                    if w.busy {
                        ui.spinner();
                        ui.label("Linking…");
                    }
                });
                error_label(ui, &w.error);
            }

            Step::Test => {
                ui.heading("Testing the connection");
                if w.busy {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(format!("Connecting to {} (waking it up if needed, this can take up to a minute)…", self.console_label()));
                    });
                } else if let Some(p) = w.test_problem.clone() {
                    ui.label(RichText::new(&p.title).strong().size(16.0));
                    ui.label(&p.fix);
                    if matches!(p.kind, ProblemKind::PasscodeNeeded | ProblemKind::PasscodeWrong) {
                        ui.horizontal(|ui| {
                            ui.label("Login passcode:");
                            ui.add(egui::TextEdit::singleline(&mut w.passcode).password(true).char_limit(8).desired_width(80.0));
                            w.passcode.retain(|c| c.is_ascii_digit());
                            if ui.add_enabled(!w.passcode.is_empty(), egui::Button::new("Continue")).clicked() {
                                self.login_pin_buf = w.passcode.clone();
                                self.dirty = true;
                                if self.save() {
                                    self.start_test(&mut w);
                                } else {
                                    w.error = self.message.clone();
                                }
                            }
                        });
                        ui.weak("It's saved on this PC so you don't have to type it again.");
                    } else {
                        ui.horizontal(|ui| {
                            if ui.button("Try again").clicked() {
                                self.start_test(&mut w);
                            }
                            if p.kind == ProblemKind::NotPaired && ui.button("‹ Link again").clicked() {
                                w.step = Step::Link;
                            }
                            if ui.button("Skip test").clicked() {
                                w.step = Step::Done;
                            }
                        });
                    }
                    ui.collapsing("Details", |ui| {
                        ui.monospace(&p.detail);
                    });
                }
                error_label(ui, &w.error);
            }

            Step::Done => {
                ui.heading(RichText::new("✔  All set!").color(Color32::from_rgb(0x16, 0xa3, 0x4a)));
                ui.label(format!("This PC is linked with {}.", self.console_label()));
                ui.add_space(6.0);
                ui.label("When you connect, a small window takes over your keyboard and mouse and sends them to the PS5 as a controller:");
                ui.label(format!("   •  {}  gives you your mouse back (press again or click the window to continue playing)", self.cfg.input.capture_toggle));
                ui.label(format!("   •  {}  disconnects", self.cfg.input.quit));
                ui.label("   •  The Bindings tab shows and changes which key does what.");
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if big_button(ui, "▶  Connect now", true) {
                        keep = false;
                        self.tab = Tab::Run;
                        self.start();
                    }
                    if ui.button("Close").clicked() {
                        keep = false;
                        self.tab = Tab::Run;
                    }
                });
            }
        }

        if keep {
            self.wizard = Some(w);
        } else {
            self.account = AccountFlow::default();
        }
    }
}
