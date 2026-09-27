//! `wisp-trimui` — the Wisp receiver for the TrimUI Brick Pro (TG4040).
//!
//! The device runs Allwinner Tina Linux with a plain `/dev/fb0` and an evdev
//! gamepad, and TrimUI has not published an SDK for this model, so the UI is
//! drawn by hand instead of through SDL2. That keeps the whole thing to one
//! statically linkable binary with no C dependencies — see `README.md` for the
//! build and install steps.

use std::time::{Duration, Instant};

use anyhow::{Context, Result};

use wisp_trimui::app::{App, AppRequest, Screen};
use wisp_trimui::config::Config;
use wisp_trimui::draw::Canvas;
use wisp_trimui::engine::{Engine, EngineSettings};
use wisp_trimui::fb;
use wisp_trimui::font::Fonts;
use wisp_trimui::input::Input;

/// How often the input devices are polled. 60 Hz keeps button presses feeling
/// immediate without spinning the CPU.
const POLL_INTERVAL: Duration = Duration::from_millis(16);

/// Redraw at least this often even when nothing happened, so the clock-like
/// parts of the UI (toast expiry, "connecting" copy) cannot get stuck.
const HEARTBEAT: Duration = Duration::from_millis(500);

fn main() -> Result<()> {
    // Read the mount table before anything else touches /mnt/SDCARD. When the
    // card is missing or busy that path is still a writable directory on the
    // internal overlay, and every write into it — the state directory and the
    // log included — leaves a stray file that the card hides on its next
    // mount. Nothing below creates a directory there until this says it is a
    // real card.
    let mounts = wisp_trimui::config::read_mounts();
    let state_writable = wisp_trimui::config::save_root_is_writable(&Config::state_dir(), &mounts);

    let _log_guard = init_tracing(state_writable);

    // Reading is harmless either way: a missing file yields defaults.
    let mut config = Config::load();
    let (secret_key, generated) = config.ensure_secret_key();
    if generated && state_writable {
        // Persist immediately: an identity that only lives in memory would
        // change on every launch and break every sender's saved entry.
        if let Err(err) = config.save() {
            tracing::warn!(
                target: "wisp_trimui",
                error = %err,
                "could not store the device identity; it will change next launch"
            );
        }
    }

    let mut framebuffer = fb::open_default()
        .context("could not open the framebuffer — run this as root, outside the stock MainUI")?;
    let width = framebuffer.width();
    let height = framebuffer.height();
    tracing::info!(target: "wisp_trimui", width, height, "starting");

    let mut canvas = Canvas::new(width, height);
    let mut fonts = Fonts::load()?;
    let mut input = Input::open(config.key_map())?;

    // Refuse to receive onto an unmounted card, for the reason given at the
    // top of `main`: the transfer would succeed, report itself saved, and
    // leave the file where the card hides it on the next mount.
    let mut app = App::new(config);
    let mut engine = None;
    if wisp_trimui::config::save_root_is_writable(&app.config.save_root, &mounts) {
        engine = Some(Engine::start(EngineSettings::from_config(
            &app.config,
            secret_key.clone(),
        ))?);
    } else {
        let path = app.config.save_root.display().to_string();
        tracing::error!(
            target: "wisp_trimui",
            save_root = %path,
            "save folder is not on a mounted filesystem; refusing to receive"
        );
        app.set_fatal(unmounted_message(&path));
    }

    let mut last_render = Instant::now() - HEARTBEAT;
    let mut dirty = true;
    let mut exit = false;

    while !exit {
        let frame_start = Instant::now();
        let mut requests = Vec::new();

        for event in input.poll() {
            requests.extend(app.handle_key(event));
            dirty = true;
        }

        if let Some(active) = engine.as_ref() {
            for event in active.poll() {
                requests.extend(app.handle_engine(event));
                dirty = true;
            }
        }

        for request in requests {
            match request {
                AppRequest::Engine(command) => {
                    if let Some(active) = engine.as_ref() {
                        active.send(command);
                    }
                }
                AppRequest::SaveConfig => {
                    if let Err(err) = app.config.save() {
                        tracing::warn!(
                            target: "wisp_trimui",
                            error = %err,
                            "could not write settings.json"
                        );
                    }
                }
                AppRequest::RestartEngine => {
                    // The save folder and conflict policy are fixed when the
                    // receiver starts, so changing either means a new one.
                    if let Some(active) = engine.take() {
                        active.shutdown();
                    }
                    match Engine::start(EngineSettings::from_config(
                        &app.config,
                        secret_key.clone(),
                    )) {
                        Ok(started) => engine = Some(started),
                        Err(err) => {
                            tracing::error!(
                                target: "wisp_trimui",
                                error = %err,
                                "could not restart the receiver"
                            );
                        }
                    }
                    dirty = true;
                }
                AppRequest::Exit => exit = true,
            }
        }

        // A transfer in flight updates continuously; everything else only
        // needs redrawing when it actually changed.
        let live = app.screen() == Screen::Transfer;
        if dirty || live || last_render.elapsed() >= HEARTBEAT {
            app.render(&mut canvas, &mut fonts);
            framebuffer.present(canvas.pixels())?;
            last_render = Instant::now();
            dirty = false;
        }

        if let Some(remaining) = POLL_INTERVAL.checked_sub(frame_start.elapsed()) {
            std::thread::sleep(remaining);
        }
    }

    if let Some(active) = engine.take() {
        active.shutdown();
    }

    // Hand the screen back black rather than leaving our last frame behind
    // for the stock launcher to reappear over.
    canvas.clear(0xFF00_0000);
    let _ = framebuffer.present(canvas.pixels());

    tracing::info!(target: "wisp_trimui", "stopped");
    Ok(())
}

/// Explains the refusal above in the one place the user can read it.
fn unmounted_message(save_root: &str) -> String {
    format!(
        "{save_root} is not on a mounted card.\n\n\
         The SD card is missing or busy. Anything received now would be \
         written to internal storage and hidden as soon as the card mounts \
         again, so nothing will be accepted.\n\n\
         Reseat the card or restart the device, then open Wisp again."
    )
}

/// Logs to a file in the state directory, so a problem on the handheld can be
/// read back over SSH, and to stderr for a desktop run.
fn init_tracing(state_writable: bool) -> Option<()> {
    use tracing_subscriber::EnvFilter;

    let filter = std::env::var("RUST_LOG")
        .ok()
        .and_then(|directives| EnvFilter::try_new(directives).ok())
        .unwrap_or_else(|| EnvFilter::new("warn,wisp_trimui=info,wisp_app=info"));

    if !state_writable {
        // The state directory is on an unmounted card; creating it would
        // leave a stray directory on internal storage. stderr still reaches
        // launch.log.
        let _ = tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_writer(std::io::stderr)
            .try_init();
        return Some(());
    }

    let log_path = Config::state_dir().join("wisp-trimui.log");
    if let Some(parent) = log_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    match std::fs::File::create(&log_path) {
        Ok(file) => {
            let result = tracing_subscriber::fmt()
                .with_env_filter(filter)
                .with_ansi(false)
                .with_writer(move || {
                    file.try_clone()
                        .unwrap_or_else(|_| std::fs::File::create("/dev/null").unwrap())
                })
                .try_init();
            if result.is_err() {
                return None;
            }
        }
        Err(_) => {
            // No writable state dir (a read-only card, a desktop run without
            // one): stderr still works.
            let _ = tracing_subscriber::fmt()
                .with_env_filter(filter)
                .with_writer(std::io::stderr)
                .try_init();
        }
    }
    Some(())
}
