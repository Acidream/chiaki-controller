//! Settings window and tray icon: edit the console, stream and input settings and the
//! bindings, register a console, and start / stop sessions. Closing the window hides it to
//! the tray; the tray menu can show it, start and stop sessions, and quit.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use anyhow::{anyhow, bail, Result};
use eframe::egui;

use crate::config::{Binding, BindingDetail, Config, MouseTarget, Profile};
use crate::input::{keys, mapper::Mapper};
use crate::controller::ControllerState;
use crate::runner::{resolve_host, CaptureIn, Phase, ProblemKind, RunState, Runner};

mod preview;
mod wizard;
use crate::{logbuf, psn, regist, session};

// ---------------------------------------------------------------------------------------------
// Session control shared between the window and the tray handlers

struct Shared {
    runner: Option<Runner>,
    stopping: Vec<JoinHandle<()>>,
    cfg_path: PathBuf,
    /// state of the last session that ended on its own (its problem stays visible)
    last_state: Option<RunState>,
    /// the settings window, which also captures keyboard and mouse while playing
    hwnd: isize,
}

/// What the window and tray show about the session.
struct Snapshot {
    running: bool,
    stopping: bool,
    state: Option<RunState>,
    controller: Option<ControllerState>,
    /// active profile and the config of the running session
    profile: Option<(String, Arc<Config>)>,
    /// keyboard and mouse go to the PS5
    capturing: bool,
}

impl Shared {
    fn reap(&mut self) {
        if self.runner.as_ref().map(|r| r.is_finished()).unwrap_or(false) {
            if let Some(r) = self.runner.take() {
                // a session the user ended (quit key) leaves nothing to explain
                self.last_state = (!r.stop_requested()).then(|| r.run_state());
                self.stopping.push(thread::spawn(move || {
                    let _ = r.stop_and_wait();
                }));
            }
        }
        self.stopping.retain(|t| !t.is_finished());
    }

    fn running(&self) -> bool {
        self.runner.as_ref().map(|r| !r.is_finished()).unwrap_or(false)
    }

    fn start(&mut self, cfg: &Config) -> Result<()> {
        self.reap();
        if self.running() {
            return Ok(());
        }
        if !self.stopping.is_empty() {
            bail!("the previous session is still shutting down, try again in a moment");
        }
        logbuf::clear();
        self.last_state = None;
        self.runner = Some(Runner::start(cfg, CaptureIn::Window(self.hwnd))?);
        Ok(())
    }

    fn snapshot(&mut self) -> Snapshot {
        self.reap();
        self.peek()
    }

    /// Like `snapshot` without side effects (for the tray timer).
    fn peek(&self) -> Snapshot {
        // told to stop (quit key) but still finishing a network call
        let winding_down = self.runner.as_ref().map(|r| r.stop_requested() && !r.is_finished()).unwrap_or(false);
        Snapshot {
            running: self.running() && !winding_down,
            stopping: !self.stopping.is_empty() || winding_down,
            state: self.runner.as_ref().map(|r| r.run_state()).or_else(|| self.last_state.clone()),
            controller: self.runner.as_ref().filter(|r| r.is_streaming()).map(|r| r.controller_state()),
            profile: self.runner.as_ref().map(|r| (r.active_profile(), r.config())),
            capturing: self.runner.as_ref().map(|r| r.is_capturing()).unwrap_or(false),
        }
    }

    fn stop(&mut self) {
        self.last_state = None;
        if let Some(r) = self.runner.take() {
            r.request_stop();
            self.stopping.push(thread::spawn(move || {
                let _ = r.stop_and_wait();
            }));
        }
    }

    fn stop_and_wait(&mut self) {
        if let Some(r) = self.runner.take() {
            let _ = r.stop_and_wait();
        }
        for t in self.stopping.drain(..) {
            let _ = t.join();
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Window helpers (Win32: eframe can't show a hidden window from outside its frame loop)

fn show_window(hwnd: isize) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{SetForegroundWindow, ShowWindow, SW_RESTORE, SW_SHOW};
    unsafe {
        ShowWindow(hwnd as _, SW_SHOW);
        ShowWindow(hwnd as _, SW_RESTORE);
        SetForegroundWindow(hwnd as _);
    }
}

fn hide_window(hwnd: isize) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{ShowWindow, SW_HIDE};
    unsafe {
        ShowWindow(hwnd as _, SW_HIDE);
    }
}

const BLUE: (u8, u8, u8) = (0x25, 0x63, 0xeb);
const AMBER: (u8, u8, u8) = (0xf5, 0x9e, 0x0b);
const GREEN: (u8, u8, u8) = (0x16, 0xa3, 0x4a);
const RED: (u8, u8, u8) = (0xdc, 0x26, 0x26);

fn icon_rgba(size: u32) -> Vec<u8> {
    icon_rgba_color(size, BLUE)
}

/// A simple round controller-ish icon.
fn icon_rgba_color(size: u32, color: (u8, u8, u8)) -> Vec<u8> {
    let mut rgba = vec![0u8; (size * size * 4) as usize];
    let c = (size as f32 - 1.0) / 2.0;
    let r = size as f32 / 2.0 - 0.5;
    for y in 0..size {
        for x in 0..size {
            let (dx, dy) = (x as f32 - c, y as f32 - c);
            let d = (dx * dx + dy * dy).sqrt();
            let i = ((y * size + x) * 4) as usize;
            if d > r {
                continue;
            }
            let alpha = ((r - d).clamp(0.0, 1.0) * 255.0) as u8;
            // white "buttons": a cross on the left, a dot on the right
            let s = size as f32;
            let (lx, ly) = (dx + s * 0.2, dy);
            let cross = (lx.abs() < s * 0.05 && ly.abs() < s * 0.17) || (ly.abs() < s * 0.05 && lx.abs() < s * 0.17);
            let dot = ((dx - s * 0.2).powi(2) + dy * dy).sqrt() < s * 0.1;
            let (cr, cg, cb) = if cross || dot { (255, 255, 255) } else { color };
            rgba[i..i + 4].copy_from_slice(&[cr, cg, cb, alpha]);
        }
    }
    rgba
}

// ---------------------------------------------------------------------------------------------
// Tray icon status: colour and tooltip follow the session, also while the window is hidden
// (a Win32 timer on the window keeps running when eframe stops drawing).

struct TrayStatus {
    tray: tray_icon::TrayIcon,
    shared: Arc<Mutex<Shared>>,
    shown: Option<(u8, String)>,
}

thread_local! {
    static TRAY: std::cell::RefCell<Option<TrayStatus>> = const { std::cell::RefCell::new(None) };
}

const TRAY_TIMER_ID: usize = 0xC41A;

impl TrayStatus {
    fn refresh(&mut self) {
        let snap = match self.shared.try_lock() {
            Ok(sh) => sh.peek(),
            Err(_) => return,
        };
        let phase = snap.state.as_ref().map(|s| s.phase.clone());
        let problem = snap.state.as_ref().and_then(|s| s.problem.clone());
        let (level, tip) = match (snap.running, phase) {
            (true, Some(Phase::Ready)) => (2, "Connected - keyboard and mouse control the PS5".to_string()),
            (true, Some(p)) => (1, p.label()),
            (true, None) => (1, "Connecting…".to_string()),
            (false, _) if snap.stopping => (1, "Disconnecting…".to_string()),
            (false, _) => match problem {
                Some(p) => (3, p.title),
                None => (0, "Not connected".to_string()),
            },
        };
        let tip = format!("chiaki-controller: {tip}");
        let tip: String = tip.chars().take(120).collect();
        if self.shown.as_ref().map(|(l, t)| *l == level && *t == tip).unwrap_or(false) {
            return;
        }
        let color = [BLUE, AMBER, GREEN, RED][level as usize];
        if let Ok(icon) = tray_icon::Icon::from_rgba(icon_rgba_color(32, color), 32, 32) {
            let _ = self.tray.set_icon(Some(icon));
        }
        let _ = self.tray.set_tooltip(Some(tip.clone()));
        self.shown = Some((level, tip));
    }
}

unsafe extern "system" fn tray_timer(_: windows_sys::Win32::Foundation::HWND, _: u32, _: usize, _: u32) {
    TRAY.with(|t| {
        if let Ok(mut t) = t.try_borrow_mut() {
            if let Some(ts) = t.as_mut() {
                ts.refresh();
            }
        }
    });
}

// ---------------------------------------------------------------------------------------------
// Editable binding rows

#[derive(Clone, Debug, Default)]
struct BindRow {
    input: String,
    action: String,
    value: Option<f32>,
    toggle: bool,
    turbo_hz: f32,
    x: u16,
    y: u16,
}

const BUTTON_ACTIONS: &[&str] = &[
    "cross", "circle", "square", "triangle", "l1", "r1", "l2", "r2", "l3", "r3", "dpad_up", "dpad_down", "dpad_left",
    "dpad_right", "options", "create", "touchpad", "ps",
];
const STICK_ACTIONS: &[&str] = &[
    "left_stick_up",
    "left_stick_down",
    "left_stick_left",
    "left_stick_right",
    "right_stick_up",
    "right_stick_down",
    "right_stick_left",
    "right_stick_right",
    "left_stick_scale",
    "right_stick_scale",
];
const OTHER_ACTIONS: &[&str] = &["touch", "touch_left", "touch_right", "mouse_hold", "profile_next"];

fn value_default(action: &str) -> Option<f32> {
    match action {
        "l2" | "r2" => Some(1.0),
        a if a.ends_with("_scale") => Some(0.5),
        a if a.contains("_stick_") => Some(1.0),
        _ => None,
    }
}

fn rows_from_profile(p: &Profile) -> Vec<BindRow> {
    p.bind
        .iter()
        .map(|(input, b)| match b {
            Binding::Simple(a) => BindRow { input: input.clone(), action: a.clone(), x: 960, y: 471, ..Default::default() },
            Binding::Detailed(d) => BindRow {
                input: input.clone(),
                action: if d.action.eq_ignore_ascii_case("profile") {
                    format!("profile:{}", d.profile.clone().unwrap_or_default())
                } else {
                    d.action.clone()
                },
                value: d.value,
                toggle: d.toggle,
                turbo_hz: d.turbo_hz.unwrap_or(0.0),
                x: d.x.unwrap_or(960),
                y: d.y.unwrap_or(471),
            },
        })
        .collect()
}

fn binds_from_rows(rows: &[BindRow]) -> Result<BTreeMap<String, Binding>> {
    let mut out = BTreeMap::new();
    for r in rows {
        if r.input.trim().is_empty() && r.action.trim().is_empty() {
            continue;
        }
        let input = keys::canonical(&r.input).ok_or_else(|| anyhow!("unknown input {:?}", r.input))?;
        if r.action.trim().is_empty() {
            bail!("{input} has no action");
        }
        let value = r.value.filter(|v| Some(*v) != value_default(&r.action));
        let is_touch = r.action == "touch";
        let detailed = value.is_some() || r.toggle || r.turbo_hz > 0.0 || is_touch;
        let binding = if detailed {
            Binding::Detailed(BindingDetail {
                action: r.action.clone(),
                value,
                toggle: r.toggle,
                turbo_hz: (r.turbo_hz > 0.0).then_some(r.turbo_hz),
                x: is_touch.then_some(r.x),
                y: is_touch.then_some(r.y),
                profile: None,
            })
        } else {
            Binding::Simple(r.action.clone())
        };
        if out.insert(input.clone(), binding).is_some() {
            bail!("{input} is bound twice");
        }
    }
    Ok(out)
}

/// Turns an egui key name into one of ours.
fn egui_key_name(key: egui::Key) -> Option<String> {
    keys::canonical(key.name())
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum ListenTarget {
    Bind(usize),
    CaptureKey,
    QuitKey,
}

#[derive(Clone, Copy, PartialEq)]
enum Tab {
    Run,
    Console,
    Settings,
    Bindings,
}

enum TaskResult {
    Message(String),
    AccountId(Result<psn::AccountId, String>),
}

/// State of the "Get my account ID" helper.
#[derive(Default)]
struct AccountFlow {
    active: bool,
    busy: bool,
    error: String,
    paste: String,
    /// clipboard text when the flow started; only a newly copied address counts
    clipboard_baseline: Option<String>,
    last_poll: Option<std::time::Instant>,
}

pub struct GuiApp {
    shared: Arc<Mutex<Shared>>,
    quit_requested: Arc<AtomicBool>,
    quitting: bool,
    hwnd: isize,

    cfg: Config,
    rows: Vec<Vec<BindRow>>,
    dirty: bool,
    tab: Tab,
    sel_profile: usize,
    listen: Option<ListenTarget>,
    listen_armed: bool,
    prev_mods: egui::Modifiers,
    login_pin_buf: String,
    message: String,
    tasks: Vec<Receiver<TaskResult>>,
    reg_account: String,
    account: AccountFlow,
    wizard: Option<wizard::Wizard>,
    /// passcode typed on the Run tab when the console asks for one
    run_passcode: String,
    all_inputs: Vec<String>,
}

impl GuiApp {
    fn new(cc: &eframe::CreationContext<'_>, cfg_path: PathBuf) -> Result<Self> {
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        let hwnd = match cc.window_handle().map(|h| h.as_raw()) {
            Ok(RawWindowHandle::Win32(h)) => h.hwnd.get(),
            _ => bail!("could not get the settings window handle"),
        };

        let mut message = String::new();
        let cfg = if cfg_path.exists() {
            match Config::load(&cfg_path) {
                Ok(c) => c,
                Err(e) => {
                    message = format!("{e:#} - showing defaults; saving will overwrite the file");
                    Config::default_config()
                }
            }
        } else {
            let c = Config::default_config();
            match c.save(&cfg_path) {
                Ok(()) => message = format!("Created {}", cfg_path.display()),
                Err(e) => message = format!("Could not create {}: {e:#}", cfg_path.display()),
            }
            c
        };

        crate::input::windows::install_in_window(hwnd)?;
        let shared = Arc::new(Mutex::new(Shared {
            runner: None,
            stopping: Vec::new(),
            cfg_path: cfg_path.clone(),
            last_state: None,
            hwnd,
        }));
        let quit_requested = Arc::new(AtomicBool::new(false));
        match Self::create_tray(&cc.egui_ctx, hwnd, shared.clone(), quit_requested.clone()) {
            Ok(tray) => {
                TRAY.with(|t| *t.borrow_mut() = Some(TrayStatus { tray, shared: shared.clone(), shown: None }));
                unsafe {
                    windows_sys::Win32::UI::WindowsAndMessaging::SetTimer(hwnd as _, TRAY_TIMER_ID, 500, Some(tray_timer));
                }
            }
            Err(e) => log::error!("tray icon: {e:#}"),
        }

        let rows = cfg.profiles.iter().map(rows_from_profile).collect();
        let mut app = GuiApp {
            shared,
            quit_requested,
            quitting: false,
            hwnd,
            login_pin_buf: cfg.console.login_pin.clone().unwrap_or_default(),
            cfg,
            rows,
            dirty: false,
            tab: Tab::Run,
            sel_profile: 0,
            listen: None,
            listen_armed: false,
            prev_mods: egui::Modifiers::default(),
            message,
            tasks: Vec::new(),
            reg_account: String::new(),
            account: AccountFlow::default(),
            wizard: None,
            run_passcode: String::new(),
            all_inputs: keys::all_names(),
        };
        if app.cfg.credentials().is_err() {
            // nothing linked yet: guide the user through it
            app.open_wizard();
        }
        if app.cfg.ui.connect_on_launch {
            app.start();
        }
        Ok(app)
    }

    fn create_tray(
        ctx: &egui::Context,
        hwnd: isize,
        shared: Arc<Mutex<Shared>>,
        quit_requested: Arc<AtomicBool>,
    ) -> Result<tray_icon::TrayIcon> {
        use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
        use tray_icon::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};

        let show = MenuItem::new("Open chiaki-controller", true, None);
        let start = MenuItem::new("Connect", true, None);
        let stop = MenuItem::new("Disconnect", true, None);
        let quit = MenuItem::new("Quit", true, None);
        let menu = Menu::new();
        menu.append_items(&[&show, &PredefinedMenuItem::separator(), &start, &stop, &PredefinedMenuItem::separator(), &quit])?;
        let icon = tray_icon::Icon::from_rgba(icon_rgba(32), 32, 32)?;
        let tray = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip("chiaki-controller")
            .with_icon(icon)
            .with_menu_on_left_click(false)
            .build()?;

        let (show_id, start_id, stop_id, quit_id) = (show.id().clone(), start.id().clone(), stop.id().clone(), quit.id().clone());
        let menu_ctx = ctx.clone();
        MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
            if e.id == show_id {
                show_window(hwnd);
            } else if e.id == start_id {
                // the window captures keyboard and mouse, so it has to be in front
                show_window(hwnd);
                let mut sh = shared.lock().unwrap();
                let result = Config::load(&sh.cfg_path).and_then(|cfg| sh.start(&cfg));
                if let Err(e) = result {
                    log::error!("start: {e:#}");
                    drop(sh);
                    show_window(hwnd);
                }
            } else if e.id == stop_id {
                shared.lock().unwrap().stop();
            } else if e.id == quit_id {
                quit_requested.store(true, Ordering::Relaxed);
                // the frame loop only runs while the window is visible
                show_window(hwnd);
            }
            menu_ctx.request_repaint();
        }));
        let tray_ctx = ctx.clone();
        TrayIconEvent::set_event_handler(Some(move |e: TrayIconEvent| {
            let show = matches!(
                e,
                TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. }
                    | TrayIconEvent::DoubleClick { button: MouseButton::Left, .. }
            );
            if show {
                show_window(hwnd);
                tray_ctx.request_repaint();
            }
        }));
        Ok(tray)
    }

    /// The config as edited, with bindings rebuilt from the rows, validated.
    fn built_config(&self) -> Result<Config> {
        let mut cfg = self.cfg.clone();
        cfg.console.login_pin = (!self.login_pin_buf.trim().is_empty()).then(|| self.login_pin_buf.trim().to_string());
        for (p, rows) in cfg.profiles.iter_mut().zip(&self.rows) {
            p.bind = binds_from_rows(rows).map_err(|e| anyhow!("profile {:?}: {e:#}", p.name))?;
        }
        let mut names: Vec<String> = cfg.profiles.iter().map(|p| p.name.to_lowercase()).collect();
        names.sort();
        names.dedup();
        if names.len() != cfg.profiles.len() {
            bail!("two profiles have the same name");
        }
        if cfg.profiles.iter().any(|p| p.name.trim().is_empty()) {
            bail!("a profile has no name");
        }
        Mapper::new(&cfg)?;
        keys::parse(&cfg.input.capture_toggle).ok_or_else(|| anyhow!("capture toggle key {:?} is unknown", cfg.input.capture_toggle))?;
        keys::parse(&cfg.input.quit).ok_or_else(|| anyhow!("quit key {:?} is unknown", cfg.input.quit))?;
        crate::stream::VideoProfile::preset(&cfg.stream.resolution, cfg.stream.fps)?;
        Ok(cfg)
    }

    fn save(&mut self) -> bool {
        let path = self.shared.lock().unwrap().cfg_path.clone();
        match self.built_config().and_then(|cfg| cfg.save(&path).map(|_| cfg)) {
            Ok(cfg) => {
                self.cfg = cfg;
                self.dirty = false;
                self.message = format!("Saved {}", path.display());
                true
            }
            Err(e) => {
                self.message = format!("Not saved: {e:#}");
                false
            }
        }
    }

    fn start(&mut self) {
        if self.dirty && !self.save() {
            return;
        }
        let cfg = self.cfg.clone();
        match self.shared.lock().unwrap().start(&cfg) {
            Ok(()) => self.message = "Session started".into(),
            Err(e) => self.message = format!("Can't start: {e:#}"),
        }
    }

    fn spawn_task(&mut self, f: impl FnOnce() -> TaskResult + Send + 'static) {
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let _ = tx.send(f());
        });
        self.tasks.push(rx);
    }

    fn poll_tasks(&mut self) {
        let mut done = Vec::new();
        for (i, rx) in self.tasks.iter().enumerate() {
            match rx.try_recv() {
                Ok(r) => done.push((i, Some(r))),
                Err(mpsc::TryRecvError::Disconnected) => done.push((i, None)),
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        for (i, r) in done.into_iter().rev() {
            self.tasks.remove(i);
            match r {
                Some(TaskResult::Message(m)) => self.message = m,
                Some(TaskResult::AccountId(r)) => {
                    self.account.busy = false;
                    match r {
                        Ok(id) => {
                            self.reg_account = id.base64;
                            self.account = AccountFlow::default();
                            self.message = "Got your PSN account ID from Sony".into();
                        }
                        Err(e) => self.account.error = e,
                    }
                }
                None => {}
            }
        }
    }

    fn exchange_redirect(&mut self, url: &str) {
        match psn::code_from_redirect(url) {
            Ok(code) => {
                self.account.busy = true;
                self.account.error.clear();
                self.account.paste.clear();
                self.spawn_task(move || TaskResult::AccountId(psn::account_id_from_code(&code).map_err(|e| format!("{e:#}"))));
            }
            Err(e) => self.account.error = format!("{e:#}"),
        }
    }

    /// While the account helper waits, pick the redirect address up from the clipboard.
    fn poll_account_clipboard(&mut self) {
        if !self.account.active || self.account.busy {
            return;
        }
        let now = std::time::Instant::now();
        if self.account.last_poll.map(|t| now.duration_since(t) < Duration::from_millis(400)).unwrap_or(false) {
            return;
        }
        self.account.last_poll = Some(now);
        let Some(text) = psn::clipboard::text() else { return };
        if Some(&text) == self.account.clipboard_baseline.as_ref() || !psn::looks_like_redirect(&text) {
            return;
        }
        // the code is single use; don't leave it lying around on the clipboard
        psn::clipboard::clear();
        self.account.clipboard_baseline = None;
        self.exchange_redirect(&text);
    }

    fn handle_listen(&mut self, ctx: &egui::Context) {
        let Some(target) = self.listen else { return };
        if !self.listen_armed {
            // skip the frame of the click that started listening
            self.listen_armed = true;
            self.prev_mods = ctx.input(|i| i.modifiers);
            return;
        }
        let prev = self.prev_mods;
        let got = ctx.input(|i| {
            let mut got = None;
            for ev in &i.events {
                match ev {
                    egui::Event::Key { key, pressed: true, repeat: false, .. } => got = got.or_else(|| egui_key_name(*key)),
                    egui::Event::PointerButton { button, pressed: true, .. } => {
                        let n = match button {
                            egui::PointerButton::Primary => "MouseLeft",
                            egui::PointerButton::Secondary => "MouseRight",
                            egui::PointerButton::Middle => "MouseMiddle",
                            egui::PointerButton::Extra1 => "Mouse4",
                            egui::PointerButton::Extra2 => "Mouse5",
                        };
                        got = got.or(Some(n.to_string()));
                    }
                    egui::Event::MouseWheel { delta, .. } => {
                        let n = if delta.y > 0.0 {
                            "WheelUp"
                        } else if delta.y < 0.0 {
                            "WheelDown"
                        } else if delta.x > 0.0 {
                            "WheelRight"
                        } else {
                            "WheelLeft"
                        };
                        got = got.or(Some(n.to_string()));
                    }
                    _ => {}
                }
            }
            let m = i.modifiers;
            if got.is_none() {
                if m.shift && !prev.shift {
                    got = Some("LShift".into());
                } else if m.ctrl && !prev.ctrl {
                    got = Some("LCtrl".into());
                } else if m.alt && !prev.alt {
                    got = Some("LAlt".into());
                }
            }
            got
        });
        self.prev_mods = ctx.input(|i| i.modifiers);
        if let Some(name) = got {
            match target {
                ListenTarget::Bind(row) => {
                    if let Some(r) = self.rows.get_mut(self.sel_profile).and_then(|rows| rows.get_mut(row)) {
                        r.input = name;
                    }
                }
                ListenTarget::CaptureKey => self.cfg.input.capture_toggle = name,
                ListenTarget::QuitKey => self.cfg.input.quit = name,
            }
            self.dirty = true;
            self.listen = None;
        }
    }

    // --- UI pieces ------------------------------------------------------------------------

    fn input_picker(&mut self, ui: &mut egui::Ui, id: impl std::hash::Hash, target: ListenTarget) {
        let current = match target {
            ListenTarget::Bind(i) => self.rows[self.sel_profile][i].input.clone(),
            ListenTarget::CaptureKey => self.cfg.input.capture_toggle.clone(),
            ListenTarget::QuitKey => self.cfg.input.quit.clone(),
        };
        let mut chosen = None;
        let unknown = !current.is_empty() && keys::canonical(&current).is_none();
        let label = if current.is_empty() { "(choose)".to_string() } else { current.clone() };
        let label = if unknown { egui::RichText::new(label).color(egui::Color32::RED) } else { egui::RichText::new(label) };
        egui::ComboBox::from_id_salt(id).selected_text(label).width(120.0).height(320.0).show_ui(ui, |ui| {
            for n in &self.all_inputs {
                if ui.selectable_label(*n == current, n).clicked() {
                    chosen = Some(n.clone());
                }
            }
        });
        let listening = self.listen == Some(target);
        let btn = ui
            .add(egui::Button::new(if listening { "press a key…" } else { "set" }).selected(listening))
            .on_hover_text("Click, then press the key / mouse button / wheel to bind");
        if btn.clicked() {
            if listening {
                self.listen = None;
            } else {
                self.listen = Some(target);
                self.listen_armed = false;
            }
        }
        if let Some(n) = chosen {
            match target {
                ListenTarget::Bind(i) => self.rows[self.sel_profile][i].input = n,
                ListenTarget::CaptureKey => self.cfg.input.capture_toggle = n,
                ListenTarget::QuitKey => self.cfg.input.quit = n,
            }
            self.dirty = true;
        }
    }

    fn console_label(&self) -> String {
        let c = &self.cfg.console;
        match (c.nickname.trim().is_empty(), c.host.trim().is_empty()) {
            (false, _) => c.nickname.clone(),
            (true, false) => c.host.clone(),
            (true, true) => "your PS5".into(),
        }
    }

    fn save_and_restart(&mut self) {
        if !self.save() {
            return;
        }
        self.shared.lock().unwrap().stop();
        self.message = "Saved; reconnecting…".into();
        let shared = self.shared.clone();
        let cfg = self.cfg.clone();
        thread::spawn(move || {
            for _ in 0..100 {
                thread::sleep(Duration::from_millis(100));
                let mut sh = shared.lock().unwrap();
                sh.reap();
                if sh.stopping.is_empty() {
                    if let Err(e) = sh.start(&cfg) {
                        log::error!("restart: {e:#}");
                    }
                    break;
                }
            }
        });
    }

    /// Says whether keyboard and mouse go to the PS5, and how to switch.
    fn capture_banner(&mut self, ui: &mut egui::Ui, snap: &Snapshot, ready: bool) {
        let (capture, quit) = (&self.cfg.input.capture_toggle, &self.cfg.input.quit);
        let (tint, headline, sub) = match (snap.capturing, ready) {
            (true, true) => (GREEN, "🎮  Keyboard and mouse control your PS5".to_string(), format!("{capture} gives you your mouse back  ·  {quit} disconnects")),
            (true, false) => (
                AMBER,
                "Waiting for the PS5…".to_string(),
                format!("Keyboard and mouse are captured and go to the PS5 once connected  ·  {capture} releases"),
            ),
            (false, _) => (
                (0x47, 0x4b, 0x55),
                "Mouse released: the PS5 gets no input".to_string(),
                format!("Press {capture} or the button to control the PS5 again  ·  {quit} disconnects"),
            ),
        };
        let color = egui::Color32::from_rgb(tint.0, tint.1, tint.2);
        egui::Frame::none().fill(color).rounding(6.0).inner_margin(egui::Margin::symmetric(14.0, 10.0)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.label(egui::RichText::new(headline).size(18.0).strong().color(egui::Color32::WHITE));
                    ui.label(egui::RichText::new(sub).color(egui::Color32::WHITE));
                });
                if !snap.capturing {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let b = egui::Button::new(egui::RichText::new(format!("🎮  Control the PS5 ({capture})")).size(15.0))
                            .min_size(egui::vec2(0.0, 32.0));
                        if ui.add(b).clicked() {
                            if let Some(r) = &self.shared.lock().unwrap().runner {
                                r.set_capture(true);
                            }
                        }
                    });
                }
            });
        });
        ui.add_space(8.0);
    }

    fn ui_run(&mut self, ui: &mut egui::Ui) {
        if self.cfg.credentials().is_err() || self.cfg.console.host.trim().is_empty() {
            ui.add_space(24.0);
            ui.heading("No PS5 linked yet");
            ui.label("Link this PC with your PS5 once. After that, one click connects.");
            ui.add_space(8.0);
            if ui.add(egui::Button::new(egui::RichText::new("Set up my PS5…").size(16.0)).min_size(egui::vec2(180.0, 36.0))).clicked() {
                self.open_wizard();
            }
            return;
        }

        let snap = self.shared.lock().unwrap().snapshot();
        let name = self.console_label();
        let phase = snap.state.as_ref().map(|s| s.phase.clone()).unwrap_or(Phase::Stopped);
        let problem = snap.state.as_ref().and_then(|s| s.problem.clone()).filter(|_| phase != Phase::Ready && !snap.stopping);

        let (color, title) = if snap.running && phase == Phase::Ready {
            (egui::Color32::from_rgb(GREEN.0, GREEN.1, GREEN.2), format!("Connected to {name}"))
        } else if snap.running {
            (egui::Color32::from_rgb(AMBER.0, AMBER.1, AMBER.2), phase.label())
        } else if snap.stopping {
            (egui::Color32::GRAY, "Disconnecting…".to_string())
        } else if problem.is_some() {
            (egui::Color32::from_rgb(RED.0, RED.1, RED.2), "Not connected".to_string())
        } else {
            (egui::Color32::GRAY, "Not connected".to_string())
        };

        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("⏺").size(26.0).color(color));
            ui.vertical(|ui| {
                ui.label(egui::RichText::new(&title).size(22.0).strong());
                ui.weak(format!("{name}  ·  {}", self.cfg.console.host));
            });
        });
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            let big = |t: &str| egui::Button::new(egui::RichText::new(t).size(17.0)).min_size(egui::vec2(170.0, 38.0));
            if snap.running {
                if ui.add(big("⏹  Disconnect")).clicked() {
                    self.shared.lock().unwrap().stop();
                }
                if self.dirty && ui.button("Save changes & reconnect").clicked() {
                    self.save_and_restart();
                }
            } else if ui.add_enabled(!snap.stopping, big("▶  Connect")).clicked() {
                self.start();
            }
        });

        if let Some(p) = &problem {
            ui.add_space(10.0);
            let stopped = !snap.running;
            let tint = if stopped { RED } else { AMBER };
            egui::Frame::group(ui.style())
                .fill(egui::Color32::from_rgb(tint.0, tint.1, tint.2).gamma_multiply(0.12))
                .stroke(egui::Stroke::new(1.0_f32, egui::Color32::from_rgb(tint.0, tint.1, tint.2)))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(egui::RichText::new(&p.title).strong().size(16.0));
                    ui.label(&p.fix);
                    match p.kind {
                        ProblemKind::PasscodeNeeded | ProblemKind::PasscodeWrong => {
                            ui.horizontal(|ui| {
                                ui.label("Login passcode:");
                                ui.add(egui::TextEdit::singleline(&mut self.run_passcode).password(true).char_limit(8).desired_width(80.0));
                                self.run_passcode.retain(|c| c.is_ascii_digit());
                                if ui.add_enabled(!self.run_passcode.is_empty(), egui::Button::new("Save & connect")).clicked() {
                                    self.login_pin_buf = std::mem::take(&mut self.run_passcode);
                                    self.dirty = true;
                                    self.start();
                                }
                            });
                        }
                        ProblemKind::NotPaired => {
                            if ui.button("Set up again…").clicked() {
                                self.open_wizard();
                            }
                        }
                        _ => {}
                    }
                    ui.collapsing("Technical details", |ui| {
                        ui.monospace(&p.detail);
                    });
                });
        }

        ui.add_space(10.0);
        if snap.running {
            self.capture_banner(ui, &snap, phase == Phase::Ready);
        }
        if snap.running && phase == Phase::Ready {
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.set_width(ui.available_width());
                if let Some(cs) = &snap.controller {
                    ui.horizontal_top(|ui| {
                        let left = (ui.available_width() * 0.52).clamp(260.0, 420.0);
                        ui.allocate_ui(egui::vec2(left, 0.0), |ui| {
                            ui.vertical(|ui| {
                                preview::controller(ui, cs);
                                ui.weak("Live: what the PS5 receives. Press keys or move the mouse while capturing to see it.");
                            });
                        });
                        ui.separator();
                        if let Some((name, cfg)) = &snap.profile {
                            ui.vertical(|ui| preview::bindings_panel(ui, cfg, name, cs));
                        }
                    });
                }
            });
        } else if !snap.running && problem.is_none() {
            ui.label("Press Connect to play. This window then takes over your keyboard and mouse:");
            ui.label(format!("   •  {}  gives you your mouse back, and press it again to continue playing", self.cfg.input.capture_toggle));
            ui.label(format!("   •  {}  disconnects", self.cfg.input.quit));
            ui.label("   •  The Bindings tab shows which key does what.");
        }

        ui.add_space(10.0);
        egui::CollapsingHeader::new("Technical log").default_open(false).show(ui, |ui| {
            if ui.small_button("Clear").clicked() {
                logbuf::clear();
            }
            egui::ScrollArea::vertical().stick_to_bottom(true).max_height(220.0).auto_shrink([false, true]).show(ui, |ui| {
                for line in logbuf::lines() {
                    ui.label(egui::RichText::new(line).monospace().size(11.0));
                }
            });
        });
    }

    fn ui_console(&mut self, ui: &mut egui::Ui) {
        let linked = self.cfg.credentials().is_ok();
        ui.add_space(4.0);
        if linked {
            ui.label(egui::RichText::new(format!("🎮  {}", self.console_label())).size(20.0).strong());
            ui.weak(format!("Linked · address {}", self.cfg.console.host));
        } else {
            ui.label(egui::RichText::new("No PS5 linked yet").size(20.0).strong());
        }
        ui.add_space(6.0);
        if ui.button(if linked { "Link a different PS5…" } else { "Set up my PS5…" }).clicked() {
            self.open_wizard();
        }
        ui.add_space(10.0);

        let mut changed = false;
        egui::Grid::new("console_basic").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
            ui.label("Login passcode");
            changed |= ui
                .add(egui::TextEdit::singleline(&mut self.login_pin_buf).password(true).hint_text("only if your PS5 asks for one"))
                .changed();
            ui.end_row();
            ui.label("");
            changed |= ui.checkbox(&mut self.cfg.console.wake, "Wake the PS5 from rest mode when connecting").changed();
            ui.end_row();
        });
        self.dirty |= changed;

        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let ready = self.built_config().ok().and_then(|cfg| {
                let creds = cfg.credentials().ok()?;
                let ip = resolve_host(&cfg.console.host).ok()?;
                Some((cfg, creds, ip))
            });
            let host = self.cfg.console.host.clone();
            if ui.button("Check status").clicked() {
                self.spawn_task(move || {
                    TaskResult::Message(match resolve_host(&host).and_then(regist::probe) {
                        Ok(s) => format!("Console: {s}"),
                        Err(e) => format!("Status: {e:#}"),
                    })
                });
            }
            if ui.add_enabled(ready.is_some(), egui::Button::new("Wake up")).clicked() {
                let (_, creds, ip) = ready.clone().unwrap();
                self.spawn_task(move || {
                    TaskResult::Message(match regist::wakeup(ip, &creds) {
                        Ok(()) => "Wakeup sent".into(),
                        Err(e) => format!("Wakeup: {e:#}"),
                    })
                });
            }
            if ui.add_enabled(ready.is_some(), egui::Button::new("Rest mode")).clicked() {
                let (cfg, creds, ip) = ready.unwrap();
                self.spawn_task(move || {
                    let r = (|| -> Result<()> {
                        let nonce = session::request_session(ip, &creds)?;
                        let ctrl = session::Ctrl::connect(ip, &nonce, &creds, cfg.console.login_pin.as_deref(), false)?;
                        ctrl.goto_bed()?;
                        thread::sleep(Duration::from_millis(500));
                        Ok(())
                    })();
                    TaskResult::Message(match r {
                        Ok(()) => "Rest mode requested".into(),
                        Err(e) => format!("Rest mode: {e:#}"),
                    })
                });
            }
        });

        ui.add_space(12.0);
        egui::CollapsingHeader::new("Advanced: address and pairing keys").default_open(false).show(ui, |ui| {
            ui.weak("Filled in by the setup. Change them only if you know what you're doing.");
            let c = &mut self.cfg.console;
            let mut changed = false;
            egui::Grid::new("console_adv").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
                ui.label("Console address");
                changed |= ui.text_edit_singleline(&mut c.host).changed();
                ui.end_row();
                ui.label("Name");
                changed |= ui.text_edit_singleline(&mut c.nickname).changed();
                ui.end_row();
                ui.label("Regist key");
                changed |= ui.add(egui::TextEdit::singleline(&mut c.regist_key).font(egui::TextStyle::Monospace)).changed();
                ui.end_row();
                ui.label("RP key");
                changed |= ui
                    .add(egui::TextEdit::singleline(&mut c.rp_key).font(egui::TextStyle::Monospace).desired_width(280.0))
                    .changed();
                ui.end_row();
            });
            self.dirty |= changed;
        });

    }

    fn ui_account_flow(&mut self, ui: &mut egui::Ui) {
        ui.strong("Get your PSN account ID");
        ui.label("1. Your browser opened Sony's sign-in page. Sign in there. Your password goes only to Sony, never to this app.");
        ui.horizontal(|ui| {
            ui.label("   Check the address bar shows");
            ui.monospace("auth.api.sonyentertainmentnetwork.com");
            if ui.small_button("open again").clicked() {
                if let Err(e) = psn::open_login_page() {
                    self.account.error = format!("{e:#}");
                }
            }
        });
        ui.label("2. After signing in, the page just says \"redirect\". Copy the address from the address bar (Ctrl+L, Ctrl+C).");
        ui.label("3. That's it: this app notices the copied address and asks Sony for your account ID. Or paste it here:");
        ui.horizontal(|ui| {
            ui.add_enabled(
                !self.account.busy,
                egui::TextEdit::singleline(&mut self.account.paste)
                    .hint_text("https://remoteplay.dl.playstation.net/remoteplay/redirect?code=…")
                    .desired_width(380.0),
            );
            if ui.add_enabled(!self.account.busy && !self.account.paste.trim().is_empty(), egui::Button::new("Use")).clicked() {
                let url = self.account.paste.clone();
                self.exchange_redirect(&url);
            }
            if ui.button("Cancel").clicked() {
                self.account = AccountFlow::default();
            }
        });
        if self.account.busy {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Asking Sony for your account ID…");
            });
        } else if self.account.active {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.weak("Waiting for the copied address…");
            });
        }
        if !self.account.error.is_empty() {
            ui.colored_label(egui::Color32::from_rgb(0xdc, 0x26, 0x26), &self.account.error);
        }
        ui.weak("The one-time code is used once and never saved. Only your account ID is kept, in the field above.");
    }

    fn ui_settings(&mut self, ui: &mut egui::Ui) {
        let mut changed = false;
        ui.strong("Stream");
        egui::Grid::new("stream").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
            let s = &mut self.cfg.stream;
            ui.label("Requested video");
            ui.horizontal(|ui| {
                egui::ComboBox::from_id_salt("res").selected_text(s.resolution.clone()).show_ui(ui, |ui| {
                    for r in ["360p", "540p", "720p", "1080p"] {
                        changed |= ui.selectable_value(&mut s.resolution, r.to_string(), r).changed();
                    }
                });
                egui::ComboBox::from_id_salt("fps").selected_text(format!("{} fps", s.fps)).show_ui(ui, |ui| {
                    for f in [30, 60] {
                        changed |= ui.selectable_value(&mut s.fps, f, format!("{f} fps")).changed();
                    }
                });
                ui.label("(never decoded; smallest is best)");
            });
            ui.end_row();
            ui.label("Bitrate");
            ui.horizontal(|ui| {
                let mut custom = s.bitrate_kbps.is_some();
                if ui.checkbox(&mut custom, "override").changed() {
                    s.bitrate_kbps = custom.then_some(2000);
                    changed = true;
                }
                if let Some(b) = s.bitrate_kbps.as_mut() {
                    changed |= ui.add(egui::DragValue::new(b).range(500..=50000).suffix(" kbps")).changed();
                }
            });
            ui.end_row();
            ui.label("Controller");
            ui.horizontal(|ui| {
                changed |= ui.radio_value(&mut s.dualsense, true, "DualSense").changed();
                changed |= ui.radio_value(&mut s.dualsense, false, "DualShock 4").changed();
            });
            ui.end_row();
            ui.label("");
            changed |= ui.checkbox(&mut s.reconnect, "Reconnect automatically").changed();
            ui.end_row();
        });

        ui.add_space(8.0);
        ui.strong("Input");
        egui::Grid::new("input").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
            ui.label("Capture toggle key");
            ui.horizontal(|ui| self.input_picker(ui, "capkey", ListenTarget::CaptureKey));
            ui.end_row();
            ui.label("End session key");
            ui.horizontal(|ui| self.input_picker(ui, "quitkey", ListenTarget::QuitKey));
            ui.end_row();
            let i = &mut self.cfg.input;
            ui.label("");
            changed |= ui.checkbox(&mut i.capture_on_start, "Capture keyboard and mouse when a session starts").changed();
            ui.end_row();
            ui.label("Wheel press length");
            changed |= ui.add(egui::DragValue::new(&mut i.wheel_press_ms).range(10..=500).suffix(" ms")).changed();
            ui.end_row();
            ui.label("Mouse update rate");
            changed |= ui.add(egui::DragValue::new(&mut i.tick_hz).range(50..=1000).suffix(" Hz")).changed();
            ui.end_row();
            ui.label("Start profile");
            let names: Vec<String> = self.cfg.profiles.iter().map(|p| p.name.clone()).collect();
            let cur = i.start_profile.clone().unwrap_or_else(|| "(first)".into());
            egui::ComboBox::from_id_salt("startprof").selected_text(cur).show_ui(ui, |ui| {
                changed |= ui.selectable_value(&mut i.start_profile, None, "(first)").changed();
                for n in names {
                    changed |= ui.selectable_value(&mut i.start_profile, Some(n.clone()), n).changed();
                }
            });
            ui.end_row();
        });

        ui.add_space(8.0);
        ui.strong("Program");
        changed |= ui.checkbox(&mut self.cfg.ui.start_in_tray, "Start in the tray (no settings window)").changed();
        changed |= ui.checkbox(&mut self.cfg.ui.connect_on_launch, "Start a session when the program starts").changed();
        self.dirty |= changed;
    }

    fn ui_bindings(&mut self, ui: &mut egui::Ui) {
        if self.sel_profile >= self.cfg.profiles.len() {
            self.sel_profile = 0;
        }
        // profile selector
        ui.horizontal_wrapped(|ui| {
            ui.label("Profile:");
            for (i, p) in self.cfg.profiles.iter().enumerate() {
                if ui.selectable_label(self.sel_profile == i, &p.name).clicked() {
                    self.sel_profile = i;
                    self.listen = None;
                }
            }
            ui.separator();
            if ui.button("➕ New").clicked() {
                let mut n = 1;
                while self.cfg.profiles.iter().any(|p| p.name == format!("profile{n}")) {
                    n += 1;
                }
                let mut p = self.cfg.profiles[self.sel_profile].clone();
                p.name = format!("profile{n}");
                p.bind.clear();
                self.cfg.profiles.push(p);
                self.rows.push(Vec::new());
                self.sel_profile = self.cfg.profiles.len() - 1;
                self.dirty = true;
            }
            if ui.button("⧉ Duplicate").clicked() {
                let mut p = self.cfg.profiles[self.sel_profile].clone();
                p.name = format!("{} copy", p.name);
                self.cfg.profiles.push(p);
                self.rows.push(self.rows[self.sel_profile].clone());
                self.sel_profile = self.cfg.profiles.len() - 1;
                self.dirty = true;
            }
            if ui.add_enabled(self.cfg.profiles.len() > 1, egui::Button::new("🗑 Delete")).clicked() {
                self.cfg.profiles.remove(self.sel_profile);
                self.rows.remove(self.sel_profile);
                self.sel_profile = self.sel_profile.saturating_sub(1);
                self.dirty = true;
                return;
            }
        });
        let sp = self.sel_profile;

        ui.horizontal(|ui| {
            ui.label("Name");
            let old = self.cfg.profiles[sp].name.clone();
            if ui.text_edit_singleline(&mut self.cfg.profiles[sp].name).changed() {
                let new = self.cfg.profiles[sp].name.clone();
                // keep "profile:<name>" bindings pointing at the renamed profile
                for rows in &mut self.rows {
                    for r in rows.iter_mut() {
                        if r.action == format!("profile:{old}") {
                            r.action = format!("profile:{new}");
                        }
                    }
                }
                if self.cfg.input.start_profile.as_deref() == Some(old.as_str()) {
                    self.cfg.input.start_profile = Some(new);
                }
                self.dirty = true;
            }
            self.dirty |= ui.checkbox(&mut self.cfg.profiles[sp].normalize_diagonals, "round stick diagonals").changed();
        });

        egui::CollapsingHeader::new("Mouse").default_open(true).show(ui, |ui| {
            let m = &mut self.cfg.profiles[sp].mouse;
            let mut changed = false;
            egui::Grid::new("mouse").num_columns(4).spacing([12.0, 4.0]).show(ui, |ui| {
                ui.label("Mouse moves");
                egui::ComboBox::from_id_salt("mtarget")
                    .selected_text(match m.target {
                        MouseTarget::RightStick => "right stick",
                        MouseTarget::LeftStick => "left stick",
                        MouseTarget::Gyro => "gyro (experimental)",
                        MouseTarget::None => "nothing",
                    })
                    .show_ui(ui, |ui| {
                        changed |= ui.selectable_value(&mut m.target, MouseTarget::RightStick, "right stick").changed();
                        changed |= ui.selectable_value(&mut m.target, MouseTarget::LeftStick, "left stick").changed();
                        changed |= ui.selectable_value(&mut m.target, MouseTarget::Gyro, "gyro (experimental)").changed();
                        changed |= ui.selectable_value(&mut m.target, MouseTarget::None, "nothing").changed();
                    });
                changed |= ui.checkbox(&mut m.invert_y, "invert Y").changed();
                ui.end_row();
                if m.target == MouseTarget::Gyro {
                    ui.label("Gyro sensitivity");
                    changed |= ui.add(egui::Slider::new(&mut m.gyro_sensitivity, 0.05..=10.0).logarithmic(true)).changed();
                } else {
                    ui.label("Sensitivity");
                    changed |= ui.add(egui::Slider::new(&mut m.sensitivity, 0.01..=2.0).logarithmic(true)).changed();
                }
                ui.label("Smoothing");
                changed |= ui.add(egui::Slider::new(&mut m.smoothing_ms, 1.0..=40.0).suffix(" ms")).changed();
                ui.end_row();
                ui.label("X multiplier");
                changed |= ui.add(egui::Slider::new(&mut m.sensitivity_x, -3.0..=3.0)).changed();
                ui.label("Y multiplier");
                changed |= ui.add(egui::Slider::new(&mut m.sensitivity_y, -3.0..=3.0)).changed();
                ui.end_row();
                ui.label("Anti dead zone");
                changed |= ui.add(egui::Slider::new(&mut m.anti_deadzone, 0.0..=0.9)).changed();
                ui.label("Curve");
                changed |= ui.add(egui::Slider::new(&mut m.curve, 0.3..=3.0)).changed();
                ui.end_row();
                ui.label("Max deflection");
                changed |= ui.add(egui::Slider::new(&mut m.max, 0.1..=1.0)).changed();
                ui.end_row();
            });
            self.dirty |= changed;
        });

        ui.separator();
        let profile_actions: Vec<String> =
            self.cfg.profiles.iter().map(|p| format!("profile:{}", p.name)).collect();
        let mut seen: BTreeMap<String, usize> = BTreeMap::new();
        for r in &self.rows[sp] {
            if let Some(c) = keys::canonical(&r.input) {
                *seen.entry(c).or_default() += 1;
            }
        }

        let mut delete = None;
        egui::ScrollArea::vertical().auto_shrink([false, false]).max_height(ui.available_height() - 30.0).show(ui, |ui| {
            egui::Grid::new("binds").striped(true).num_columns(6).spacing([10.0, 4.0]).show(ui, |ui| {
                ui.strong("Input");
                ui.strong("Action");
                ui.strong("Amount");
                ui.strong("Toggle");
                ui.strong("Turbo");
                ui.strong("");
                ui.end_row();
                for i in 0..self.rows[sp].len() {
                    ui.horizontal(|ui| {
                        self.input_picker(ui, ("bind_in", sp, i), ListenTarget::Bind(i));
                        let dup = keys::canonical(&self.rows[sp][i].input).map(|c| seen.get(&c).copied().unwrap_or(0) > 1).unwrap_or(false);
                        if dup {
                            ui.colored_label(egui::Color32::RED, "dup").on_hover_text("This input is bound more than once");
                        }
                    });
                    let row = &mut self.rows[sp][i];
                    let mut changed = false;
                    egui::ComboBox::from_id_salt(("bind_act", sp, i))
                        .selected_text(if row.action.is_empty() { "(choose)".to_string() } else { row.action.clone() })
                        .width(150.0)
                        .height(360.0)
                        .show_ui(ui, |ui| {
                            for group in [BUTTON_ACTIONS, STICK_ACTIONS, OTHER_ACTIONS] {
                                for a in group {
                                    changed |= ui.selectable_value(&mut row.action, a.to_string(), *a).changed();
                                }
                                ui.separator();
                            }
                            for a in &profile_actions {
                                changed |= ui.selectable_value(&mut row.action, a.clone(), a).changed();
                            }
                        });
                    if changed {
                        row.value = None;
                    }
                    ui.horizontal(|ui| {
                        if let Some(def) = value_default(&row.action) {
                            let mut v = row.value.unwrap_or(def);
                            if ui.add(egui::DragValue::new(&mut v).range(0.0..=1.0).speed(0.01).fixed_decimals(2)).changed() {
                                row.value = Some(v);
                                changed = true;
                            }
                        } else if row.action == "touch" {
                            changed |= ui.add(egui::DragValue::new(&mut row.x).range(0..=1919).prefix("x ")).changed();
                            changed |= ui.add(egui::DragValue::new(&mut row.y).range(0..=941).prefix("y ")).changed();
                        } else {
                            ui.weak("—");
                        }
                    });
                    let is_profile = row.action.starts_with("profile");
                    changed |= ui.add_enabled(!is_profile, egui::Checkbox::without_text(&mut row.toggle)).changed();
                    changed |= ui
                        .add_enabled(
                            !is_profile,
                            egui::DragValue::new(&mut row.turbo_hz).range(0.0..=30.0).speed(0.2).custom_formatter(|v, _| {
                                if v <= 0.0 {
                                    "off".into()
                                } else {
                                    format!("{v:.0} Hz")
                                }
                            }),
                        )
                        .changed();
                    if ui.small_button("✖").on_hover_text("Remove").clicked() {
                        delete = Some(i);
                    }
                    self.dirty |= changed;
                    ui.end_row();
                }
            });
        });
        if let Some(i) = delete {
            self.rows[sp].remove(i);
            if matches!(self.listen, Some(ListenTarget::Bind(_))) {
                self.listen = None;
            }
            self.dirty = true;
        }
        ui.horizontal(|ui| {
            if ui.button("➕ Add binding").clicked() {
                self.rows[sp].push(BindRow { x: 960, y: 471, ..Default::default() });
                self.listen = Some(ListenTarget::Bind(self.rows[sp].len() - 1));
                self.listen_armed = false;
                self.dirty = true;
            }
            ui.weak("Tip: click \"set\", then press a key, mouse button or turn the wheel.");
        });
    }
}

impl eframe::App for GuiApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.quit_requested.swap(false, Ordering::Relaxed) {
            self.quitting = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        if ctx.input(|i| i.viewport().close_requested()) && !self.quitting {
            // closing the window keeps the program in the tray
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            hide_window(self.hwnd);
        }
        if crate::input::windows::is_capturing() {
            // keyboard and mouse belong to the PS5: show what they do
            self.tab = Tab::Run;
        }
        self.poll_tasks();
        self.poll_account_clipboard();
        self.handle_listen(ctx);

        egui::TopBottomPanel::top("top").show(ctx, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.add_enabled_ui(self.wizard.is_none(), |ui| {
                    ui.selectable_value(&mut self.tab, Tab::Run, "▶ Play");
                    ui.selectable_value(&mut self.tab, Tab::Console, "🖥 Connection options");
                    ui.selectable_value(&mut self.tab, Tab::Settings, "⚙ Settings");
                    ui.selectable_value(&mut self.tab, Tab::Bindings, "🎮 Bindings");
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Quit").on_hover_text("Stop the session and exit").clicked() {
                        self.quitting = true;
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                    if ui.button("Hide to tray").clicked() {
                        hide_window(self.hwnd);
                    }
                    if ui.add_enabled(self.dirty, egui::Button::new("💾 Save")).clicked() {
                        self.save();
                    }
                    if self.dirty {
                        if ui.button("Revert").clicked() {
                            let path = self.shared.lock().unwrap().cfg_path.clone();
                            match Config::load(&path) {
                                Ok(c) => {
                                    self.rows = c.profiles.iter().map(rows_from_profile).collect();
                                    self.login_pin_buf = c.console.login_pin.clone().unwrap_or_default();
                                    self.cfg = c;
                                    self.dirty = false;
                                    self.message = "Reverted to the saved config".into();
                                }
                                Err(e) => self.message = format!("{e:#}"),
                            }
                        }
                        ui.colored_label(egui::Color32::from_rgb(0xea, 0xb3, 0x08), "unsaved changes");
                    }
                });
            });
            ui.add_space(2.0);
        });
        egui::TopBottomPanel::bottom("bottom").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(&self.message);
                if !self.tasks.is_empty() {
                    ui.spinner();
                }
            });
        });
        if self.wizard.is_some() {
            egui::CentralPanel::default().show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| self.ui_wizard(ui));
            });
            ctx.request_repaint_after(Duration::from_millis(250));
            return;
        }
        egui::CentralPanel::default().show(ctx, |ui| match self.tab {
            Tab::Run => {
                egui::ScrollArea::vertical().show(ui, |ui| self.ui_run(ui));
            }
            Tab::Console => {
                egui::ScrollArea::vertical().show(ui, |ui| self.ui_console(ui));
            }
            Tab::Settings => {
                egui::ScrollArea::vertical().show(ui, |ui| self.ui_settings(ui));
            }
            Tab::Bindings => self.ui_bindings(ui),
        });

        let (session_active, streaming) = {
            let sh = self.shared.lock().unwrap();
            (sh.runner.is_some() || !sh.stopping.is_empty(), sh.runner.as_ref().map(|r| r.is_streaming()).unwrap_or(false))
        };
        if streaming && self.tab == Tab::Run {
            ctx.request_repaint_after(Duration::from_millis(33)); // live controller preview
        } else if session_active && self.tab == Tab::Run {
            ctx.request_repaint_after(Duration::from_millis(100)); // capture state
        } else if !self.tasks.is_empty() || self.listen.is_some() || self.account.active || session_active {
            ctx.request_repaint_after(Duration::from_millis(250));
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.shared.lock().unwrap().stop_and_wait();
        unsafe {
            windows_sys::Win32::UI::WindowsAndMessaging::KillTimer(self.hwnd as _, TRAY_TIMER_ID);
        }
        // dropping the tray icon removes it from the taskbar
        TRAY.with(|t| t.borrow_mut().take());
    }
}

pub fn run(cfg_path: PathBuf) -> Result<()> {
    let start_hidden = Config::load(&cfg_path).map(|c| c.ui.start_in_tray).unwrap_or(false);
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("chiaki-controller")
            .with_inner_size([760.0, 620.0])
            .with_min_inner_size([560.0, 420.0])
            .with_visible(!start_hidden)
            .with_icon(egui::IconData { rgba: icon_rgba(64), width: 64, height: 64 }),
        ..Default::default()
    };
    eframe::run_native(
        "chiaki-controller",
        options,
        Box::new(move |cc| match GuiApp::new(cc, cfg_path) {
            Ok(app) => Ok(Box::new(app) as Box<dyn eframe::App>),
            Err(e) => Err(e.into()),
        }),
    )
    .map_err(|e| anyhow!("settings window: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_roundtrip_through_config() {
        let cfg = Config::default_config();
        for p in &cfg.profiles {
            let rows = rows_from_profile(p);
            let binds = binds_from_rows(&rows).unwrap();
            assert_eq!(format!("{:?}", binds), format!("{:?}", p.bind), "profile {}", p.name);
        }
    }

    #[test]
    fn rows_validation() {
        let row = |input: &str, action: &str| BindRow { input: input.into(), action: action.into(), ..Default::default() };
        assert!(binds_from_rows(&[row("W", "cross"), row("w", "circle")]).is_err(), "duplicate input");
        assert!(binds_from_rows(&[row("Nope", "cross")]).is_err(), "unknown input");
        let b = binds_from_rows(&[BindRow { value: Some(0.4), ..row("LCtrl", "left_stick_scale") }, row("", "")]).unwrap();
        assert!(matches!(b.get("LCtrl"), Some(Binding::Detailed(d)) if d.value == Some(0.4)));
        // default amount collapses to the short form
        let b = binds_from_rows(&[BindRow { value: Some(1.0), ..row("MouseLeft", "r2") }]).unwrap();
        assert!(matches!(b.get("MouseLeft"), Some(Binding::Simple(a)) if a == "r2"));
    }

    #[test]
    fn config_save_load_keeps_console() {
        let mut cfg = Config::default_config();
        cfg.console.host = "192.168.1.50".into();
        cfg.console.regist_key = "3563343133626237".into();
        cfg.console.rp_key = "00112233445566778899aabbccddeeff".into();
        cfg.console.login_pin = Some("1234".into());
        cfg.ui.start_in_tray = true;
        let dir = std::env::temp_dir().join(format!("chiaki-controller-test-{}", std::process::id()));
        let path = dir.join("cfg.toml");
        cfg.save(&path).unwrap();
        let back = Config::load(&path).unwrap();
        std::fs::remove_dir_all(&dir).ok();
        assert_eq!(back.console.host, cfg.console.host);
        assert_eq!(back.console.regist_key, cfg.console.regist_key);
        assert_eq!(back.console.rp_key, cfg.console.rp_key);
        assert_eq!(back.console.login_pin, cfg.console.login_pin);
        assert!(back.ui.start_in_tray);
        assert_eq!(back.profiles.len(), cfg.profiles.len());
        Mapper::new(&back).unwrap();
        assert!(back.credentials().is_ok());
    }
}
