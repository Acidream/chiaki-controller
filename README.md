# chiaki-controller

Play your PS5 with keyboard and mouse from a Windows PC. chiaki-controller is an input-only PS5
Remote Play client: your keyboard and mouse become a DualSense (or DualShock 4) on the console,
and it does nothing else: no video, no audio. Use it when you watch the PS5 on a TV, monitor or
capture card and just want to control it from the PC.

It is made to work over Wi-Fi. Your input is a few small packets, the console is asked for the
smallest video stream (which is thrown away unread), and nothing waits on video or audio, so a PC
on Wi-Fi doesn't get the lag and stutter of a full Remote Play stream. Tested with GTA VI.

**[Download chiaki-controller.exe](https://github.com/Acidream/chiaki-controller/releases/latest)**
(Windows 10 / 11, 64-bit, no installer)

## Getting started

1. **Prepare the PS5**: Settings › System › Remote Play › turn on **Enable Remote Play**.
   To let the app wake the PS5 from rest mode, also turn on Settings › System › Power Saving ›
   Features Available in Rest Mode › **Stay Connected to the Internet** and **Enable Turning On
   PS5 from Network**. The PC and the PS5 must be on the same network.
2. **Download** `chiaki-controller.exe` from the
   [latest release](https://github.com/Acidream/chiaki-controller/releases/latest) and start it.
   The exe isn't signed, so Windows SmartScreen may warn about it: choose *More info › Run anyway*.
3. **Follow the setup guide**, which opens the first time:
   1. **Find PS5**: consoles on your network are listed with their name and whether they are on
      or in rest mode (or type the address).
   2. **Sign in**: press **Sign in with PlayStation…**. Sony's sign-in page opens in your
      browser; sign in, and when the page shows "redirect", copy the address bar. The app notices
      the copied address and gets your account ID from Sony (see [below](#is-signing-in-safe)).
   3. **Link**: on the PS5 open Settings › System › Remote Play › **Link Device** and type the
      8-digit code into the app.
   4. **Test**: the app connects once. If your PS5 user has a login passcode, it asks for it here.
4. **Play**: on the Play tab press **Connect**. The window takes over your keyboard and mouse.
   **F8** gives you your mouse back (press it again to continue playing), **F12** disconnects.

### Default keys

Two profiles come ready to use; the Bindings tab shows and changes every key.

| Shooter (starts with this one) | | Menu | |
|---|---|---|---|
| W A S D | left stick | W A S D, arrows | D-pad |
| Mouse | right stick (aim) | Enter, Space, left click | ✕ (cross) |
| Left click / right click | R2 fire / L2 aim | Backspace, right click | ○ (circle) |
| Space | ✕ jump | Q / E | L1 / R1 |
| C | ○ crouch | Esc | Options |
| R, F | □ reload / interact | Tab | Touchpad |
| 1, wheel | △ switch weapon | Home | PS button |
| Q | L1 | **F1** | switch to Shooter |
| E, G | R1 | | |
| LShift / V, middle click | L3 sprint / R3 melee | | |
| LCtrl (hold) | walk slowly | | |
| LAlt (hold) | mouse holds the right stick in place (hotwiring, lock picking) | | |
| Arrows, Z / X, Mouse 4 / 5 | D-pad | | |
| Esc / Backspace / Tab / Home | Options / Create / Touchpad / PS | | |
| **F2** | switch to Menu | | |

### Is signing in safe?

Your password is only typed into Sony's own site in your own browser, never into this app (there
is no embedded login window). Only an address on Sony's exact redirect page is accepted. The
one-time code it contains is exchanged once with Sony over verified HTTPS and is never logged or
saved, and the clipboard is cleared after it is read. Only the account ID itself is kept, in your
config file on this PC. If you already use chiaki-ng, you can paste its base64 "PSN Account-ID"
instead of signing in.

## The window and tray

- **Play**: one Connect / Disconnect button and what's happening in plain words (waking the PS5,
  connecting, signing in, connected). Problems are explained with what to do, for example
  "Can't reach the PS5", "Remote Play is already in use", "Your PS5 asks for your login passcode"
  (with a field to enter it right there) or "The PS5 doesn't recognise this PC" (with a button to
  link again). The technical details and the log are folded away underneath. While connected,
  this tab is where you play (see below), with a live controller drawing of exactly what the PS5
  receives and the active profile's keys next to it.
- **Connection options**: the linked PS5, login passcode, wake from rest mode, check status /
  wake up / rest mode, and "Link a different PS5…". Address and pairing keys are under *Advanced*.
- **Settings**: requested video size (never decoded), DualSense vs DualShock 4, reconnect, the
  release-mouse and disconnect keys, and program options (start in the tray, connect on launch).
- **Bindings**: profiles (new, duplicate, rename, delete), mouse settings with sliders, and the
  bindings table. Click **set** next to an input and press the key, mouse button or wheel you
  want, pick the action, and adjust amount / toggle / turbo. Duplicate inputs are flagged.

**Save** writes everything to the config file (unsaved changes are marked; **Revert** reloads it).
While connected, **Save changes & reconnect** applies new settings.

Closing the window hides it to the tray. The tray icon shows the state by colour (blue: not
connected, amber: connecting, green: connected, red: needs attention) and in its tooltip; its menu
has Open, Connect, Disconnect and Quit.

While connected, the window itself takes over keyboard and mouse (the cursor is hidden and kept
inside it) and switches to the Play tab. A banner says what's going on: green "Keyboard and mouse
control your PS5", amber "Waiting for the PS5…", or grey "Mouse released" with a
**Control the PS5** button. **F8** releases / recaptures, **F12** disconnects. Keyboard and mouse
are released automatically when the window goes to the background or is hidden to the tray;
**Connect** in the tray menu brings the window to the front.

The config lives in `chiaki-controller.toml` in the current directory or next to the program if
one exists there, otherwise in `%APPDATA%\chiaki-controller\`.

## Command line

The window is the default; the command line still works without it:

```
chiaki-controller init
chiaki-controller account-id # sign in with PlayStation, prints your account ID
chiaki-controller register --host 192.168.1.50 --pin 12345678 --account-id <id>
chiaki-controller run        # session + a small capture window, no settings window
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

## How it works

It is a port of the protocol parts of [chiaki-ng](https://github.com/streetpea/chiaki-ng)
(registration, session/ctrl, Takion transport, stream crypto, feedback packets), reduced to PS5
and Windows.

Why it's low latency:

- Every input change is sent the moment the Raw Input event arrives: the window hands it straight
  to the input thread, which applies the bindings and sends at once (events that arrive together
  go out as one update). Button changes go out as feedback-history packets, stick changes as
  feedback-state packets.
- Lost button releases are covered by resending the newest history packet 20/50/100/200 ms later
  (same scheme as chiaki-ng), so nothing gets stuck.
- Video and audio packets are dropped after reading one byte: no MAC check, no decryption, no
  decode. The client asks for the smallest stream (360p, 30 fps, 2 Mbps) so the console and
  network have little to send. (The console always streams video; it can't be turned off.)
- Mouse to stick conversion runs at 500 Hz with a 1 ms system timer.

## Building from source

```
cargo build --release
```

The program is `target\release\chiaki-controller.exe`.

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
