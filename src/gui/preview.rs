//! A small live DualSense drawing: shows what the PS5 currently receives.

use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, Rounding, Sense, Shape, Stroke, Vec2};

use crate::controller::{button, ControllerState, TOUCHPAD_HEIGHT, TOUCHPAD_WIDTH};

pub fn controller(ui: &mut egui::Ui, st: &ControllerState) {
    let width = ui.available_width().min(420.0);
    let (resp, painter) = ui.allocate_painter(Vec2::new(width, width * 0.58), Sense::hover());
    let r = resp.rect;
    let s = r.width() / 420.0; // design units: 420 x 244
    let p = |x: f32, y: f32| Pos2::new(r.left() + x * s, r.top() + y * s);

    let dark = ui.visuals().dark_mode;
    let body = if dark { Color32::from_gray(52) } else { Color32::from_gray(232) };
    let outline = if dark { Color32::from_gray(90) } else { Color32::from_gray(170) };
    let idle = if dark { Color32::from_gray(80) } else { Color32::from_gray(200) };
    let text = if dark { Color32::from_gray(210) } else { Color32::from_gray(60) };
    let on = Color32::from_rgb(0x25, 0x63, 0xeb);
    let stroke = Stroke::new(1.5 * s, outline);
    let pressed = |b: u32| st.buttons & b != 0;
    let fill = |b: bool| if b { on } else { idle };

    // triggers (L2/R2 pressure bars) and bumpers
    for (x, v, label) in [(70.0, st.l2, "L2"), (290.0, st.r2, "R2")] {
        let bar = Rect::from_min_size(p(x, 2.0), Vec2::new(60.0 * s, 14.0 * s));
        painter.rect(bar, Rounding::same(4.0 * s), idle, stroke);
        let mut lvl = bar;
        lvl.set_width(bar.width() * v as f32 / 255.0);
        if v > 0 {
            painter.rect_filled(lvl, Rounding::same(4.0 * s), on);
        }
        painter.text(bar.center(), Align2::CENTER_CENTER, label, FontId::proportional(10.0 * s), text);
    }
    for (x, b, label) in [(70.0, button::L1, "L1"), (290.0, button::R1, "R1")] {
        let bump = Rect::from_min_size(p(x, 20.0), Vec2::new(60.0 * s, 12.0 * s));
        painter.rect(bump, Rounding::same(4.0 * s), fill(pressed(b)), stroke);
        painter.text(bump.center(), Align2::CENTER_CENTER, label, FontId::proportional(10.0 * s), text);
    }

    // body with grips (the grips are drawn first so the body outline covers their top)
    for gx in [88.0, 332.0] {
        painter.circle(p(gx, 168.0), 44.0 * s, body, stroke);
    }
    painter.rect(Rect::from_min_max(p(40.0, 38.0), p(380.0, 172.0)), Rounding::same(40.0 * s), body, stroke);
    // hide the grip outlines inside the body
    painter.rect_filled(Rect::from_min_max(p(46.0, 120.0), p(374.0, 170.0)), Rounding::same(30.0 * s), body);

    // touchpad with touch point
    let pad = Rect::from_min_max(p(140.0, 44.0), p(280.0, 108.0));
    painter.rect(pad, Rounding::same(8.0 * s), fill(pressed(button::TOUCHPAD)), stroke);
    for t in st.touches.iter().filter(|t| t.id >= 0) {
        let tx = pad.left() + pad.width() * t.x as f32 / TOUCHPAD_WIDTH as f32;
        let ty = pad.top() + pad.height() * t.y as f32 / TOUCHPAD_HEIGHT as f32;
        painter.circle_filled(Pos2::new(tx, ty), 5.0 * s, Color32::WHITE);
    }

    // create / options / PS
    painter.rect(Rect::from_center_size(p(125.0, 56.0), Vec2::new(10.0 * s, 18.0 * s)), Rounding::same(4.0 * s), fill(pressed(button::SHARE)), stroke);
    painter.rect(Rect::from_center_size(p(295.0, 56.0), Vec2::new(10.0 * s, 18.0 * s)), Rounding::same(4.0 * s), fill(pressed(button::OPTIONS)), stroke);
    painter.circle(p(210.0, 132.0), 9.0 * s, fill(pressed(button::PS)), stroke);
    painter.text(p(210.0, 132.0), Align2::CENTER_CENTER, "PS", FontId::proportional(7.5 * s), text);

    // d-pad
    let dpad = [
        (button::DPAD_UP, 0.0, -1.0),
        (button::DPAD_DOWN, 0.0, 1.0),
        (button::DPAD_LEFT, -1.0, 0.0),
        (button::DPAD_RIGHT, 1.0, 0.0),
    ];
    for (b, dx, dy) in dpad {
        let c = p(90.0 + dx * 17.0, 82.0 + dy * 17.0);
        painter.rect(Rect::from_center_size(c, Vec2::splat(15.0 * s)), Rounding::same(3.0 * s), fill(pressed(b)), stroke);
    }

    // face buttons
    // symbols drawn as shapes: the default font has no triangle / cross / square glyphs
    let face = [(button::TRIANGLE, 0.0, -1.0), (button::CROSS, 0.0, 1.0), (button::SQUARE, -1.0, 0.0), (button::CIRCLE, 1.0, 0.0)];
    for (b, dx, dy) in face {
        let c = p(330.0 + dx * 20.0, 82.0 + dy * 20.0);
        painter.circle(c, 9.5 * s, fill(pressed(b)), stroke);
        let sym = Stroke::new(1.6 * s, if pressed(b) { Color32::WHITE } else { text });
        let k = 4.5 * s;
        match b {
            button::TRIANGLE => {
                let pts = vec![c + Vec2::new(0.0, -k), c + Vec2::new(k, k * 0.8), c + Vec2::new(-k, k * 0.8)];
                painter.add(Shape::closed_line(pts, sym));
            }
            button::CROSS => {
                painter.line_segment([c + Vec2::new(-k, -k), c + Vec2::new(k, k)], sym);
                painter.line_segment([c + Vec2::new(-k, k), c + Vec2::new(k, -k)], sym);
            }
            button::SQUARE => {
                painter.rect_stroke(Rect::from_center_size(c, Vec2::splat(2.0 * k)), Rounding::ZERO, sym);
            }
            _ => {
                painter.circle_stroke(c, k, sym);
            }
        }
    }

    // sticks: ring, deflection dot, L3/R3 fill
    for (cx, x, y, b) in [(150.0, st.left_x, st.left_y, button::L3), (270.0, st.right_x, st.right_y, button::R3)] {
        let c = p(cx, 150.0);
        let rad = 22.0 * s;
        painter.circle(c, rad, if pressed(b) { on.gamma_multiply(0.35) } else { idle }, stroke);
        let off = Vec2::new(x as f32 / 32767.0, y as f32 / 32767.0) * (rad - 7.0 * s);
        let moved = x != 0 || y != 0;
        painter.circle_filled(c + off, 7.0 * s, if moved || pressed(b) { on } else { outline });
    }

    let gyro = st.gyro;
    if gyro.iter().any(|g| g.abs() > 0.001) {
        painter.text(p(210.0, 236.0), Align2::CENTER_CENTER, format!("gyro {:+.1} {:+.1}", gyro[0], gyro[1]), FontId::proportional(10.0 * s), text);
    }
}

/// Normalizes action names from the config (aliases, case).
fn canonical_action(a: &str) -> String {
    let a = a.trim().to_ascii_lowercase().replace('-', "_");
    match a.as_str() {
        "x" => "cross".into(),
        "moon" => "circle".into(),
        "box" => "square".into(),
        "pyramid" => "triangle".into(),
        "start" => "options".into(),
        "share" => "create".into(),
        "home" => "ps".into(),
        "touchpad_click" => "touchpad".into(),
        _ => a,
    }
}

const BUTTON_ROWS: &[(&str, &str, u32)] = &[
    ("cross", "Cross", button::CROSS),
    ("circle", "Circle", button::CIRCLE),
    ("square", "Square", button::SQUARE),
    ("triangle", "Triangle", button::TRIANGLE),
    ("l1", "L1", button::L1),
    ("r1", "R1", button::R1),
    ("l2", "L2", 0),
    ("r2", "R2", 0),
    ("l3", "L3 (press left stick)", button::L3),
    ("r3", "R3 (press right stick)", button::R3),
    ("dpad_up", "D-pad up", button::DPAD_UP),
    ("dpad_down", "D-pad down", button::DPAD_DOWN),
    ("dpad_left", "D-pad left", button::DPAD_LEFT),
    ("dpad_right", "D-pad right", button::DPAD_RIGHT),
    ("touchpad", "Touchpad click", button::TOUCHPAD),
    ("options", "Options", button::OPTIONS),
    ("create", "Create", button::SHARE),
    ("ps", "PS button", button::PS),
    ("touch", "Touch the touchpad", 0),
    ("touch_left", "Touch left side", 0),
    ("touch_right", "Touch right side", 0),
];

/// The active profile's switch keys and bindings; rows light up while their control is in use.
pub fn bindings_panel(ui: &mut egui::Ui, cfg: &crate::config::Config, active: &str, st: &ControllerState) {
    use crate::config::{Binding, MouseTarget};
    let on = Color32::from_rgb(0x25, 0x63, 0xeb);

    ui.weak("Profile");
    ui.label(egui::RichText::new(active).size(20.0).strong());
    let Some(profile) = cfg.profiles.iter().find(|p| p.name.eq_ignore_ascii_case(active)) else {
        ui.weak("(profile not found in the config)");
        return;
    };

    // action -> keys, and the profile switch keys
    let mut keys_for: std::collections::BTreeMap<String, Vec<String>> = Default::default();
    let mut switches: Vec<(String, String)> = Vec::new();
    for (key, b) in &profile.bind {
        let (action, target) = match b {
            Binding::Simple(a) => (canonical_action(a), None),
            Binding::Detailed(d) => (canonical_action(&d.action), d.profile.clone()),
        };
        if action == "profile_next" {
            switches.push((key.clone(), "next profile".into()));
        } else if let Some(t) = action.strip_prefix("profile:") {
            switches.push((key.clone(), t.to_string()));
        } else if action == "profile" {
            switches.push((key.clone(), target.unwrap_or_default()));
        } else {
            keys_for.entry(action).or_default().push(key.clone());
        }
    }

    ui.add_space(4.0);
    ui.strong("Switch profile");
    if switches.is_empty() {
        ui.weak("No switch key in this profile (add one in Bindings)");
    } else {
        for (key, target) in &switches {
            ui.horizontal(|ui| {
                ui.monospace(key);
                ui.label(format!("›  {target}"));
            });
        }
    }
    let others: Vec<&str> = cfg.profiles.iter().map(|p| p.name.as_str()).filter(|n| !n.eq_ignore_ascii_case(active)).collect();
    if !others.is_empty() {
        ui.weak(format!("Other profiles: {}", others.join(", ")));
    }

    ui.add_space(6.0);
    ui.strong("Bindings");
    egui::ScrollArea::vertical().id_salt("bindings_panel").max_height(260.0).auto_shrink([false, true]).show(ui, |ui| {
        egui::Grid::new("bindings_panel_grid").num_columns(2).spacing([10.0, 2.0]).striped(true).show(ui, |ui| {
            let row = |ui: &mut egui::Ui, label: &str, keys: String, active: bool| {
                let l = egui::RichText::new(label);
                let k = egui::RichText::new(keys).monospace();
                if active {
                    ui.label(l.color(on).strong());
                    ui.label(k.color(on).strong());
                } else {
                    ui.label(l);
                    ui.label(k);
                }
                ui.end_row();
            };

            // sticks: one row each, directions with their keys, plus the mouse
            for (stick, name, x, y) in [("left", "Left stick", st.left_x, st.left_y), ("right", "Right stick", st.right_x, st.right_y)] {
                let dirs: Vec<Option<String>> = ["up", "left", "down", "right"]
                    .iter()
                    .map(|d| keys_for.get(&format!("{stick}_stick_{d}")).map(|k| k.join("/")))
                    .collect();
                let mut parts = Vec::new();
                if dirs.iter().all(|d| d.is_some()) {
                    // all four bound: the familiar "W A S D" order (up, left, down, right)
                    parts.push(dirs.iter().flatten().cloned().collect::<Vec<_>>().join(" "));
                } else {
                    for (d, label) in dirs.iter().zip(["up", "left", "down", "right"]) {
                        if let Some(k) = d {
                            parts.push(format!("{label} {k}"));
                        }
                    }
                }
                let mouse = (stick == "left" && profile.mouse.target == MouseTarget::LeftStick)
                    || (stick == "right" && profile.mouse.target == MouseTarget::RightStick);
                if mouse {
                    parts.push("Mouse".into());
                }
                if !parts.is_empty() {
                    row(ui, name, parts.join(" · "), x != 0 || y != 0);
                }
                if let Some(k) = keys_for.get(&format!("{stick}_stick_scale")) {
                    row(ui, &format!("{name} slower (hold)"), k.join(", "), false);
                }
            }
            if profile.mouse.target == MouseTarget::Gyro {
                row(ui, "Gyro (motion)", "Mouse".into(), st.gyro.iter().any(|g| g.abs() > 0.001));
            }

            for (action, label, bit) in BUTTON_ROWS {
                let Some(k) = keys_for.get(*action) else { continue };
                let active = match *action {
                    "l2" => st.l2 > 0,
                    "r2" => st.r2 > 0,
                    "touch" | "touch_left" | "touch_right" => st.touches.iter().any(|t| t.id >= 0),
                    _ => st.buttons & bit != 0,
                };
                row(ui, label, k.join(", "), active);
            }
        });
    });
}
