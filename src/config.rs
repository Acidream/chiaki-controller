//! Configuration file (TOML): console credentials, stream settings and input profiles.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::http;

/// What registration produced and every session needs.
#[derive(Clone)]
pub struct Credentials {
    /// Parsed `PS5-RegistKey` bytes, zero padded.
    pub regist_key: [u8; 16],
    /// `RP-Key`.
    pub morning: [u8; 16],
}

impl Credentials {
    pub fn regist_key_trimmed(&self) -> &[u8] {
        let len = self.regist_key.iter().position(|&b| b == 0).unwrap_or(16);
        &self.regist_key[..len]
    }
}

#[derive(Deserialize, Serialize, Default, Clone)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub console: ConsoleConfig,
    #[serde(default)]
    pub stream: StreamConfig,
    #[serde(default)]
    pub input: InputConfig,
    #[serde(default)]
    pub ui: UiConfig,
    #[serde(default, rename = "profile")]
    pub profiles: Vec<Profile>,
}

#[derive(Deserialize, Serialize, Default, Clone)]
#[serde(deny_unknown_fields)]
pub struct ConsoleConfig {
    /// IP address of the PS5.
    #[serde(default)]
    pub host: String,
    /// `PS5-RegistKey` from registration, as hex.
    #[serde(default)]
    pub regist_key: String,
    /// `RP-Key` from registration, as hex (32 characters).
    #[serde(default)]
    pub rp_key: String,
    /// PSN login passcode, only if the console asks for one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub login_pin: Option<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub nickname: String,
    /// Send a wakeup packet and wait for the console when it is in rest mode.
    #[serde(default = "yes")]
    pub wake: bool,
}

#[derive(Deserialize, Serialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct StreamConfig {
    /// Requested video resolution. Video is never decoded; the lowest setting keeps the
    /// network quiet. One of 360p, 540p, 720p, 1080p.
    #[serde(default = "default_resolution")]
    pub resolution: String,
    #[serde(default = "default_fps")]
    pub fps: u32,
    /// Overrides the bitrate of the resolution preset (kbps).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bitrate_kbps: Option<u32>,
    /// Present as a DualSense (true) or DualShock 4 (false).
    #[serde(default = "yes")]
    pub dualsense: bool,
    /// Reconnect automatically when the stream drops.
    #[serde(default = "yes")]
    pub reconnect: bool,
}

impl Default for StreamConfig {
    fn default() -> Self {
        Self { resolution: default_resolution(), fps: default_fps(), bitrate_kbps: None, dualsense: true, reconnect: true }
    }
}

#[derive(Deserialize, Serialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct InputConfig {
    /// Key that grabs / releases mouse and keyboard.
    #[serde(default = "default_capture_toggle")]
    pub capture_toggle: String,
    /// Key that quits the program.
    #[serde(default = "default_quit")]
    pub quit: String,
    /// Profile active at start (defaults to the first one).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_profile: Option<String>,
    /// Grab input as soon as the program starts.
    #[serde(default = "yes")]
    pub capture_on_start: bool,
    /// How long a mouse wheel notch holds its bound action (ms).
    #[serde(default = "default_wheel_ms")]
    pub wheel_press_ms: u64,
    /// Internal update rate for mouse-to-stick conversion (Hz).
    #[serde(default = "default_tick_hz")]
    pub tick_hz: u32,
}

impl Default for InputConfig {
    fn default() -> Self {
        Self {
            capture_toggle: default_capture_toggle(),
            quit: default_quit(),
            start_profile: None,
            capture_on_start: true,
            wheel_press_ms: default_wheel_ms(),
            tick_hz: default_tick_hz(),
        }
    }
}

/// Settings of the tray / settings window front end.
#[derive(Deserialize, Serialize, Clone, Default)]
#[serde(deny_unknown_fields)]
pub struct UiConfig {
    /// Start with only the tray icon, no settings window.
    #[serde(default)]
    pub start_in_tray: bool,
    /// Start a session as soon as the program starts.
    #[serde(default)]
    pub connect_on_launch: bool,
}

#[derive(Deserialize, Serialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    pub name: String,
    #[serde(default)]
    pub mouse: MouseConfig,
    /// Input name -> action. See the README for the names.
    #[serde(default)]
    pub bind: BTreeMap<String, Binding>,
    /// Scale stick diagonals from keys onto the unit circle.
    #[serde(default = "yes")]
    pub normalize_diagonals: bool,
}

#[derive(Deserialize, Serialize, Clone, Debug)]
#[serde(untagged)]
pub enum Binding {
    Simple(String),
    Detailed(BindingDetail),
}

#[derive(Deserialize, Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct BindingDetail {
    pub action: String,
    /// Stick deflection / trigger pressure / stick scale factor (0.0 - 1.0).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<f32>,
    /// Press once to hold, press again to release.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub toggle: bool,
    /// Repeat the action while held (presses per second).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turbo_hz: Option<f32>,
    /// Touchpad position for the "touch" action (0 - 1919, 0 - 941).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub x: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub y: Option<u16>,
    /// Profile name for the "profile" action.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
}

#[derive(Deserialize, Serialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "snake_case")]
pub enum MouseTarget {
    None,
    LeftStick,
    RightStick,
    /// Experimental: mouse motion as gyroscope rotation (for games with gyro aiming).
    Gyro,
}

#[derive(Deserialize, Serialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct MouseConfig {
    #[serde(default = "default_mouse_target")]
    pub target: MouseTarget,
    /// Stick deflection per mouse count per millisecond.
    #[serde(default = "default_sensitivity")]
    pub sensitivity: f32,
    /// Extra multipliers per axis.
    #[serde(default = "one")]
    pub sensitivity_x: f32,
    #[serde(default = "one")]
    pub sensitivity_y: f32,
    #[serde(default)]
    pub invert_y: bool,
    /// Output deflection where movement starts, to jump over the game's own dead zone (0.0 - 0.9).
    #[serde(default = "default_anti_deadzone")]
    pub anti_deadzone: f32,
    /// Response curve exponent; 1.0 = linear, above 1 = finer control of small movements.
    #[serde(default = "one")]
    pub curve: f32,
    /// Maximum deflection (0.0 - 1.0).
    #[serde(default = "one")]
    pub max: f32,
    /// Movement is averaged over this window (ms). Raise it if the stick jitters with
    /// a low polling rate mouse; lower it for less latency.
    #[serde(default = "default_smoothing")]
    pub smoothing_ms: f32,
    /// Gyro target: rotation speed (rad/s) per mouse count per millisecond.
    #[serde(default = "default_gyro_sensitivity")]
    pub gyro_sensitivity: f32,
}

impl Default for MouseConfig {
    fn default() -> Self {
        Self {
            target: default_mouse_target(),
            sensitivity: default_sensitivity(),
            sensitivity_x: 1.0,
            sensitivity_y: 1.0,
            invert_y: false,
            anti_deadzone: default_anti_deadzone(),
            curve: 1.0,
            max: 1.0,
            smoothing_ms: default_smoothing(),
            gyro_sensitivity: default_gyro_sensitivity(),
        }
    }
}

fn yes() -> bool {
    true
}
fn one() -> f32 {
    1.0
}
fn default_resolution() -> String {
    "360p".into()
}
fn default_fps() -> u32 {
    30
}
fn default_capture_toggle() -> String {
    "F8".into()
}
fn default_quit() -> String {
    "F12".into()
}
fn default_wheel_ms() -> u64 {
    50
}
fn default_tick_hz() -> u32 {
    500
}
fn default_mouse_target() -> MouseTarget {
    MouseTarget::RightStick
}
fn default_sensitivity() -> f32 {
    0.25
}
fn default_anti_deadzone() -> f32 {
    0.2
}
fn default_smoothing() -> f32 {
    10.0
}
fn default_gyro_sensitivity() -> f32 {
    1.0
}

impl Config {
    pub fn load(path: &Path) -> Result<Config> {
        let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let cfg: Config = toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
        Ok(cfg)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let text = toml::to_string_pretty(self).context("serializing config")?;
        if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, format!("# chiaki-controller configuration (written by the settings window)

{text}"))?;
        std::fs::rename(&tmp, path).with_context(|| format!("writing {}", path.display()))?;
        Ok(())
    }

    /// The default config, parsed.
    pub fn default_config() -> Config {
        toml::from_str(DEFAULT_CONFIG).expect("built-in default config")
    }

    pub fn credentials(&self) -> Result<Credentials> {
        if self.console.regist_key.is_empty() || self.console.rp_key.is_empty() {
            bail!("the config has no console credentials yet; run the `register` command first");
        }
        let rk = http::parse_hex(&self.console.regist_key).context("console.regist_key")?;
        if rk.len() > 16 {
            bail!("console.regist_key is too long");
        }
        let mut regist_key = [0u8; 16];
        regist_key[..rk.len()].copy_from_slice(&rk);
        let morning: [u8; 16] = http::parse_hex(&self.console.rp_key)
            .context("console.rp_key")?
            .try_into()
            .map_err(|_| anyhow::anyhow!("console.rp_key must be 32 hex characters"))?;
        Ok(Credentials { regist_key, morning })
    }
}

/// The config written by `init` / `register`: a commented default layout.
pub const DEFAULT_CONFIG: &str = include_str!("../config.example.toml");
