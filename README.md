# chiaki-controller

An input-only PS5 Remote Play client for Windows, written in Rust. It turns your keyboard and
mouse into a DualSense (or DualShock 4) on your PS5, and does nothing else: no video decoding,
no audio, no rendering. Use it when you're watching the PS5 on a TV or capture card and just want
to play it with keyboard and mouse.

It is a port of the protocol parts of [chiaki-ng](https://github.com/streetpea/chiaki-ng)
(registration, session/ctrl, Takion transport, stream crypto, feedback packets), reduced to PS5
and Windows.

## Why it's low latency

- Every input change is sent the moment the Raw Input event arrives, on the same thread, without
  queues or a separate sender thread. Button changes go out as feedback-history packets, stick
  changes as feedback-state packets.
- Lost button releases are covered by resending the newest history packet 20/50/100/200 ms later
  (same scheme as chiaki-ng), so nothing gets stuck.
- Video and audio packets are dropped after reading one byte: no MAC check, no decryption, no
  decode. The client asks for the smallest stream (360p, 30 fps, 2 Mbps) so the console and
  network have little to send. (The console always streams video; it can't be turned off.)
- Mouse to stick conversion runs at 500 Hz with a 1 ms system timer.

## Setup

1. On the PS5: Settings > System > Remote Play > enable Remote Play. For waking from rest mode,
   also enable Settings > System > Power Saving > Features Available in Rest Mode > Stay Connected
   to the Internet and Enable Turning On PS5 from Network.
2. Get your PSN account id. If you already use chiaki-ng, it is the base64 "PSN Account-ID" in
   its settings. Otherwise follow chiaki-ng's documentation for obtaining it. The decimal number
   works too.
3. Build: `cargo build --release` (the binary is `target\release\chiaki-controller.exe`).
4. Start `chiaki-controller.exe` (no arguments). The settings window opens on the **Console** tab:
   enter the console address, the 8 digit code from Settings > System > Remote Play > Link Device,
   and your account id, then press **Register**. The console parameters (address, keys, name) are
   saved to the config right away and kept from then on.
5. On the **Run** tab press **Start**.

## The settings window and tray

- **Run**: start / stop the session, connection status, live log.
- **Console**: console address and credentials, login passcode, wake from rest mode; register a
  console; check status, wake it up, or put it into rest mode.
- **Settings**: requested video size (never decoded), DualSense vs DualShock 4, reconnect, the
  capture / end-session keys, and program options (start in the tray, start a session on launch).
- **Bindings**: profiles (new, duplicate, rename, delete), mouse settings with sliders, and the
  bindings table. Click **set** next to an input and press the key, mouse button or wheel you
  want, pick the action, and adjust amount / toggle / turbo. Duplicate inputs are flagged.

**Save** writes everything to the config file (unsaved changes are marked; **Revert** reloads it).
While a session runs, **Save & restart** applies new settings.

Closing the window hides it to the tray. The tray icon's menu has Settings, Start, Stop and Quit;
left-clicking the icon opens the settings. Starting from the tray uses the saved config.

During a session a small capture window grabs keyboard and mouse: **F8** releases / recaptures
(or click it), **F12** ends the session. Capture is released automatically when it loses focus.

The config lives in `chiaki-controller.toml` in the current directory or next to the program if
one exists there, otherwise in `%APPDATA%\chiaki-controller\`.

## Command line

The window is the default; the command line still works without it:

```
chiaki-controller init
chiaki-controller register --host 192.168.1.50 --pin 12345678 --account-id <id>
chiaki-controller run        # session + capture window, no settings window
chiaki-controller wake | sleep | status | keys
```

## Configuration

Everything is in `chiaki-controller.toml` (`init` writes a commented example). The default file
has a `shooter` profile (WASD, mouse aim, mouse buttons on triggers) and a `menu` profile; F2 / F1
switch between them.

```toml
[[profile]]
name = "shooter"

[profile.mouse]
target = "right_stick"     # right_stick | left_stick | gyro (experimental) | none
sensitivity = 0.25         # stick deflection per (mouse count / ms)
sensitivity_y = 0.8
anti_deadzone = 0.2        # skip the game's stick dead zone
curve = 1.0                # >1 = finer small movements
smoothing_ms = 10          # lower = less lag, higher = smoother with 125 Hz mice

[profile.bind]
W = "left_stick_up"
LCtrl = { action = "left_stick_scale", value = 0.45 }   # hold to walk
MouseLeft = "r2"
MouseRight = { action = "l2", toggle = true }           # toggle aim
F = { action = "square", turbo_hz = 12 }                # repeat while held
T = { action = "touch", x = 1500, y = 400 }             # finger on the touchpad
F2 = "profile:menu"
```

Actions: `cross circle square triangle l1 r1 l2 r2 l3 r3 dpad_up dpad_down dpad_left dpad_right
options create touchpad ps`, `left_stick_up/down/left/right`, `right_stick_up/down/left/right`,
`left_stick_scale`, `right_stick_scale`, `touch`, `touch_left`, `touch_right`, `profile:<name>`,
`profile_next`.

Inputs: `A`-`Z`, `0`-`9`, `F1`-`F24`, `Numpad0`-`Numpad9`, `Space`, `Enter`, `LShift`, `RCtrl`,
`LAlt`, arrows, ..., `MouseLeft`, `MouseRight`, `MouseMiddle`, `Mouse4`, `Mouse5`, `WheelUp`,
`WheelDown`, `WheelLeft`, `WheelRight` (`chiaki-controller keys` prints them all). A wheel notch
holds its action for `input.wheel_press_ms`.

`[stream]` sets the requested resolution / fps / bitrate, DualSense vs DualShock 4, and automatic
reconnect. `[console]` holds the address, the credentials, `wake` and an optional `login_pin` for
consoles with a PSN login passcode.

## Limitations

- PS5 on the local network only (no PSN remote connection / hole punching), Windows only.
- No rumble, adaptive triggers, lightbar or microphone.
- Senkusha (MTU/RTT probing) is skipped; the stream uses chiaki-ng's fallback values
  (MTU 1454, RTT 1 ms), which is fine on a LAN.
- The Windows keys are swallowed while capturing so the Start menu doesn't open; they can't be bound.

## Tests

`cargo test` runs the known-answer vectors from chiaki-ng's own test suite (ECDH, stream key
stream, GMAC, PS5 registration crypto, congestion packet MAC), feedback encoding and input mapping
tests, and a loopback test that plays the console side of a Takion handshake.

## License

AGPL-3.0 (see [LICENSE](LICENSE)), like chiaki-ng, which this is derived from. `src/rpcrypt_tables.rs` holds key tables
extracted from chiaki-ng's `lib/src/rpcrypt.c`.
