//! Windows front end: a small window that grabs keyboard and mouse through Raw Input,
//! confines and hides the cursor while capturing, and shows the connection status.

use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{bail, Result};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    BeginPaint, ClientToScreen, DrawTextW, EndPaint, FillRect, GetSysColorBrush, InvalidateRect, SetBkMode, SetTextColor,
    COLOR_WINDOW, DT_LEFT, DT_NOPREFIX, DT_WORDBREAK, PAINTSTRUCT, TRANSPARENT,
};
use windows_sys::Win32::Media::{timeBeginPeriod, timeEndPeriod};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{VK_CONTROL, VK_LWIN, VK_MENU, VK_RETURN, VK_RWIN, VK_SHIFT};
use windows_sys::Win32::UI::Input::{
    GetRawInputData, RegisterRawInputDevices, HRAWINPUT, RAWINPUT, RAWINPUTDEVICE, RAWINPUTHEADER, RID_INPUT, RIM_TYPEKEYBOARD,
    RIM_TYPEMOUSE,
};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use super::keys::{self, InputId};
use super::mapper::{Event, Mapper};
use crate::stream::ControllerLink;

pub const WM_APP_STATUS: u32 = WM_APP + 1;
const MOUSE_MOVE_ABSOLUTE: u32 = 1;

static CAPTURING: AtomicBool = AtomicBool::new(false);

/// Lets other threads repaint the status and close the window.
#[derive(Clone, Copy)]
pub struct WindowHandle(usize);

impl WindowHandle {
    pub fn refresh(&self) {
        unsafe {
            PostMessageW(self.0 as HWND, WM_APP_STATUS, 0, 0);
        }
    }
    pub fn close(&self) {
        unsafe {
            PostMessageW(self.0 as HWND, WM_CLOSE, 0, 0);
        }
    }
}

/// (Re)claims Raw Input for this window. Registration is per process, so another window
/// (e.g. the settings UI) may have taken it in the meantime.
fn register_raw_input(hwnd: HWND) -> bool {
    let devices = [
        RAWINPUTDEVICE { usUsagePage: 0x01, usUsage: 0x02, dwFlags: 0, hwndTarget: hwnd }, // mouse
        RAWINPUTDEVICE { usUsagePage: 0x01, usUsage: 0x06, dwFlags: 0, hwndTarget: hwnd }, // keyboard
    ];
    unsafe { RegisterRawInputDevices(devices.as_ptr(), devices.len() as u32, std::mem::size_of::<RAWINPUTDEVICE>() as u32) != 0 }
}

struct App {
    hwnd: HWND,
    mapper: Mapper,
    link: Arc<ControllerLink>,
    status: Arc<Mutex<String>>,
    capturing: bool,
    capture_key: InputId,
    quit_key: InputId,
    last_abs: Option<(i32, i32)>,
    last_input: String,
    dirty: bool,
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

impl App {
    fn set_capture(&mut self, on: bool) {
        if on == self.capturing {
            return;
        }
        self.capturing = on;
        CAPTURING.store(on, Ordering::Relaxed);
        unsafe {
            if on {
                register_raw_input(self.hwnd);
                let mut rc: RECT = std::mem::zeroed();
                GetClientRect(self.hwnd, &mut rc);
                let mut tl = POINT { x: rc.left, y: rc.top };
                let mut br = POINT { x: rc.right, y: rc.bottom };
                ClientToScreen(self.hwnd, &mut tl);
                ClientToScreen(self.hwnd, &mut br);
                let clip = RECT { left: tl.x, top: tl.y, right: br.x, bottom: br.y };
                ClipCursor(&clip);
                while ShowCursor(0) >= 0 {}
            } else {
                ClipCursor(std::ptr::null());
                while ShowCursor(1) < 0 {}
            }
        }
        self.mapper.release_all();
        self.last_abs = None;
        self.dirty = true;
        log::info!("input capture {}", if on { "on" } else { "off" });
        self.repaint();
    }

    fn repaint(&self) {
        unsafe {
            InvalidateRect(self.hwnd, std::ptr::null(), 1);
        }
    }

    fn key_event(&mut self, id: InputId, down: bool) {
        if down && id == self.quit_key {
            unsafe { PostMessageW(self.hwnd, WM_CLOSE, 0, 0) };
            return;
        }
        if id == self.capture_key {
            if down {
                let c = !self.capturing;
                self.set_capture(c);
            }
            return;
        }
        if !self.capturing {
            return;
        }
        if down {
            self.last_input = keys::name(id);
            self.repaint();
        }
        let prof = self.mapper.profile_name().to_string();
        self.mapper.handle(if down { Event::Down(id) } else { Event::Up(id) }, Instant::now());
        if self.mapper.profile_name() != prof {
            self.repaint();
        }
        self.dirty = true;
    }

    fn raw_input(&mut self, lparam: LPARAM) {
        let mut buf = [0u64; 16]; // 128 bytes, 8-aligned: enough for mouse and keyboard
        let mut size = std::mem::size_of_val(&buf) as u32;
        let got = unsafe {
            GetRawInputData(
                lparam as HRAWINPUT,
                RID_INPUT,
                buf.as_mut_ptr() as *mut _,
                &mut size,
                std::mem::size_of::<RAWINPUTHEADER>() as u32,
            )
        };
        if got == u32::MAX || got == 0 {
            return;
        }
        let raw = unsafe { &*(buf.as_ptr() as *const RAWINPUT) };
        match raw.header.dwType {
            RIM_TYPEKEYBOARD => {
                let kb = unsafe { raw.data.keyboard };
                let down = kb.Flags & RI_KEY_BREAK as u16 == 0;
                let e0 = kb.Flags & RI_KEY_E0 as u16 != 0;
                let vk = kb.VKey;
                if vk == 0xff {
                    return; // fake key from an escape sequence
                }
                let id: InputId = match vk {
                    VK_SHIFT => {
                        if kb.MakeCode == 0x36 {
                            keys::VK_RSHIFT
                        } else {
                            keys::VK_LSHIFT
                        }
                    }
                    VK_CONTROL => {
                        if e0 {
                            keys::VK_RCONTROL
                        } else {
                            keys::VK_LCONTROL
                        }
                    }
                    VK_MENU => {
                        if e0 {
                            keys::VK_RMENU
                        } else {
                            keys::VK_LMENU
                        }
                    }
                    VK_RETURN if e0 => keys::NUMPAD_ENTER,
                    v => v,
                };
                self.key_event(id, down);
            }
            RIM_TYPEMOUSE => {
                let m = unsafe { raw.data.mouse };
                let (flags, data) = unsafe { (m.Anonymous.Anonymous.usButtonFlags as u32, m.Anonymous.Anonymous.usButtonData as i16) };
                if !self.capturing {
                    return;
                }
                let now = Instant::now();
                let (dx, dy) = if m.usFlags as u32 & MOUSE_MOVE_ABSOLUTE != 0 {
                    // remote desktop / tablets report absolute positions
                    let d = self.last_abs.map(|(x, y)| (m.lLastX - x, m.lLastY - y)).unwrap_or((0, 0));
                    self.last_abs = Some((m.lLastX, m.lLastY));
                    (d.0 / 32, d.1 / 32)
                } else {
                    (m.lLastX, m.lLastY)
                };
                if dx != 0 || dy != 0 {
                    self.mapper.handle(Event::MouseMove(dx, dy), now);
                    self.dirty = true;
                }
                const BUTTONS: [(u32, u32, InputId); 5] = [
                    (RI_MOUSE_LEFT_BUTTON_DOWN, RI_MOUSE_LEFT_BUTTON_UP, keys::MOUSE_LEFT),
                    (RI_MOUSE_RIGHT_BUTTON_DOWN, RI_MOUSE_RIGHT_BUTTON_UP, keys::MOUSE_RIGHT),
                    (RI_MOUSE_MIDDLE_BUTTON_DOWN, RI_MOUSE_MIDDLE_BUTTON_UP, keys::MOUSE_MIDDLE),
                    (RI_MOUSE_BUTTON_4_DOWN, RI_MOUSE_BUTTON_4_UP, keys::MOUSE_4),
                    (RI_MOUSE_BUTTON_5_DOWN, RI_MOUSE_BUTTON_5_UP, keys::MOUSE_5),
                ];
                for (down_flag, up_flag, id) in BUTTONS {
                    if flags & down_flag != 0 {
                        self.key_event(id, true);
                    }
                    if flags & up_flag != 0 {
                        self.key_event(id, false);
                    }
                }
                if flags & RI_MOUSE_WHEEL != 0 && data != 0 {
                    let id = if data > 0 { keys::WHEEL_UP } else { keys::WHEEL_DOWN };
                    self.wheel(id, now);
                }
                if flags & RI_MOUSE_HWHEEL != 0 && data != 0 {
                    let id = if data > 0 { keys::WHEEL_RIGHT } else { keys::WHEEL_LEFT };
                    self.wheel(id, now);
                }
            }
            _ => {}
        }
    }

    fn wheel(&mut self, id: InputId, now: Instant) {
        if id == self.capture_key || id == self.quit_key {
            self.key_event(id, true);
            return;
        }
        self.last_input = keys::name(id);
        self.mapper.handle(Event::Wheel(id), now);
        self.dirty = true;
        self.repaint();
    }

    fn paint(&self) {
        unsafe {
            let mut ps: PAINTSTRUCT = std::mem::zeroed();
            let hdc = BeginPaint(self.hwnd, &mut ps);
            let mut rc: RECT = std::mem::zeroed();
            GetClientRect(self.hwnd, &mut rc);
            FillRect(hdc, &rc, GetSysColorBrush(COLOR_WINDOW));
            SetBkMode(hdc, TRANSPARENT as i32);
            SetTextColor(hdc, 0x00202020);
            let status = self.status.lock().map(|s| s.clone()).unwrap_or_default();
            let text = format!(
                "{}\n\nInput: {}   Profile: {}\n{} toggles capture, {} quits.{}\n{}",
                status,
                if self.capturing { "CAPTURED" } else { "released (click here to capture)" },
                self.mapper.profile_name(),
                keys::name(self.capture_key),
                keys::name(self.quit_key),
                if self.link.is_streaming() { "" } else { "  (not streaming yet)" },
                if self.last_input.is_empty() { String::new() } else { format!("Last input: {}", self.last_input) },
            );
            let mut w: Vec<u16> = text.encode_utf16().collect();
            rc.left += 12;
            rc.top += 10;
            rc.right -= 12;
            DrawTextW(hdc, w.as_mut_ptr(), w.len() as i32, &mut rc, DT_LEFT | DT_WORDBREAK | DT_NOPREFIX);
            EndPaint(self.hwnd, &ps);
        }
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // must not depend on the App borrow: DestroyWindow sends it synchronously
    if msg == WM_DESTROY {
        PostQuitMessage(0);
        return 0;
    }
    let mut destroy = false;
    let handled = APP.with(|cell| {
        let Ok(mut guard) = cell.try_borrow_mut() else { return None };
        let app = guard.as_mut()?;
        match msg {
            WM_INPUT => {
                app.raw_input(lparam);
                None // DefWindowProc must still clean up
            }
            WM_PAINT => {
                app.paint();
                Some(0)
            }
            WM_APP_STATUS => {
                app.repaint();
                Some(0)
            }
            WM_LBUTTONDOWN if !app.capturing => {
                app.set_capture(true);
                Some(0)
            }
            WM_ACTIVATE => {
                if (wparam & 0xffff) as u32 == WA_INACTIVE {
                    app.set_capture(false);
                } else {
                    register_raw_input(hwnd);
                }
                None
            }
            WM_KILLFOCUS => {
                app.set_capture(false);
                None
            }
            WM_SIZE | WM_MOVE => {
                if app.capturing {
                    app.set_capture(false);
                    app.set_capture(true);
                }
                app.repaint();
                None
            }
            // keep Alt / F10 from opening the system menu while playing
            WM_SYSKEYDOWN | WM_SYSKEYUP | WM_SYSCHAR if app.capturing => Some(0),
            WM_SYSCOMMAND if app.capturing && (wparam & 0xfff0) as u32 == SC_KEYMENU => Some(0),
            WM_CLOSE => {
                app.set_capture(false);
                destroy = true; // outside the borrow, see WM_DESTROY above
                Some(0)
            }
            _ => None,
        }
    });
    if destroy {
        DestroyWindow(hwnd);
    }
    match handled {
        Some(r) => r,
        None => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

/// Swallows the Windows keys while capturing so they don't open the Start menu.
unsafe extern "system" fn keyboard_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 && CAPTURING.load(Ordering::Relaxed) {
        let kb = &*(lparam as *const KBDLLHOOKSTRUCT);
        if kb.vkCode == VK_LWIN as u32 || kb.vkCode == VK_RWIN as u32 {
            return 1;
        }
    }
    CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam)
}

pub struct UiOptions {
    pub capture_key: InputId,
    pub quit_key: InputId,
    pub capture_on_start: bool,
    pub tick: Duration,
}

/// Creates the window and runs the message loop until it is closed.
/// `on_created` receives a handle other threads can use to refresh or close the window.
pub fn run(
    mapper: Mapper,
    link: Arc<ControllerLink>,
    status: Arc<Mutex<String>>,
    opts: UiOptions,
    on_created: impl FnOnce(WindowHandle),
) -> Result<()> {
    unsafe {
        let hinstance = GetModuleHandleW(std::ptr::null());
        let class_name = wide("ChiakiControllerWindow");
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(wndproc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: hinstance,
            hIcon: std::ptr::null_mut(),
            hCursor: LoadCursorW(std::ptr::null_mut(), IDC_ARROW),
            hbrBackground: std::ptr::null_mut(),
            lpszMenuName: std::ptr::null(),
            lpszClassName: class_name.as_ptr(),
            hIconSm: std::ptr::null_mut(),
        };
        // a second session in the same process finds the class already registered
        if RegisterClassExW(&wc) == 0 && windows_sys::Win32::Foundation::GetLastError() != 1410 {
            bail!("RegisterClassExW failed: {}", std::io::Error::last_os_error());
        }
        let title = wide("chiaki-controller");
        let hwnd = CreateWindowExW(
            0,
            class_name.as_ptr(),
            title.as_ptr(),
            WS_OVERLAPPEDWINDOW | WS_VISIBLE,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            560,
            260,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            hinstance,
            std::ptr::null(),
        );
        if hwnd.is_null() {
            bail!("CreateWindowExW failed: {}", std::io::Error::last_os_error());
        }

        if !register_raw_input(hwnd) {
            bail!("RegisterRawInputDevices failed: {}", std::io::Error::last_os_error());
        }
        let hook = SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_hook), hinstance, 0);

        APP.with(|cell| {
            *cell.borrow_mut() = Some(App {
                hwnd,
                mapper,
                link: link.clone(),
                status,
                capturing: false,
                capture_key: opts.capture_key,
                quit_key: opts.quit_key,
                last_abs: None,
                last_input: String::new(),
                dirty: true,
            });
        });
        ShowWindow(hwnd, SW_SHOW);
        SetForegroundWindow(hwnd);
        if opts.capture_on_start {
            APP.with(|cell| {
                if let Some(app) = cell.borrow_mut().as_mut() {
                    app.set_capture(true);
                }
            });
        }
        on_created(WindowHandle(hwnd as usize));

        timeBeginPeriod(1);
        let tick_ms = opts.tick.as_millis().max(1) as u32;
        let mut msg: MSG = std::mem::zeroed();
        'outer: loop {
            let needs_ticks = APP.with(|c| c.borrow().as_ref().map(|a| a.mapper.needs_ticks()).unwrap_or(false));
            let timeout = if needs_ticks { tick_ms } else { 100 };
            MsgWaitForMultipleObjectsEx(0, std::ptr::null(), timeout, QS_ALLINPUT, MWMO_INPUTAVAILABLE);
            while PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                if msg.message == WM_QUIT {
                    break 'outer;
                }
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            APP.with(|c| {
                if let Some(app) = c.borrow_mut().as_mut() {
                    if app.dirty || app.mapper.needs_ticks() {
                        app.dirty = false;
                        let st = app.mapper.state(Instant::now());
                        app.link.set_state(&st);
                    }
                }
            });
        }
        timeEndPeriod(1);
        if !hook.is_null() {
            UnhookWindowsHookEx(hook);
        }
        APP.with(|c| *c.borrow_mut() = None);
        link.set_state(&crate::controller::ControllerState::default());
    }
    Ok(())
}
