//! A running session: the session thread (wake, connect, stream, reconnect) plus the input
//! capture window on its own thread. Can be started and stopped any number of times.

use std::net::{IpAddr, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};

use crate::config::{Config, Credentials};
use crate::input::windows::{UiOptions, WindowHandle};
use crate::{input, regist, session, stream};

pub fn resolve_host(host: &str) -> Result<IpAddr> {
    if let Ok(ip) = host.trim().parse() {
        return Ok(ip);
    }
    (host.trim(), 0)
        .to_socket_addrs()
        .with_context(|| format!("resolving {host}"))?
        .find(|a| a.is_ipv4())
        .map(|a| a.ip())
        .with_context(|| format!("no IPv4 address for {host}"))
}

/// Sends a wakeup and waits until the console answers discovery as running.
fn wake_and_wait(ip: IpAddr, creds: &Credentials, stop: &AtomicBool, status: &dyn Fn(&str)) -> Result<()> {
    status("Console not reachable, waking it up ...");
    regist::wakeup(ip, creds)?;
    let deadline = Instant::now() + Duration::from_secs(45);
    let mut last_wakeup = Instant::now();
    while Instant::now() < deadline && !stop.load(Ordering::Relaxed) {
        if regist::probe(ip).map(|s| s.starts_with("200")).unwrap_or(false) {
            thread::sleep(Duration::from_secs(2)); // let Remote Play come up
            return Ok(());
        }
        if last_wakeup.elapsed() > Duration::from_secs(10) {
            let _ = regist::wakeup(ip, creds); // UDP: repeat in case it was lost
            last_wakeup = Instant::now();
        }
        thread::sleep(Duration::from_millis(500));
    }
    if stop.load(Ordering::Relaxed) {
        return Ok(());
    }
    bail!("the console did not wake up (is \"Enable Turning On PS5 from Network\" on?)")
}

/// True for errors that mean the console isn't listening (off / rest mode), as opposed to
/// the console answering and refusing.
fn is_unreachable(e: &anyhow::Error) -> bool {
    e.chain().any(|c| c.downcast_ref::<std::io::Error>().is_some())
}

pub struct Runner {
    stop: Arc<AtomicBool>,
    status: Arc<Mutex<String>>,
    window: Arc<Mutex<Option<WindowHandle>>>,
    link: Arc<stream::ControllerLink>,
    session: Option<JoinHandle<()>>,
    ui: Option<JoinHandle<Result<()>>>,
}

impl Runner {
    /// Validates the config and starts the session and the capture window.
    pub fn start(cfg: &Config) -> Result<Runner> {
        let creds = cfg.credentials()?;
        if cfg.console.host.trim().is_empty() {
            bail!("no console address set");
        }
        let ip = resolve_host(&cfg.console.host)?;
        let mut profile = stream::VideoProfile::preset(&cfg.stream.resolution, cfg.stream.fps)?;
        if let Some(b) = cfg.stream.bitrate_kbps {
            profile.bitrate_kbps = b;
        }
        let mapper = input::mapper::Mapper::new(cfg)?;
        let capture_key = input::keys::parse(&cfg.input.capture_toggle).context("capture toggle key is not a known key")?;
        let quit_key = input::keys::parse(&cfg.input.quit).context("quit key is not a known key")?;
        let tick = Duration::from_secs_f64(1.0 / cfg.input.tick_hz.clamp(50, 1000) as f64);

        let link = stream::ControllerLink::new();
        let status = Arc::new(Mutex::new(String::from("Starting ...")));
        let window: Arc<Mutex<Option<WindowHandle>>> = Arc::new(Mutex::new(None));
        let stop = Arc::new(AtomicBool::new(false));

        let session = {
            let link = link.clone();
            let status = status.clone();
            let window = window.clone();
            let stop = stop.clone();
            let cfg = cfg.clone();
            thread::Builder::new()
                .name("session".into())
                .spawn(move || session_loop(cfg, creds, ip, profile, link, status, window, stop))?
        };

        let ui = {
            let link = link.clone();
            let status = status.clone();
            let status_ui = status.clone();
            let window = window.clone();
            let stop = stop.clone();
            let capture_on_start = cfg.input.capture_on_start;
            thread::Builder::new().name("capture".into()).spawn(move || {
                let r = input::windows::run(
                    mapper,
                    link,
                    status_ui,
                    UiOptions { capture_key, quit_key, capture_on_start, tick },
                    |h| *window.lock().unwrap() = Some(h),
                );
                *window.lock().unwrap() = None;
                if let Err(e) = &r {
                    log::error!("capture window: {e:#}");
                    *status.lock().unwrap() = format!("Capture window failed: {e:#}");
                }
                // closing the capture window ends the session
                stop.store(true, Ordering::Relaxed);
                r
            })?
        };

        Ok(Runner { stop, status, window, link, session: Some(session), ui: Some(ui) })
    }

    pub fn status(&self) -> String {
        self.status.lock().map(|s| s.clone()).unwrap_or_default()
    }

    pub fn is_streaming(&self) -> bool {
        self.link.is_streaming()
    }

    /// True once the capture window is closed and the session thread has ended.
    pub fn is_finished(&self) -> bool {
        self.session.as_ref().map(|t| t.is_finished()).unwrap_or(true) && self.ui.as_ref().map(|t| t.is_finished()).unwrap_or(true)
    }

    /// Asks everything to stop without waiting.
    pub fn request_stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(w) = *self.window.lock().unwrap() {
            w.close();
        }
    }

    /// Stops and waits (up to the socket timeouts, a few seconds).
    pub fn stop_and_wait(mut self) -> Result<()> {
        self.request_stop();
        let ui = self.ui.take().map(|t| t.join());
        if let Some(t) = self.session.take() {
            let _ = t.join();
        }
        match ui {
            Some(Ok(r)) => r,
            Some(Err(_)) => bail!("capture window thread panicked"),
            None => Ok(()),
        }
    }

    /// Runs until the capture window is closed (used by the command line `run`).
    pub fn wait(mut self) -> Result<()> {
        let r = self.ui.take().map(|t| t.join());
        self.stop_and_wait()?;
        match r {
            Some(Ok(r)) => r,
            Some(Err(_)) => bail!("capture window thread panicked"),
            None => Ok(()),
        }
    }
}

impl Drop for Runner {
    fn drop(&mut self) {
        self.request_stop();
    }
}

#[allow(clippy::too_many_arguments)]
fn session_loop(
    cfg: Config,
    creds: Credentials,
    ip: IpAddr,
    profile: stream::VideoProfile,
    link: Arc<stream::ControllerLink>,
    status: Arc<Mutex<String>>,
    window: Arc<Mutex<Option<WindowHandle>>>,
    stop: Arc<AtomicBool>,
) {
    let st = |t: &str| {
        log::info!("{t}");
        *status.lock().unwrap() = t.to_string();
        if let Some(w) = *window.lock().unwrap() {
            w.refresh();
        }
    };
    let params = stream::StreamParams { ip, profile, dualsense: cfg.stream.dualsense };
    let mut backoff = Duration::from_secs(2);
    while !stop.load(Ordering::Relaxed) {
        let attempt = (|| -> Result<stream::StreamEnd> {
            st(&format!("Connecting to {ip} ..."));
            // connect first; only wake when the console isn't listening, so a running
            // console connects without waiting on (UDP) discovery
            let nonce = match session::request_session(ip, &creds) {
                Ok(n) => n,
                Err(e) if cfg.console.wake && is_unreachable(&e) => {
                    log::info!("{e:#}");
                    wake_and_wait(ip, &creds, &stop, &st)?;
                    if stop.load(Ordering::Relaxed) {
                        return Ok(stream::StreamEnd::Stopped);
                    }
                    st(&format!("Connecting to {ip} ..."));
                    session::request_session(ip, &creds)?
                }
                Err(e) => return Err(e),
            };
            let ctrl = session::Ctrl::connect(ip, &nonce, &creds, cfg.console.login_pin.as_deref(), params.dualsense)?;
            st("Starting stream ...");
            let connected_status = format!(
                "Connected to {} ({ip}) as {}",
                if cfg.console.nickname.is_empty() { "PS5" } else { &cfg.console.nickname },
                if params.dualsense { "DualSense" } else { "DualShock 4" }
            );
            let started = Instant::now();
            let done = AtomicBool::new(false);
            let end = thread::scope(|s| {
                s.spawn(|| {
                    // the stream marks the link as streaming once input is live
                    while !done.load(Ordering::Relaxed) {
                        if link.is_streaming() {
                            st(&connected_status);
                            break;
                        }
                        thread::sleep(Duration::from_millis(50));
                    }
                });
                let r = stream::run(&params, &ctrl, &link, &stop);
                done.store(true, Ordering::Relaxed);
                r
            });
            if started.elapsed() > Duration::from_secs(30) {
                backoff = Duration::from_secs(2);
            }
            end
        })();
        if stop.load(Ordering::Relaxed) {
            break;
        }
        let msg = match attempt {
            Ok(stream::StreamEnd::Stopped) => break,
            Ok(stream::StreamEnd::RemoteDisconnected(r)) => format!("Console ended the session: {r}"),
            Ok(stream::StreamEnd::CtrlFailed) => "Session control channel closed".to_string(),
            Err(e) => format!("Error: {e:#}"),
        };
        if !cfg.stream.reconnect {
            st(&format!("{msg}\nReconnect is off."));
            break;
        }
        st(&format!("{msg}\nRetrying in {}s ...", backoff.as_secs()));
        let until = Instant::now() + backoff;
        while Instant::now() < until && !stop.load(Ordering::Relaxed) {
            thread::sleep(Duration::from_millis(100));
        }
        backoff = (backoff * 2).min(Duration::from_secs(30));
    }
    *status.lock().unwrap() = "Stopped".into();
}
