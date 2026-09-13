use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex};

use tokio::sync::Semaphore;

use futures_lite::StreamExt;
use wisp_app::{
    send::SendCancelHandle, AppError, CandidatePath as AppCandidatePath,
    ConnectionPath as AppConnectionPath, ConnectionPathKind as AppConnectionPathKind, SendConfig,
    SendDestination, SendDraft, SendEvent as AppSendEvent, SendInput, SendPhase as AppSendPhase,
    SendSession, SendSessionOutcome,
};
use wisp_core::transfer::{TransferPhase, TransferSnapshot};

use super::transfer::{map_plan, TransferPhaseData, TransferPlanData, TransferSnapshotData};
use super::RUNTIME;
use crate::api::error::internal_user_facing_error;
use crate::api::error::map_optional_user_facing_error;
use crate::frb_generated::StreamSink;

const DEFAULT_RENDEZVOUS_URL: &str = "https://rendezvous.wisp.mooo.com";
/// The send this process is running, or is about to.
struct ActiveSend {
    generation: u64,
    /// `None` between [`start_send_transfer`] returning and the spawned task
    /// actually starting the session — it has to wait for [`SEND_SLOT`] first.
    handle: Option<SendCancelHandle>,
    /// A cancel that arrived while `handle` was still `None`.
    ///
    /// Without this the cancel was simply lost: the slot was empty, so
    /// `cancel_active_send_transfer` reported "nothing to cancel" and the
    /// session it was meant to stop went on to accept the transfer. Observed on
    /// a device as a send that kept going after Cancel, reaching the receiver
    /// as a mid-transfer cancellation seconds later instead of withdrawing the
    /// offer. Recording the request lets the session cancel itself the moment
    /// it has something to cancel.
    cancel_requested: bool,
}

static ACTIVE_SEND: LazyLock<Mutex<Option<ActiveSend>>> = LazyLock::new(|| Mutex::new(None));

/// Serializes send sessions: exactly one permit, held for a whole send.
///
/// A send used to be started *before* the previous one was told to stop, and
/// the stop was fire-and-forget on top of that. Picking a file, leaving the
/// screen and picking again therefore left two sessions hashing the same
/// descriptors at once — observed in the field as three overlapping sessions,
/// the abandoned one finishing its hash 3 s after its replacement had started
/// and then releasing the picker's file descriptor out from under it. The
/// replacement died importing a descriptor that no longer existed.
static SEND_SLOT: LazyLock<Semaphore> = LazyLock::new(|| Semaphore::new(1));

/// Bumped by every [`start_send_transfer`]. A send that wins the permit only
/// to find the counter has moved on was superseded while it queued, so it
/// steps aside instead of running a transfer the user has already replaced.
static SEND_GENERATION: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendTransferPhase {
    /// Hashing and importing the picked files — nothing has been sent yet.
    Preparing,
    Connecting,
    WaitingForDecision,
    Accepted,
    Declined,
    Sending,
    Completed,
    Cancelled,
    Failed,
}

/// One picked source, as Dart hands it to the core.
#[derive(Debug, Clone)]
pub struct SendSourceData {
    /// The path the core opens.  On Android a SAF pick travels here as
    /// `/proc/self/fd/<n>` so the file never has to be copied into the app
    /// cache first; the descriptor is held open natively for the whole
    /// transfer, because the blob store reopens this path lazily as it serves.
    pub path: String,
    /// Where a descriptor source lands on the receiver.  Required there, since
    /// `/proc/self/fd/<n>` ends in the fd number rather than a file name.  A
    /// bare file name for a picked file; a relative path
    /// (`photos/trip/cat.jpg`) for one file of a picked folder, which is sent
    /// as one descriptor per file.  `None` marks an ordinary path, which names
    /// itself.
    pub fd_transfer_path: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SendTransferRequest {
    pub code: String,
    pub sources: Vec<SendSourceData>,
    pub server_url: Option<String>,
    pub device_name: String,
    pub device_type: String,
    pub ticket: Option<String>,
    pub lan_destination_label: Option<String>,
    /// Text-only send.  When set, `sources` is ignored and the text is shared
    /// inline (≤ 16 KB) or as a synthetic `.txt` for larger payloads.
    pub inline_text: Option<String>,
}

fn map_source(source: SendSourceData) -> SendInput {
    let path = PathBuf::from(source.path);
    match source.fd_transfer_path {
        Some(transfer_path) => SendInput::FileDescriptor {
            path,
            transfer_path,
        },
        None => SendInput::Path(path),
    }
}

#[derive(Debug, Clone)]
pub struct SendConnectionPath {
    pub kind: String,
    pub relay_url: Option<String>,
    pub direct_addr: Option<String>,
}

/// One candidate transport address iroh is attempting for the peer, surfaced
/// to Dart so the connecting screen can list every IP/relay being tried.
/// `kind` uses the same `"p2p"`/`"relay"` labels as [`SendConnectionPath`].
#[derive(Debug, Clone)]
pub struct SendConnectionCandidate {
    pub addr: String,
    pub kind: String,
    pub active: bool,
}

#[derive(Debug, Clone)]
pub struct SendTransferEvent {
    pub phase: SendTransferPhase,
    pub destination_label: String,
    pub status_message: String,
    pub item_count: u64,
    pub total_size: u64,
    pub bytes_sent: u64,
    pub plan: Option<TransferPlanData>,
    pub snapshot: Option<TransferSnapshotData>,
    pub remote_device_type: Option<String>,
    pub remote_endpoint_id: Option<String>,
    /// True when the peer is a throwaway browser-receiver identity (ephemeral
    /// key). Dart skips remembering it in Recent/Saved. `None` until known.
    pub remote_ephemeral: Option<bool>,
    /// Re-serialized ticket of the resolved peer address (see
    /// `wisp_app::types::SendEvent::remote_ticket`).  Surfaced to Dart so
    /// the saved-devices repo can persist a `lastTicket` for code-based
    /// sends — otherwise Recent tile shows "no cached connection info".
    pub remote_ticket: Option<String>,
    /// Bytes hashed so far while `Preparing`. `None` in every other phase.
    /// Deliberately not folded into `bytes_sent`: nothing is sent during that
    /// phase, and the ring is driven off this against `total_size`.
    pub bytes_hashed: Option<u64>,
    pub connection_path: Option<SendConnectionPath>,
    /// Every candidate path iroh is attempting, tagged active/idle. Drives the
    /// per-candidate rows on the connecting screen. Empty outside Connecting /
    /// when iroh has no candidates yet.
    pub connection_candidates: Vec<SendConnectionCandidate>,
    pub error: Option<crate::api::error::UserFacingErrorData>,
}

pub fn start_send_transfer(
    request: SendTransferRequest,
    updates: StreamSink<SendTransferEvent>,
) -> Result<(), crate::api::error::UserFacingErrorData> {
    let fallback_destination = fallback_destination_label(&request);

    let config = SendConfig {
        device_name: request.device_name,
        device_type: request.device_type,
    };
    let draft = match request.inline_text {
        Some(text) => SendDraft::new_text(config, text),
        None => SendDraft::new(
            config,
            request.sources.into_iter().map(map_source).collect(),
        ),
    };

    let destination = match request
        .ticket
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(ticket) => SendDestination::nearby(
            ticket.to_owned(),
            request
                .lan_destination_label
                .unwrap_or_else(|| "Nearby receiver".to_owned()),
        ),
        None => SendDestination::code(
            request.code,
            request
                .server_url
                .or(Some(DEFAULT_RENDEZVOUS_URL.to_owned())),
        ),
    };

    // Reuse the receiver service's already-bound iroh endpoint when
    // available so the process holds exactly one endpoint per secret key —
    // avoids the "Another endpoint connected with the same endpoint id"
    // relay rejection that used to break cross-network sends when both the
    // long-lived receiver service and a fresh send session each tried to
    // claim the relay slot.  Blob serving on the shared endpoint goes
    // through `BlobDispatcher::global()`, plugged into the receiver
    // service's `iroh::protocol::Router` for `iroh_blobs::ALPN`.
    //
    // Fallback to binding a private endpoint when no receiver service is
    // running (rare — only happens if Dart starts a send before
    // `current_service_endpoint()` is initialised).  In that fallback path
    // the old internal-router blob serving still works because no other
    // process-local endpoint is competing for the relay slot.
    let session = match crate::api::receiver::current_service_endpoint() {
        Some(endpoint) => SendSession::with_endpoint(draft, destination, endpoint),
        None => SendSession::new(draft, destination),
    };
    // Claim this send's place in line before spawning: the counter has to move
    // synchronously with the call, or two rapid starts could both believe they
    // are the newest.
    let generation = SEND_GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    // Claim the slot *now*, so a cancel arriving before the session starts has
    // somewhere to land.
    let previous = ACTIVE_SEND.lock().ok().and_then(|mut guard| {
        guard.replace(ActiveSend {
            generation,
            handle: None,
            cancel_requested: false,
        })
    });
    let previous_handle = previous.and_then(|previous| previous.handle);

    RUNTIME.spawn(async move {
        // Tell the outgoing send to stop, then wait for the permit — which it
        // only releases once it has actually wound down. Cancelling without
        // waiting is what allowed two sessions to hold the same descriptors.
        if let Some(previous) = previous_handle {
            let _ = previous.cancel_transfer().await;
        }
        let Ok(_slot) = SEND_SLOT.acquire().await else {
            return;
        };

        // Someone tapped Send again while we were queued. Their session is the
        // one the user is looking at; run theirs, not ours.
        if SEND_GENERATION.load(Ordering::SeqCst) != generation {
            let _ = updates.add(terminal_event_for_app_error(
                fallback_destination,
                AppError::Cancelled {
                    reason: "replaced by a newer send".to_owned(),
                },
            ));
            return;
        }

        let run = session.start();
        let handle = run.cancel_handle();
        // Publish the handle and pick up any cancel that beat us to it.
        let cancel_immediately = match ACTIVE_SEND.lock() {
            Ok(mut guard) => match guard.as_mut() {
                Some(active) if active.generation == generation => {
                    active.handle = Some(handle.clone());
                    active.cancel_requested
                }
                // Superseded while we were starting: stop rather than run a
                // transfer the user has already replaced.
                _ => true,
            },
            Err(_) => false,
        };
        if cancel_immediately {
            let _ = handle.cancel_transfer().await;
        }

        let (mut events, outcome_rx) = run.into_parts();

        let event_updates = updates.clone();
        tokio::spawn(async move {
            while let Some(event) = events.next().await {
                let _ = event_updates.add(map_event(event));
            }
        });

        let outcome = outcome_rx.await;

        // Only clear our own entry — a newer send may already own the slot.
        if let Ok(mut guard) = ACTIVE_SEND.lock() {
            if guard
                .as_ref()
                .is_some_and(|active| active.generation == generation)
            {
                guard.take();
            }
        }

        match outcome {
            Ok(Ok(SendSessionOutcome::Accepted { .. })) => {}
            Ok(Ok(SendSessionOutcome::Declined { .. })) => {}
            // The session already emitted a fully-classified Failed event; don't
            // synthesize a second terminal event that would overwrite it.
            Ok(Ok(SendSessionOutcome::Failed)) => {}
            Ok(Err(error)) => {
                let _ = updates.add(terminal_event_for_app_error(fallback_destination, error));
            }
            Err(error) => {
                let _ = updates.add(terminal_internal_failure_event(
                    fallback_destination,
                    format!("Waiting for send outcome failed: {error}"),
                ));
            }
        }
    });

    Ok(())
}

pub fn cancel_active_send_transfer() -> Result<(), crate::api::error::UserFacingErrorData> {
    let mut guard = ACTIVE_SEND.lock().map_err(|_| {
        internal_user_facing_error(
            "Couldn't cancel send — internal state corrupted",
            "The send-cancel handle mutex was poisoned by a panic in another task. \
             Restart Wisp to recover.",
        )
    })?;
    let Some(active) = guard.as_mut() else {
        return Err(internal_user_facing_error(
            "Nothing to cancel — no active send",
            "The send finished or was already cancelled before the cancel reached the runtime.",
        ));
    };
    let Some(cancel_handle) = active.handle.clone() else {
        // The session is claimed but has not started yet — it is queued behind
        // the outgoing one. Record the request so it cancels itself the moment
        // it has a handle. Reporting "nothing to cancel" here is what silently
        // dropped the cancel and let the send carry on to accept the transfer.
        active.cancel_requested = true;
        return Ok(());
    };
    drop(guard);

    RUNTIME
        .block_on(cancel_handle.cancel_transfer())
        .map_err(|error| match error {
            AppError::NoActiveTransfer => internal_user_facing_error(
                "Nothing to cancel — no active send",
                "The send finished before the cancel reached the runtime.",
            ),
            _ => internal_user_facing_error(
                "Couldn't cancel send",
                format!("The send transfer rejected the cancel: {error}"),
            ),
        })
}

fn terminal_event_for_app_error(destination_label: String, error: AppError) -> SendTransferEvent {
    // Map AppError variants to user-facing titles that name WHERE in the send
    // pipeline things broke, so the user can tell e.g. "I never reached the
    // receiver" from "the receiver disconnected mid-transfer" without having
    // to parse the technical message.
    let (phase, status_message, title): (SendTransferPhase, &str, &str) = match &error {
        AppError::Cancelled { .. } => (
            SendTransferPhase::Cancelled,
            "Transfer cancelled.",
            "Send cancelled",
        ),
        AppError::InvalidCode { .. } => (
            SendTransferPhase::Failed,
            "Pairing code rejected by rendezvous server.",
            "Invalid pairing code",
        ),
        AppError::DiscoveryFailed => (
            SendTransferPhase::Failed,
            "Couldn't claim the pairing code from the rendezvous server.",
            "Pairing code not found or expired",
        ),
        AppError::BindingFailed { .. } => (
            SendTransferPhase::Failed,
            "Couldn't open a network port for the send.",
            "Network port unavailable",
        ),
        AppError::ActorStopped { .. } | AppError::ActorDroppedReply { .. } => (
            SendTransferPhase::Failed,
            "Send runtime stopped before the transfer completed.",
            "Send runtime stopped",
        ),
        AppError::ReceiverUnavailable { .. } => (
            SendTransferPhase::Failed,
            "Receiver side became unavailable during the send.",
            "Receiver disconnected",
        ),
        AppError::Internal { .. } => (
            SendTransferPhase::Failed,
            "Internal send error — see message for details.",
            "Send failed (internal)",
        ),
        _ => (
            SendTransferPhase::Failed,
            "Send failed before completion.",
            "Send failed",
        ),
    };

    SendTransferEvent {
        phase,
        destination_label,
        status_message: status_message.to_owned(),
        item_count: 0,
        total_size: 0,
        bytes_sent: 0,
        plan: None,
        snapshot: None,
        remote_device_type: None,
        remote_endpoint_id: None,
        remote_ephemeral: None,
        remote_ticket: None,
        bytes_hashed: None,
        connection_path: None,
        connection_candidates: Vec::new(),
        error: Some(internal_user_facing_error(title, error.to_string())),
    }
}

fn terminal_internal_failure_event(destination_label: String, detail: String) -> SendTransferEvent {
    SendTransferEvent {
        phase: SendTransferPhase::Failed,
        destination_label,
        status_message: "Send runtime crashed before reporting an outcome.".to_owned(),
        item_count: 0,
        total_size: 0,
        bytes_sent: 0,
        plan: None,
        snapshot: None,
        remote_device_type: None,
        remote_endpoint_id: None,
        remote_ephemeral: None,
        remote_ticket: None,
        bytes_hashed: None,
        connection_path: None,
        connection_candidates: Vec::new(),
        error: Some(internal_user_facing_error("Send runtime crashed", detail)),
    }
}

fn fallback_destination_label(request: &SendTransferRequest) -> String {
    request
        .lan_destination_label
        .as_deref()
        .map(str::trim)
        .filter(|label| !label.is_empty())
        .map(str::to_owned)
        .or_else(|| format_code_label(&request.code))
        .unwrap_or_else(|| "Nearby receiver".to_owned())
}

fn format_code_label(code: &str) -> Option<String> {
    let normalized = code.trim().to_ascii_uppercase();
    if normalized.len() == 6 {
        Some(format!("Code {} {}", &normalized[..3], &normalized[3..]))
    } else if normalized.is_empty() {
        None
    } else {
        Some(format!("Code {normalized}"))
    }
}

fn map_event(event: AppSendEvent) -> SendTransferEvent {
    SendTransferEvent {
        phase: match event.phase {
            AppSendPhase::Preparing => SendTransferPhase::Preparing,
            AppSendPhase::Connecting => SendTransferPhase::Connecting,
            AppSendPhase::WaitingForDecision => SendTransferPhase::WaitingForDecision,
            AppSendPhase::Accepted => SendTransferPhase::Accepted,
            AppSendPhase::Declined => SendTransferPhase::Declined,
            AppSendPhase::Sending => SendTransferPhase::Sending,
            AppSendPhase::Completed => SendTransferPhase::Completed,
            AppSendPhase::Cancelled => SendTransferPhase::Cancelled,
            AppSendPhase::Failed => SendTransferPhase::Failed,
        },
        destination_label: event.destination_label,
        status_message: event.status_message,
        item_count: event.item_count,
        total_size: event.total_size,
        bytes_sent: event.bytes_sent,
        plan: event.plan.map(map_plan),
        snapshot: event.snapshot.map(map_snapshot),
        remote_device_type: event.remote_device_type,
        remote_endpoint_id: event.remote_endpoint_id,
        remote_ephemeral: event.remote_ephemeral,
        remote_ticket: event.remote_ticket,
        bytes_hashed: event.bytes_hashed,
        connection_path: event.connection_path.map(map_connection_path),
        connection_candidates: event
            .connection_candidates
            .into_iter()
            .map(map_connection_candidate)
            .collect(),
        error: map_optional_user_facing_error(event.error),
    }
}

fn map_connection_path(path: AppConnectionPath) -> SendConnectionPath {
    SendConnectionPath {
        kind: path.label().to_owned(),
        relay_url: path.relay_url,
        direct_addr: path.direct_addr,
    }
}

fn map_connection_candidate(candidate: AppCandidatePath) -> SendConnectionCandidate {
    // Reuse the `"p2p"`/`"relay"` labels that `ConnectionPath::label()` emits so
    // the Dart side parses candidate kinds through the same switch as the
    // connection-path badge. Candidates are only ever Direct or Relay.
    let kind = match candidate.kind {
        AppConnectionPathKind::Direct => "p2p",
        AppConnectionPathKind::Relay => "relay",
        AppConnectionPathKind::Unknown => "unknown",
    };
    SendConnectionCandidate {
        addr: candidate.addr,
        kind: kind.to_owned(),
        active: candidate.active,
    }
}

fn map_snapshot(snapshot: TransferSnapshot) -> TransferSnapshotData {
    TransferSnapshotData {
        session_id: snapshot.session_id,
        phase: map_phase(snapshot.phase),
        total_files: snapshot.total_files,
        completed_files: snapshot.completed_files,
        total_bytes: snapshot.total_bytes,
        bytes_transferred: snapshot.bytes_transferred,
        active_file_id: snapshot.active_file_id,
        active_file_bytes: snapshot.active_file_bytes,
        bytes_per_sec: snapshot.bytes_per_sec,
        eta_seconds: snapshot.eta_seconds,
    }
}

fn map_phase(phase: TransferPhase) -> TransferPhaseData {
    match phase {
        TransferPhase::Connecting => TransferPhaseData::Connecting,
        TransferPhase::AwaitingAcceptance => TransferPhaseData::AwaitingAcceptance,
        TransferPhase::Transferring => TransferPhaseData::Transferring,
        TransferPhase::Finalizing => TransferPhaseData::Finalizing,
        TransferPhase::Completed => TransferPhaseData::Completed,
        TransferPhase::Cancelled => TransferPhaseData::Cancelled,
        TransferPhase::Failed => TransferPhaseData::Failed,
    }
}

#[cfg(test)]
mod tests {
    use wisp_app::AppError;
    use wisp_app::{ConnectionPath, ConnectionPathKind, SendEvent as AppSendEvent, SendPhase};

    use super::{
        map_event, terminal_event_for_app_error, terminal_internal_failure_event, SendTransferPhase,
    };

    #[test]
    fn cancelled_app_error_maps_to_cancelled_terminal_event() {
        let event = terminal_event_for_app_error(
            "Code ABC 123".to_owned(),
            AppError::Cancelled {
                reason: "user requested cancel".to_owned(),
            },
        );

        assert_eq!(event.phase, SendTransferPhase::Cancelled);
        assert_eq!(event.status_message, "Transfer cancelled.");
        assert!(event.error.is_some());
    }

    #[test]
    fn internal_failure_terminal_event_is_failed_phase() {
        let event = terminal_internal_failure_event(
            "Code ABC 123".to_owned(),
            "outcome channel closed".to_owned(),
        );

        assert_eq!(event.phase, SendTransferPhase::Failed);
        assert_eq!(
            event.status_message,
            "Send runtime crashed before reporting an outcome."
        );
        assert!(event.error.is_some());
    }

    fn base_app_send_event(connection_path: Option<ConnectionPath>) -> AppSendEvent {
        AppSendEvent {
            phase: SendPhase::Sending,
            destination_label: "Receiver".to_owned(),
            status_message: "Sending".to_owned(),
            item_count: 1,
            total_size: 100,
            bytes_sent: 50,
            plan: None,
            snapshot: None,
            remote_device_type: None,
            remote_endpoint_id: None,
            remote_ephemeral: None,
            remote_ticket: None,
            connection_candidates: Vec::new(),
            connection_path,
            error: None,
        }
    }

    #[test]
    fn map_event_propagates_direct_connection_path() {
        let event = base_app_send_event(Some(ConnectionPath {
            kind: ConnectionPathKind::Direct,
            relay_url: None,
            direct_addr: Some("192.168.1.5:5000".to_owned()),
        }));
        let mapped = map_event(event);
        let path = mapped.connection_path.expect("connection_path present");
        assert_eq!(path.kind, "p2p");
        assert!(path.relay_url.is_none());
        assert_eq!(path.direct_addr.as_deref(), Some("192.168.1.5:5000"));
    }

    #[test]
    fn map_event_propagates_relay_connection_path_with_url() {
        let event = base_app_send_event(Some(ConnectionPath {
            kind: ConnectionPathKind::Relay,
            relay_url: Some("https://relay.example/".to_owned()),
            direct_addr: None,
        }));
        let mapped = map_event(event);
        let path = mapped.connection_path.expect("connection_path present");
        assert_eq!(path.kind, "relay");
        assert_eq!(path.relay_url.as_deref(), Some("https://relay.example/"));
        assert!(path.direct_addr.is_none());
    }

    #[test]
    fn map_event_passes_through_none_connection_path() {
        let event = base_app_send_event(None);
        let mapped = map_event(event);
        assert!(mapped.connection_path.is_none());
    }
}
