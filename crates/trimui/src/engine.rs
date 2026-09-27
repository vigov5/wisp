//! Bridge between the single-threaded UI loop and the async receiver.
//!
//! The UI owns the framebuffer and must never block, so the tokio runtime and
//! [`ReceiverService`] live on their own threads and talk to the frame loop
//! through two channels: events out on a `std::sync::mpsc` the loop drains once
//! per frame, commands in on a tokio channel.
//!
//! Policy decisions — in particular whether an offer is auto-accepted — are
//! deliberately *not* made here. The engine reports the offer and the app
//! decides, so the trust rules stay in one testable place next to the config.

use std::path::PathBuf;
use std::sync::mpsc as std_mpsc;
use std::time::Duration;

use anyhow::{Context, Result};
use iroh::SecretKey;
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::mpsc as tokio_mpsc;
use tokio_stream::StreamExt;
use wisp_app::{
    ConflictPolicy, NearbyReceiver, OfferDecision, PairingCodeState, QrPairingInfo, ReceiverConfig,
    ReceiverEvent, ReceiverOfferEvent, ReceiverService, SendConfig, SendDestination, SendDraft,
    SendEvent, SendInput, SendSession,
};

use crate::config::{Config, Conflict};

/// How often the QR payload is rebuilt. The pairing ticket embeds the
/// device's LAN addresses, and Wi-Fi often associates *after* the app has
/// started, so a code rendered once at launch would stay undialable.
const QR_REFRESH: Duration = Duration::from_secs(3);

#[derive(Debug, Clone)]
pub enum EngineEvent {
    /// The receiver is listening. Transfers can arrive from here on.
    ///
    /// Carries this device's endpoint id — its public key, and the identity a
    /// sender stores when it remembers the handheld. Shown on the home screen
    /// so the two ends can be compared by eye.
    Ready {
        endpoint_id: String,
    },
    /// A new rendezvous short code, or the loss of one.
    Code(PairingCodeState),
    /// Registering a short code failed — usually no internet. Not fatal: LAN
    /// discovery and QR pairing both keep working without the rendezvous
    /// server, so this is surfaced as a hint rather than an error screen.
    CodeUnavailable(String),
    /// A fresh offline-pairing payload for the QR panel.
    Pairing(QrPairingInfo),
    Offer(ReceiverOfferEvent),
    /// Result of a nearby scan: the devices found, or the reason there are
    /// none.
    Nearby(Result<Vec<NearbyReceiver>, String>),
    /// Progress of the outbound transfer.
    Send(SendEvent),
    /// The receiver could not start at all.
    Fatal(String),
}

#[derive(Debug, Clone)]
pub enum EngineCommand {
    Respond(OfferDecision),
    Cancel,
    /// Ask the rendezvous server for a new short code.
    RefreshCode,
    /// Browse the LAN for receivers, for the send destination picker.
    ScanNearby {
        timeout_secs: u64,
    },
    /// Send `paths` to `destination`.
    StartSend {
        paths: Vec<PathBuf>,
        destination: SendTarget,
    },
    /// Abort the outbound transfer.
    CancelSend,
    Shutdown,
}

/// Where a send is going, in terms the UI can hold onto.
///
/// Kept separate from `SendDestination` so the app layer never has to build
/// one: a ticket from a nearby scan and a ticket from the recent list are the
/// same thing here, which is what lets both reuse one code path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SendTarget {
    /// A six-character rendezvous code the user typed.
    Code(String),
    /// A ticket, from a nearby scan or a previously used device.
    Ticket { ticket: String, label: String },
}

impl SendTarget {
    pub fn label(&self) -> &str {
        match self {
            SendTarget::Code(code) => code,
            SendTarget::Ticket { label, .. } => label,
        }
    }
}

/// Everything the receiver needs, resolved from [`Config`] before start.
#[derive(Debug, Clone)]
pub struct EngineSettings {
    pub device_name: String,
    pub save_root: PathBuf,
    pub conflict: Conflict,
    pub server: Option<String>,
    pub secret_key: SecretKey,
}

impl EngineSettings {
    pub fn from_config(config: &Config, secret_key: SecretKey) -> Self {
        Self {
            device_name: config.device_name.clone(),
            save_root: config.save_root.clone(),
            conflict: config.conflict,
            server: config.server.clone(),
            secret_key,
        }
    }
}

pub struct Engine {
    events: std_mpsc::Receiver<EngineEvent>,
    commands: tokio_mpsc::UnboundedSender<EngineCommand>,
    /// Kept alive for the lifetime of the engine; dropping it stops the
    /// receiver's tasks.
    runtime: Option<tokio::runtime::Runtime>,
}

impl Engine {
    pub fn start(settings: EngineSettings) -> Result<Self> {
        // The TG4040 boots with cores 2-3 parked, so two workers is the honest
        // width here; more would just add scheduler overhead on a 1 GB device.
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .thread_name("wisp-trimui")
            .build()
            .context("build the tokio runtime")?;

        let (event_tx, event_rx) = std_mpsc::channel();
        let (command_tx, command_rx) = tokio_mpsc::unbounded_channel();

        runtime.spawn(drive(settings, event_tx, command_rx));

        Ok(Self {
            events: event_rx,
            commands: command_tx,
            runtime: Some(runtime),
        })
    }

    /// Drains everything the receiver has produced since the last frame.
    pub fn poll(&self) -> Vec<EngineEvent> {
        let mut out = Vec::new();
        while let Ok(event) = self.events.try_recv() {
            out.push(event);
        }
        out
    }

    pub fn send(&self, command: EngineCommand) {
        // A closed channel means the driver already exited; the UI will see
        // that through the events it stops receiving, so this is not an error
        // worth surfacing.
        let _ = self.commands.send(command);
    }

    /// Asks the receiver to shut down, then gives it a moment to finish.
    pub fn shutdown(mut self) {
        self.send(EngineCommand::Shutdown);
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown_timeout(Duration::from_secs(2));
        }
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown_timeout(Duration::from_secs(1));
        }
    }
}

fn conflict_policy(conflict: Conflict) -> ConflictPolicy {
    match conflict {
        Conflict::Rename => ConflictPolicy::Rename,
        Conflict::Reject => ConflictPolicy::Reject,
    }
}

async fn drive(
    settings: EngineSettings,
    events: std_mpsc::Sender<EngineEvent>,
    mut commands: tokio_mpsc::UnboundedReceiver<EngineCommand>,
) {
    let server = settings.server.clone();
    // Kept because an outbound send needs to introduce this device by name,
    // and the receiver config takes ownership of the original.
    let device_name = settings.device_name.clone();
    let config = ReceiverConfig {
        device_name: settings.device_name,
        // The protocol's device taxonomy only has phone and laptop. A
        // handheld is neither, and "laptop" is the one that makes senders
        // show a generic non-phone tile.
        device_type: "laptop".to_owned(),
        download_root: settings.save_root,
        conflict_policy: conflict_policy(settings.conflict),
        secret_key: settings.secret_key,
    };

    let service = match ReceiverService::start(config).await {
        Ok(service) => service,
        Err(err) => {
            let _ = events.send(EngineEvent::Fatal(format!("{err}")));
            return;
        }
    };
    let endpoint_id = service.endpoint().addr().id.to_string();
    let _ = events.send(EngineEvent::Ready { endpoint_id });

    // A short code needs the rendezvous server; QR and mDNS do not. Failing
    // here must leave the receiver running.
    match service.ensure_registered(server.clone()).await {
        Ok(_) => {}
        Err(err) => {
            let _ = events.send(EngineEvent::CodeUnavailable(format!("{err}")));
        }
    }

    // Feature R3: advertise over mDNS so a sender's "nearby" list finds this
    // device without anyone typing a code.
    if let Err(err) = service.set_discoverable(true).await {
        tracing::warn!(
            target: "wisp_trimui::engine",
            error = %err,
            "mDNS advertising failed; the short code and QR still work"
        );
    }

    // Shared so a nearby scan can run on its own task: the scan blocks for
    // its full timeout, and doing it inline would freeze the event loop — and
    // with it the UI — for those seconds.
    let service = std::sync::Arc::new(service);

    // Cancel handle for the outbound transfer, when one is running.
    let mut send_cancel: Option<wisp_app::send::SendCancelHandle> = None;

    let mut receiver_events = service.subscribe_events();
    let mut pairing_code = service.subscribe_pairing_code();
    let _ = events.send(EngineEvent::Code(pairing_code.borrow().clone()));

    let mut qr_timer = tokio::time::interval(QR_REFRESH);
    let mut last_qr: Option<QrPairingInfo> = None;

    loop {
        tokio::select! {
            event = receiver_events.recv() => match event {
                Ok(ReceiverEvent::OfferUpdated(offer)) => {
                    if events.send(EngineEvent::Offer(offer)).is_err() {
                        break;
                    }
                }
                Ok(ReceiverEvent::Shutdown) => break,
                Ok(_) => {}
                Err(RecvError::Closed) => break,
                Err(RecvError::Lagged(count)) => {
                    tracing::warn!(
                        target: "wisp_trimui::engine",
                        dropped = count,
                        "receiver events lagged"
                    );
                }
            },
            changed = pairing_code.changed() => {
                if changed.is_err() {
                    break;
                }
                let state = pairing_code.borrow().clone();
                if events.send(EngineEvent::Code(state)).is_err() {
                    break;
                }
            }
            _ = qr_timer.tick() => {
                match service.qr_pairing_info() {
                    Ok(info) => {
                        // Only push on change: the panel re-encodes the QR
                        // when this arrives, and that is wasted work three
                        // times a second otherwise.
                        if last_qr.as_ref() != Some(&info) {
                            last_qr = Some(info.clone());
                            if events.send(EngineEvent::Pairing(info)).is_err() {
                                break;
                            }
                        }
                    }
                    Err(err) => tracing::debug!(
                        target: "wisp_trimui::engine",
                        error = %err,
                        "no pairing payload yet"
                    ),
                }
            }
            command = commands.recv() => match command {
                Some(EngineCommand::Respond(decision)) => {
                    if let Err(err) = service.respond_to_offer(decision).await {
                        tracing::warn!(
                            target: "wisp_trimui::engine",
                            error = %err,
                            "responding to the offer failed"
                        );
                    }
                }
                Some(EngineCommand::Cancel) => {
                    if let Err(err) = service.cancel_transfer().await {
                        tracing::warn!(
                            target: "wisp_trimui::engine",
                            error = %err,
                            "cancelling the transfer failed"
                        );
                    }
                }
                Some(EngineCommand::RefreshCode) => {
                    match service.setup(server.clone()).await {
                        Ok(_) => {}
                        Err(err) => {
                            let _ = events.send(EngineEvent::CodeUnavailable(format!("{err}")));
                        }
                    }
                }
                Some(EngineCommand::ScanNearby { timeout_secs }) => {
                    let service = std::sync::Arc::clone(&service);
                    let events = events.clone();
                    tokio::spawn(async move {
                        let found = service
                            .scan_nearby(timeout_secs)
                            .await
                            .map_err(|err| format!("{err}"));
                        let _ = events.send(EngineEvent::Nearby(found));
                    });
                }
                Some(EngineCommand::StartSend { paths, destination }) => {
                    let draft = SendDraft::new(
                        SendConfig {
                            device_name: device_name.clone(),
                            device_type: "laptop".to_owned(),
                        },
                        paths.into_iter().map(SendInput::Path).collect(),
                    );
                    let target = match destination {
                        SendTarget::Code(code) => {
                            SendDestination::code(code, server.clone())
                        }
                        SendTarget::Ticket { ticket, label } => {
                            SendDestination::nearby(ticket, label)
                        }
                    };
                    // Reuse the receiver's endpoint: binding a second one with
                    // the same secret key makes the two fight for the relay
                    // slot.
                    let session =
                        SendSession::with_endpoint(draft, target, service.endpoint());
                    let run = session.start();
                    send_cancel = Some(run.cancel_handle());
                    let (mut stream, _outcome) = run.into_parts();
                    let events = events.clone();
                    tokio::spawn(async move {
                        // The terminal SendEvent already carries the outcome,
                        // so the UI is driven from the stream alone.
                        while let Some(event) = stream.next().await {
                            if events.send(EngineEvent::Send(event)).is_err() {
                                break;
                            }
                        }
                    });
                }
                Some(EngineCommand::CancelSend) => {
                    if let Some(handle) = send_cancel.as_ref()
                        && let Err(err) = handle.cancel_transfer().await
                    {
                        tracing::warn!(
                            target: "wisp_trimui::engine",
                            error = %err,
                            "cancelling the send failed"
                        );
                    }
                }
                Some(EngineCommand::Shutdown) | None => break,
            },
        }
    }

    let _ = service.shutdown().await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conflict_maps_to_the_core_policy() {
        assert_eq!(conflict_policy(Conflict::Rename), ConflictPolicy::Rename);
        assert_eq!(conflict_policy(Conflict::Reject), ConflictPolicy::Reject);
    }

    #[test]
    fn settings_are_taken_from_the_config() {
        let mut config = Config::default();
        config.device_name = "Brick Pro".to_owned();
        config.conflict = Conflict::Reject;
        config.save_root = PathBuf::from("/mnt/SDCARD/Roms");

        let key = SecretKey::from_bytes(&[7u8; 32]);
        let settings = EngineSettings::from_config(&config, key.clone());

        assert_eq!(settings.device_name, "Brick Pro");
        assert_eq!(settings.conflict, Conflict::Reject);
        assert_eq!(settings.save_root, PathBuf::from("/mnt/SDCARD/Roms"));
        assert_eq!(settings.secret_key.to_bytes(), key.to_bytes());
    }
}
