#![allow(dead_code)]

mod actor;
mod runtime;
mod session;
mod wisp_handler;

#[cfg(test)]
mod tests;

use std::time::Duration;

use iroh::{Endpoint, RelayMode, endpoint::presets, protocol::Router};
use tokio::sync::{broadcast, mpsc, oneshot, watch};

use crate::blob_dispatcher::BlobDispatcher;
use crate::error::{AppError, AppResult};
use crate::types::{
    ConflictPolicy, ConnectionPath, NearbyReceiver, PairingCodeState, QrPairingInfo,
    ReceiverConfig, ReceiverOfferEvent, ReceiverRegistration,
};
use wisp_core::protocol::{ALPN, DeviceType};

use self::actor::{ReceiverCommand, run_receiver_actor};
use self::runtime::ReceiverRuntime;
use self::wisp_handler::WispProtocolHandler;

/// How long an abandoned per-transfer record dir under
/// `<download_root>/.wisp/transfers/<hash>/` is kept on disk before
/// the receiver-service startup sweep deletes it.  Failed /
/// connection-dropped transfers leave their resume state behind so
/// the user can retry; this TTL bounds how long that state lingers
/// before we GC it.
const STALE_TRANSFER_RECORD_TTL: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// Maps a core LAN scan hit to the app-facing [`NearbyReceiver`], decoding the
/// endpoint id from the ticket (best-effort; empty when the ticket won't parse).
fn map_core_nearby_receiver(receiver: wisp_core::lan::NearbyReceiver) -> NearbyReceiver {
    let endpoint_id = wisp_core::util::decode_ticket(&receiver.ticket)
        .map(|a| a.id.to_string())
        .unwrap_or_default();
    NearbyReceiver {
        fullname: receiver.fullname,
        label: receiver.label,
        device_type: match receiver.device_type {
            DeviceType::Phone => "phone".to_owned(),
            DeviceType::Laptop => "laptop".to_owned(),
        },
        code: receiver.code,
        ticket: receiver.ticket,
        endpoint_id,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReceiverLifecycle {
    Starting,
    Ready,
    Stopped,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReceiverSnapshot {
    pub lifecycle: ReceiverLifecycle,
    pub discoverable_requested: bool,
    pub advertising_active: bool,
    pub has_registration: bool,
    pub has_pending_offer: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OfferDecision {
    Accept,
    Decline,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReceiverEvent {
    RegistrationUpdated(ReceiverRegistration),
    SetupCompleted(ReceiverRegistration),
    DiscoverabilityChanged {
        requested: bool,
        active: bool,
    },
    OfferUpdated(ReceiverOfferEvent),
    ConnectionPathChanged {
        offer_id: u64,
        connection_path: ConnectionPath,
    },
    Shutdown,
}

#[derive(Debug)]
pub struct ReceiverService {
    cmd_tx: mpsc::Sender<ReceiverCommand>,
    state_rx: watch::Receiver<ReceiverSnapshot>,
    pairing_rx: watch::Receiver<PairingCodeState>,
    event_tx: broadcast::Sender<ReceiverEvent>,
    endpoint: Endpoint,
    device_name: String,
    device_type: String,
}

impl ReceiverService {
    pub async fn start(config: ReceiverConfig) -> AppResult<Self> {
        // Bind a single endpoint that advertises **both** ALPNs we speak:
        // `wisp_core::protocol::ALPN` for handshake/control and
        // `iroh_blobs::ALPN` for data transfers.  An active sender (running
        // in the same process) reuses this endpoint via `BlobDispatcher`
        // rather than binding its own — that's what avoids the "two
        // endpoints with the same secret key fight for the relay slot"
        // failure mode.
        let endpoint = Endpoint::builder(presets::N0)
            .alpns(vec![ALPN.to_vec(), iroh_blobs::ALPN.to_vec()])
            .relay_mode(RelayMode::Default)
            .transport_config(crate::quic_keepalive::build_transport_config())
            .secret_key(config.secret_key.clone())
            .bind()
            .await
            .map_err(|e| AppError::BindingFailed {
                context: format!("receiver v2 endpoint: {e}"),
            })?;

        if matches!(config.conflict_policy, ConflictPolicy::Overwrite) {
            return Err(AppError::UnsupportedLocalOperation {
                operation: "receiver overwrite policy",
            });
        }
        let device_type = parse_device_type(&config.device_type)?;
        if let Err(err) = tokio::fs::create_dir_all(&config.download_root).await {
            return Err(AppError::Internal {
                message: format!(
                    "could not prepare save location {}: {err}",
                    config.download_root.display()
                ),
            });
        }

        // Sweep abandoned `.wisp/transfers/<hash>/` dirs older than the TTL.
        // Failed / connection-dropped transfers leave their per-transfer
        // resume state on disk so the user can retry, but if no retry
        // happens within the TTL we GC them so the cache doesn't grow
        // unbounded.  Best-effort: a sweep error doesn't block startup.
        let swept = wisp_core::transfer::path::sweep_stale_transfer_records(
            &config.download_root,
            STALE_TRANSFER_RECORD_TTL,
        )
        .await;
        if swept > 0 {
            tracing::info!(
                target: "wisp_app::receiver",
                count = swept,
                "swept stale transfer record dirs older than {}d",
                STALE_TRANSFER_RECORD_TTL.as_secs() / 86_400,
            );
        }

        // Channel sized for high-throughput receivers.  Progress events emit
        // every ~100 ms at peak — 10 events/sec.  16 used to be enough but
        // any pause in the actor loop (e.g. the HTTP register call during
        // `refresh_registration_after_offer`) lets the producer overflow
        // 16 slots in <2 s, causing `try_send` in
        // `ReceiverSession::handle_progress` to drop updates silently —
        // visible to users as a UI speed indicator stuck well below the
        // real transfer rate.  256 slots absorbs a multi-second actor
        // stall at 10 Hz.  Long-term fix: progress should ride a
        // `watch::channel` so the latest value always wins and the actor
        // can't fall behind.  For now, the extra capacity is the cheap
        // pragmatic fix.
        let (cmd_tx, cmd_rx) = mpsc::channel(256);
        let (state_tx, state_rx) = watch::channel(ReceiverSnapshot {
            lifecycle: ReceiverLifecycle::Ready,
            discoverable_requested: false,
            advertising_active: false,
            has_registration: false,
            has_pending_offer: false,
        });
        let (pairing_tx, pairing_rx) = watch::channel(PairingCodeState::Unavailable);
        // Broadcast channel for fan-out to Dart subscribers.  Each
        // subscriber has its own backlog of `capacity` items; iroh-fast
        // transfers can fill 32 in well under a second when the Flutter
        // bridge is busy rebuilding the progress UI on the platform
        // thread.  Bumped to 256 to match `cmd_tx` capacity — see comment
        // there.  (broadcast::Sender drops the oldest item per slow
        // subscriber rather than blocking the producer, so over-capacity
        // is "missed UI updates", never deadlock.)
        let (event_tx, _) = broadcast::channel(256);

        // Build the Router that multiplexes inbound ALPNs.  We accept
        // `wisp_ALPN` ourselves (delegates to `ReceiverSession`) and route
        // `iroh_blobs::ALPN` to the global `BlobDispatcher`, which forwards
        // to whichever sender (in the same process) is currently active.
        let wisp_handler = WispProtocolHandler::new(
            cmd_tx.clone(),
            config.download_root.clone(),
            config.device_name.clone(),
            device_type,
            config.conflict_policy,
            endpoint.clone(),
        );
        let router = Router::builder(endpoint.clone())
            .accept(ALPN, wisp_handler)
            .accept(iroh_blobs::ALPN, BlobDispatcher::global())
            .spawn();
        tracing::info!(
            target: "wisp_app::receiver",
            endpoint_id = %endpoint.addr().id,
            "receiver service started; Router accepting wisp_ALPN + iroh_blobs ALPN \
             on shared endpoint"
        );

        let endpoint_for_service = endpoint.clone();
        let device_name_for_service = config.device_name.clone();
        let device_type_for_service = config.device_type.clone();
        let runtime = ReceiverRuntime::new(config, endpoint, router);

        tokio::spawn(run_receiver_actor(
            runtime,
            cmd_rx,
            state_tx,
            pairing_tx,
            event_tx.clone(),
        ));

        Ok(Self {
            cmd_tx,
            state_rx,
            pairing_rx,
            event_tx,
            endpoint: endpoint_for_service,
            device_name: device_name_for_service,
            device_type: device_type_for_service,
        })
    }

    /// Returns a ticket built from currently-known addresses (no
    /// `online()` wait — works offline-LAN) plus the LAN-routable direct
    /// socket addresses. Used by the QR pairing screen so the sender can
    /// dial directly via UDP without needing internet.
    /// Clone of the underlying iroh endpoint. Shared with `SendSession` so
    /// outbound transfers reuse the receiver's already-bound endpoint
    /// instead of binding a second one with the same secret key (which
    /// would race for the relay slot).
    pub fn endpoint(&self) -> Endpoint {
        self.endpoint.clone()
    }

    pub fn qr_pairing_info(&self) -> Result<QrPairingInfo, wisp_core::util::TicketError> {
        let ticket =
            wisp_core::util::make_qr_payload(&self.endpoint, &self.device_name, &self.device_type)?;
        let lan_ips = wisp_core::util::lan_direct_addrs(&self.endpoint)
            .into_iter()
            .map(|s| s.to_string())
            .collect();
        Ok(QrPairingInfo { ticket, lan_ips })
    }

    pub fn snapshot(&self) -> ReceiverSnapshot {
        self.state_rx.borrow().clone()
    }

    pub fn subscribe_state(&self) -> watch::Receiver<ReceiverSnapshot> {
        self.state_rx.clone()
    }

    pub fn pairing_code(&self) -> PairingCodeState {
        self.pairing_rx.borrow().clone()
    }

    pub fn subscribe_pairing_code(&self) -> watch::Receiver<PairingCodeState> {
        self.pairing_rx.clone()
    }

    pub fn subscribe_events(&self) -> broadcast::Receiver<ReceiverEvent> {
        self.event_tx.subscribe()
    }

    pub async fn setup(&self, server_url: Option<String>) -> AppResult<ReceiverRegistration> {
        self.call_registration_command(|reply| ReceiverCommand::Setup { server_url, reply })
            .await
    }

    pub async fn ensure_registered(
        &self,
        server_url: Option<String>,
    ) -> AppResult<ReceiverRegistration> {
        self.call_registration_command(|reply| ReceiverCommand::EnsureRegistered {
            server_url,
            reply,
        })
        .await
    }

    pub async fn set_discoverable(&self, enabled: bool) -> AppResult<()> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.cmd_tx
            .send(ReceiverCommand::SetDiscoverable {
                enabled,
                reply: reply_tx,
            })
            .await
            .map_err(|_| AppError::ActorStopped {
                action: "setting discoverable",
            })?;
        reply_rx.await.map_err(|_| AppError::ActorDroppedReply {
            action: "setting discoverable",
        })?
    }

    pub async fn respond_to_offer(&self, decision: OfferDecision) -> AppResult<()> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.cmd_tx
            .send(ReceiverCommand::RespondToOffer {
                decision,
                reply: reply_tx,
            })
            .await
            .map_err(|_| AppError::ActorStopped {
                action: "responding to offer",
            })?;
        reply_rx.await.map_err(|_| AppError::ActorDroppedReply {
            action: "responding to offer",
        })?
    }

    pub async fn cancel_transfer(&self) -> AppResult<()> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.cmd_tx
            .send(ReceiverCommand::CancelTransfer { reply: reply_tx })
            .await
            .map_err(|_| AppError::ActorStopped {
                action: "cancelling transfer",
            })?;
        reply_rx.await.map_err(|_| AppError::ActorDroppedReply {
            action: "cancelling transfer",
        })?
    }

    pub async fn scan_nearby(&self, timeout_secs: u64) -> AppResult<Vec<NearbyReceiver>> {
        // Deliberately bypasses the actor command queue. The scan is pure
        // UDP/mDNS and only needs our own endpoint id (to drop self from the
        // results). The actor also performs *blocking* rendezvous-registration
        // HTTP — at startup and on a 15s maintenance tick — which hangs when
        // there's no internet (USB-cable / isolated-LAN transfers). Queuing the
        // scan behind that starved discovery entirely, defeating the whole
        // point of an offline link, so we run the browse directly here.
        let exclude = Some(self.endpoint.addr().id);
        let timeout = Duration::from_secs(timeout_secs.max(1));
        let receivers = tokio::task::spawn_blocking(move || {
            wisp_core::lan::browse_nearby_receivers(timeout, exclude)
        })
        .await
        .map_err(|e| AppError::Internal {
            message: format!("nearby scan task: {e}"),
        })?
        .map_err(|e| AppError::Internal {
            message: format!("nearby scan error: {e}"),
        })?;

        Ok(receivers
            .into_iter()
            .map(map_core_nearby_receiver)
            .collect())
    }

    pub async fn shutdown(&self) -> AppResult<()> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.cmd_tx
            .send(ReceiverCommand::Shutdown { reply: reply_tx })
            .await
            .map_err(|_| AppError::ActorStopped {
                action: "shutting down",
            })?;
        reply_rx.await.map_err(|_| AppError::ActorDroppedReply {
            action: "shutting down",
        })?
    }

    async fn call_registration_command(
        &self,
        command: impl FnOnce(oneshot::Sender<AppResult<ReceiverRegistration>>) -> ReceiverCommand,
    ) -> AppResult<ReceiverRegistration> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.cmd_tx
            .send(command(reply_tx))
            .await
            .map_err(|_| AppError::ActorStopped {
                action: "registration command",
            })?;
        reply_rx.await.map_err(|_| AppError::ActorDroppedReply {
            action: "registration command",
        })?
    }
}

pub(super) fn parse_device_type(value: &str) -> AppResult<DeviceType> {
    match value.trim().to_ascii_lowercase().as_str() {
        "phone" => Ok(DeviceType::Phone),
        "laptop" => Ok(DeviceType::Laptop),
        other => Err(AppError::InvalidDeviceType {
            value: other.to_owned(),
        }),
    }
}
