use std::sync::Arc;
use std::time::Duration;

use drift_core::protocol::{Identity, TransferRole};
use drift_core::transfer::{
    SendRequest, Sender, SenderEvent as CoreSenderEvent, TransferOutcome as CoreTransferOutcome,
    TransferPlan,
};
use drift_core::util::snapshot_connection_path;
use iroh::{Endpoint, RelayMode, endpoint::presets};
use tokio::sync::{Mutex, mpsc, oneshot, watch};
use tokio::task::JoinHandle;
use tokio_stream::StreamExt;
use tokio_stream::wrappers::UnboundedReceiverStream;

use crate::error::{AppError, AppResult, UserFacingError, UserFacingErrorKind};
use crate::types::{ConnectionPath, SendEvent, SendPhase};

/// Interval between connection-path re-snapshots while a send session is active.
const CONNECTION_PATH_POLL_INTERVAL: Duration = Duration::from_millis(1500);

type StdMutex<T> = std::sync::Mutex<T>;

use super::destination::SendDestination;
use super::destination::{
    device_type_label, display_destination_label, is_receiver_decline_cancel, parse_device_type,
};
use super::draft::SendDraft;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SendSessionOutcome {
    Accepted {
        receiver_device_name: String,
        receiver_endpoint_id: iroh::EndpointId,
    },
    Declined {
        reason: String,
    },
}

#[derive(Debug)]
pub struct SendRun {
    pub events: SendEventStream,
    cancel_tx: Arc<Mutex<Option<watch::Sender<bool>>>>,
    outcome_rx: oneshot::Receiver<AppResult<SendSessionOutcome>>,
}

#[derive(Debug, Clone)]
pub struct SendCancelHandle {
    cancel_tx: Arc<Mutex<Option<watch::Sender<bool>>>>,
}

pub type SendEventStream = UnboundedReceiverStream<SendEvent>;

#[derive(Debug, Clone)]
pub struct SendSession {
    draft: SendDraft,
    destination: SendDestination,
    /// Endpoint to use for the outbound transfer.
    ///
    /// `Some(_)` → reuse the caller's already-bound endpoint (shared with
    /// the receiver service so we don't double-register on the relay with
    /// the same EndpointId).
    ///
    /// `None` → bind a fresh endpoint inside `drive()` using the persistent
    /// app identity.  Kept as a fallback for tests / one-off senders that
    /// don't have a long-lived endpoint to share.
    endpoint: Option<Endpoint>,
}

impl SendSession {
    pub fn new(draft: SendDraft, destination: SendDestination) -> Self {
        Self {
            draft,
            destination,
            endpoint: None,
        }
    }

    /// Same as [`new`] but reuses the supplied iroh endpoint instead of
    /// binding a new one.  Pass the receiver service's endpoint so the
    /// app holds at most one iroh instance per process (avoids relay
    /// "duplicate endpoint id" errors).
    pub fn with_endpoint(
        draft: SendDraft,
        destination: SendDestination,
        endpoint: Endpoint,
    ) -> Self {
        Self {
            draft,
            destination,
            endpoint: Some(endpoint),
        }
    }

    pub fn draft(&self) -> &SendDraft {
        &self.draft
    }

    pub fn destination(&self) -> &SendDestination {
        &self.destination
    }

    pub fn start(self) -> SendRun {
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let (outcome_tx, outcome_rx) = oneshot::channel();
        let cancel_tx = Arc::new(Mutex::new(None));
        let cancel_tx_for_task = Arc::clone(&cancel_tx);

        tokio::spawn(async move {
            let outcome = self.drive(event_tx, cancel_tx_for_task).await;
            let _ = outcome_tx.send(outcome);
        });

        SendRun {
            events: UnboundedReceiverStream::new(event_rx),
            cancel_tx,
            outcome_rx,
        }
    }

    async fn drive(
        self,
        event_tx: mpsc::UnboundedSender<SendEvent>,
        cancel_tx_slot: Arc<Mutex<Option<watch::Sender<bool>>>>,
    ) -> AppResult<SendSessionOutcome> {
        let preview = self.draft.inspect()?;
        let mut destination_label = self.destination.display_label();

        emit_send_event(
            &event_tx,
            SendEvent {
                phase: SendPhase::Connecting,
                destination_label: destination_label.clone(),
                status_message: "Request sent".to_owned(),
                item_count: preview.file_count,
                total_size: preview.total_size,
                bytes_sent: 0,
                plan: None,
                snapshot: None,
                remote_device_type: None,
                remote_endpoint_id: None,
                remote_ticket: None,
                connection_path: None,
                error: None,
            },
        );

        let resolved = match self.destination.resolve().await {
            Ok(resolved) => resolved,
            Err(error) => {
                emit_send_event(
                    &event_tx,
                    failed_event_from_error(&destination_label, error.clone().into()),
                );
                return Err(error);
            }
        };
        destination_label = resolved.destination_label;

        let device_type = parse_device_type(&self.draft.config().device_type)?;
        // Two paths for the iroh endpoint we send over:
        //
        // - **Shared (preferred):** the caller (the Flutter bridge in
        //   production) passes the receiver service's endpoint via
        //   `with_endpoint`.  Reusing that endpoint means we register on
        //   the relay only once per process, so the receiver service and
        //   the active sender don't fight for the same `EndpointId` /
        //   relay slot.  Blob serving runs against the shared endpoint's
        //   `Router` through `BlobDispatcher::global()`.
        //
        // - **Fallback (tests, CLI):** if no shared endpoint was supplied
        //   we bind our own.  Blob serving then uses an internal Router
        //   spawned by `BlobService`.  This path is fine when nothing else
        //   in the process is bound to the same secret key, e.g. unit
        //   tests with a fresh key.
        let (endpoint, blob_strategy) = if let Some(shared) = self.endpoint.clone() {
            tracing::info!(
                target: "drift_app::send::session",
                endpoint_id = %shared.addr().id,
                "using shared receiver-service endpoint + BlobDispatcher (External strategy)"
            );
            (
                shared,
                drift_core::blobs::BlobServingStrategy::External(Arc::new(
                    crate::blob_dispatcher::BlobDispatcher::global(),
                )),
            )
        } else {
            tracing::info!(
                target: "drift_app::send::session",
                "no shared endpoint available; binding private sender endpoint (Internal strategy)"
            );
            let endpoint = Endpoint::builder(presets::N0)
                .alpns(vec![
                    drift_core::protocol::ALPN.to_vec(),
                    iroh_blobs::ALPN.to_vec(),
                ])
                .relay_mode(RelayMode::Default)
                .transport_config(crate::quic_keepalive::build_transport_config())
                .secret_key(crate::identity::current_secret_key())
                .bind()
                .await
                .map_err(|e| AppError::BindingFailed {
                    context: format!("sender endpoint: {e}"),
                })?;
            (endpoint, drift_core::blobs::BlobServingStrategy::Internal)
        };
        let identity = Identity {
            role: TransferRole::Sender,
            endpoint_id: endpoint.addr().id,
            device_name: self.draft.config().device_name.clone(),
            device_type,
        };
        let watcher_endpoint = endpoint.clone();
        let peer_endpoint_id = resolved.peer_endpoint_id;
        let initial_path = snapshot_connection_path(&watcher_endpoint, peer_endpoint_id).await;
        let current_path: Arc<StdMutex<ConnectionPath>> = Arc::new(StdMutex::new(initial_path));
        let last_event: Arc<StdMutex<Option<SendEvent>>> = Arc::new(StdMutex::new(None));
        // Re-encode the resolved peer addr so emitted events carry a ticket
        // Dart can persist as `lastTicket`.  Encoding errors fall back to
        // `None` — pkarr will still be able to find the peer via endpoint id,
        // we just lose the offline-LAN fast path on next reconnect.
        let remote_ticket: Option<String> =
            drift_core::util::encode_ticket(resolved.peer_endpoint_addr.clone()).ok();
        let sender = Sender::new(
            endpoint,
            identity,
            SendRequest {
                peer_endpoint_addr: resolved.peer_endpoint_addr.clone(),
                peer_endpoint_id: resolved.peer_endpoint_id,
                files: self.draft.paths().to_vec(),
            },
        )
        .with_blob_strategy(blob_strategy);

        let sender_run = sender.run_with_events();
        let (mut core_events, cancel_tx, outcome_rx) = sender_run.into_parts();
        {
            let mut slot = cancel_tx_slot.lock().await;
            *slot = Some(cancel_tx);
        }
        let mut current_label = destination_label.clone();
        let mut current_plan: Option<TransferPlan> = None;

        let (path_shutdown_tx, path_shutdown_rx) = oneshot::channel::<()>();
        let path_watcher = spawn_send_path_watcher(
            watcher_endpoint,
            peer_endpoint_id,
            event_tx.clone(),
            Arc::clone(&current_path),
            Arc::clone(&last_event),
            path_shutdown_rx,
        );

        while let Some(core_event) = core_events.next().await {
            let mut mapped =
                map_sender_event(&mut current_label, &preview, &mut current_plan, core_event);
            mapped.remote_ticket = remote_ticket.clone();
            let mapped = maybe_demote_pre_handshake_failure(&last_event, mapped);
            emit_send_event_stamped(&event_tx, &current_path, &last_event, mapped);
        }

        let _ = path_shutdown_tx.send(());
        let _ = path_watcher.await;

        let core_outcome = outcome_rx.await.map_err(|e| AppError::Internal {
            message: e.to_string(),
        })?;

        match core_outcome {
            Ok(CoreTransferOutcome::Completed) => Ok(SendSessionOutcome::Accepted {
                receiver_device_name: String::new(),
                receiver_endpoint_id: resolved.peer_endpoint_id,
            }),
            Ok(CoreTransferOutcome::Declined { reason }) => {
                Ok(SendSessionOutcome::Declined { reason })
            }
            Ok(CoreTransferOutcome::Cancelled(cancellation)) => {
                if is_receiver_decline_cancel(&cancellation) {
                    Ok(SendSessionOutcome::Declined {
                        reason: cancellation.reason,
                    })
                } else {
                    Err(AppError::Cancelled {
                        reason: cancellation.reason,
                    })
                }
            }
            Err(error) => {
                emit_send_event(
                    &event_tx,
                    failed_event_from_error(&current_label, error.into()),
                );
                Err(AppError::Internal {
                    message: "transfer failed".to_owned(),
                })
            }
        }
    }
}

impl SendRun {
    pub fn cancel_handle(&self) -> SendCancelHandle {
        SendCancelHandle {
            cancel_tx: Arc::clone(&self.cancel_tx),
        }
    }

    pub fn into_parts(
        self,
    ) -> (
        SendEventStream,
        oneshot::Receiver<AppResult<SendSessionOutcome>>,
    ) {
        (self.events, self.outcome_rx)
    }

    pub async fn cancel_transfer(&self) -> AppResult<()> {
        self.cancel_handle().cancel_transfer().await
    }

    pub async fn outcome(self) -> AppResult<SendSessionOutcome> {
        self.outcome_rx.await.map_err(|_| AppError::Internal {
            message: "waiting for send outcome".to_owned(),
        })?
    }
}

impl SendCancelHandle {
    pub async fn cancel_transfer(&self) -> AppResult<()> {
        let guard = self.cancel_tx.lock().await;
        match guard.as_ref() {
            Some(cancel_tx) => {
                let _ = cancel_tx.send(true);
                Ok(())
            }
            None => Err(AppError::NoActiveTransfer),
        }
    }
}

pub(crate) fn emit_send_event(event_tx: &mpsc::UnboundedSender<SendEvent>, event: SendEvent) {
    let _ = event_tx.send(event);
}

fn emit_send_event_stamped(
    event_tx: &mpsc::UnboundedSender<SendEvent>,
    current_path: &Arc<StdMutex<ConnectionPath>>,
    last_event: &Arc<StdMutex<Option<SendEvent>>>,
    mut event: SendEvent,
) {
    event.connection_path = Some(current_path.lock().unwrap().clone());
    *last_event.lock().unwrap() = Some(event.clone());
    let _ = event_tx.send(event);
}

/// When the sender hits a Failed event before any Hello has been exchanged
/// (i.e., we never advanced past `Connecting`), the most likely cause is the
/// peer being offline or not in receive mode — not a generic "connection
/// lost" mid-transfer. Re-classify so the user sees an actionable hint.
///
/// We pick between two kinds based on whether iroh ever observed an active
/// connection path before the failure:
///   - never observed any active path → [`UserFacingErrorKind::PeerUnreachable`]
///     (peer probably offline / out of range).
///   - observed Direct or Relay briefly, then dial failed → [`UserFacingErrorKind::PeerNotReceiving`]
///     (peer is on the network but isn't accepting transfers).
///
/// This is a heuristic — iroh's `ConnectError` doesn't cleanly distinguish
/// the two cases at the source, but the connection-path watcher's record is
/// a useful proxy: if we never saw the peer light up an active path, we
/// almost certainly never reached them.
fn maybe_demote_pre_handshake_failure(
    last_event: &Arc<StdMutex<Option<SendEvent>>>,
    mut event: SendEvent,
) -> SendEvent {
    if !matches!(event.phase, SendPhase::Failed) {
        return event;
    }
    let prior = last_event.lock().unwrap().clone();
    let prior_phase = prior.as_ref().map(|e| e.phase);
    let pre_handshake = matches!(prior_phase, None | Some(SendPhase::Connecting));
    if !pre_handshake {
        return event;
    }

    let observed_active_path = prior
        .as_ref()
        .and_then(|e| e.connection_path.as_ref())
        .is_some_and(|path| {
            matches!(
                path.kind,
                drift_core::util::ConnectionPathKind::Direct
                    | drift_core::util::ConnectionPathKind::Relay
            )
        });
    let kind = if observed_active_path {
        UserFacingErrorKind::PeerNotReceiving
    } else {
        UserFacingErrorKind::PeerUnreachable
    };
    event.error = Some(UserFacingError::from_kind(kind));
    event
}

fn spawn_send_path_watcher(
    endpoint: Endpoint,
    peer_endpoint_id: iroh::EndpointId,
    event_tx: mpsc::UnboundedSender<SendEvent>,
    current_path: Arc<StdMutex<ConnectionPath>>,
    last_event: Arc<StdMutex<Option<SendEvent>>>,
    mut shutdown_rx: oneshot::Receiver<()>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(CONNECTION_PATH_POLL_INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        // Skip the first immediate tick: the initial path was already snapshotted
        // by the caller before this watcher was spawned.
        interval.tick().await;
        loop {
            tokio::select! {
                _ = &mut shutdown_rx => break,
                _ = interval.tick() => {
                    let snapshot = snapshot_connection_path(&endpoint, peer_endpoint_id).await;
                    let changed = {
                        let mut guard = current_path.lock().unwrap();
                        // Don't downgrade a known Direct/Relay path to Unknown — that
                        // happens when iroh momentarily has no Active address (NAT
                        // rebind, path migration), and would cause the UI badge to
                        // flicker hide/show every poll.
                        let downgrade_to_unknown = matches!(
                            snapshot.kind,
                            drift_core::util::ConnectionPathKind::Unknown
                        ) && !matches!(
                            guard.kind,
                            drift_core::util::ConnectionPathKind::Unknown
                        );
                        if *guard != snapshot && !downgrade_to_unknown {
                            *guard = snapshot.clone();
                            true
                        } else {
                            false
                        }
                    };
                    if !changed {
                        continue;
                    }
                    let last = last_event.lock().unwrap().clone();
                    if let Some(mut event) = last {
                        event.connection_path = Some(snapshot);
                        *last_event.lock().unwrap() = Some(event.clone());
                        if event_tx.send(event).is_err() {
                            break;
                        }
                    }
                }
            }
        }
    })
}

pub(crate) fn failed_event_from_error(
    destination_label: &str,
    error: UserFacingError,
) -> SendEvent {
    SendEvent {
        phase: SendPhase::Failed,
        destination_label: destination_label.to_owned(),
        status_message: format!("Starting transfer to {destination_label}."),
        item_count: 0,
        total_size: 0,
        bytes_sent: 0,
        plan: None,
        snapshot: None,
        remote_device_type: None,
        remote_endpoint_id: None,
        remote_ticket: None,
        connection_path: None,
        error: Some(error),
    }
}

fn map_sender_event(
    current_label: &mut String,
    preview: &crate::types::SelectionPreview,
    current_plan: &mut Option<TransferPlan>,
    event: CoreSenderEvent,
) -> SendEvent {
    match event {
        CoreSenderEvent::Connecting { prepared_plan, .. } => SendEvent {
            phase: SendPhase::Connecting,
            destination_label: current_label.clone(),
            status_message: "Request sent".to_owned(),
            item_count: preview.file_count,
            total_size: preview.total_size,
            bytes_sent: 0,
            plan: Some(prepared_plan),
            snapshot: None,
            remote_device_type: None,
            remote_endpoint_id: None,
            remote_ticket: None,
            connection_path: None,
            error: None,
        },
        CoreSenderEvent::WaitingForDecision {
            receiver_device_name,
            receiver_device_type,
            receiver_endpoint_id,
            prepared_plan,
            ..
        } => {
            *current_label = display_destination_label(&receiver_device_name);
            SendEvent {
                phase: SendPhase::WaitingForDecision,
                destination_label: current_label.clone(),
                status_message: "Waiting for confirmation.".to_owned(),
                item_count: preview.file_count,
                total_size: preview.total_size,
                bytes_sent: 0,
                plan: Some(prepared_plan),
                snapshot: None,
                remote_device_type: Some(device_type_label(receiver_device_type)),
                remote_endpoint_id: Some(receiver_endpoint_id.to_string()),
                remote_ticket: None,
                connection_path: None,
                error: None,
            }
        }
        CoreSenderEvent::Accepted {
            receiver_device_name,
            receiver_device_type,
            receiver_endpoint_id,
            prepared_plan,
            ..
        } => {
            *current_label = display_destination_label(&receiver_device_name);
            SendEvent {
                phase: SendPhase::Accepted,
                destination_label: current_label.clone(),
                status_message: format!("Receiver {receiver_device_name} confirmed."),
                item_count: preview.file_count,
                total_size: preview.total_size,
                bytes_sent: 0,
                plan: Some(prepared_plan),
                snapshot: None,
                remote_device_type: Some(device_type_label(receiver_device_type)),
                remote_endpoint_id: Some(receiver_endpoint_id.to_string()),
                remote_ticket: None,
                connection_path: None,
                error: None,
            }
        }
        CoreSenderEvent::Declined {
            reason,
            prepared_plan,
            ..
        } => SendEvent {
            phase: SendPhase::Declined,
            destination_label: current_label.clone(),
            status_message: "Transfer declined.".to_owned(),
            item_count: preview.file_count,
            total_size: preview.total_size,
            bytes_sent: 0,
            plan: Some(prepared_plan),
            snapshot: None,
            remote_device_type: None,
            remote_endpoint_id: None,
            remote_ticket: None,
            connection_path: None,
            error: Some(UserFacingError::new(
                UserFacingErrorKind::PeerDeclined,
                "Transfer declined",
                reason,
            )),
        },
        CoreSenderEvent::Failed {
            error,
            prepared_plan,
            ..
        } => SendEvent {
            phase: SendPhase::Failed,
            destination_label: current_label.clone(),
            status_message: format!("Starting transfer to {current_label}."),
            item_count: preview.file_count,
            total_size: preview.total_size,
            bytes_sent: 0,
            plan: Some(prepared_plan),
            snapshot: None,
            remote_device_type: None,
            remote_endpoint_id: None,
            remote_ticket: None,
            connection_path: None,
            error: Some(UserFacingError::from(error)),
        },
        CoreSenderEvent::TransferStarted { plan, .. } => {
            *current_plan = Some(plan.clone());
            SendEvent {
                phase: SendPhase::Sending,
                destination_label: current_label.clone(),
                status_message: format!("Sending to {current_label}."),
                item_count: u64::from(plan.total_files),
                total_size: plan.total_bytes,
                bytes_sent: 0,
                plan: Some(plan.clone()),
                snapshot: None,
                remote_device_type: None,
                remote_endpoint_id: None,
                remote_ticket: None,
                connection_path: None,
                error: None,
            }
        }
        CoreSenderEvent::TransferProgress { snapshot, .. } => SendEvent {
            phase: SendPhase::Sending,
            destination_label: current_label.clone(),
            status_message: "Sending to ".to_owned() + &current_label,
            item_count: current_plan
                .as_ref()
                .map(|plan| u64::from(plan.total_files))
                .unwrap_or(u64::from(snapshot.total_files)),
            total_size: current_plan
                .as_ref()
                .map(|plan| plan.total_bytes)
                .unwrap_or(snapshot.total_bytes),
            bytes_sent: snapshot.bytes_transferred,
            plan: current_plan.clone(),
            snapshot: Some(snapshot.clone()),
            remote_device_type: None,
            remote_endpoint_id: None,
            remote_ticket: None,
            connection_path: None,
            error: None,
        },
        CoreSenderEvent::TransferCompleted { snapshot, .. } => SendEvent {
            phase: SendPhase::Completed,
            destination_label: current_label.clone(),
            status_message: "Files sent successfully".to_owned(),
            item_count: current_plan
                .as_ref()
                .map(|plan| u64::from(plan.total_files))
                .unwrap_or(u64::from(snapshot.total_files)),
            total_size: current_plan
                .as_ref()
                .map(|plan| plan.total_bytes)
                .unwrap_or(snapshot.total_bytes),
            bytes_sent: snapshot.bytes_transferred,
            plan: current_plan.clone(),
            snapshot: Some(snapshot.clone()),
            remote_device_type: None,
            remote_endpoint_id: None,
            remote_ticket: None,
            connection_path: None,
            error: None,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::{
        SendRun, failed_event_from_error, is_receiver_decline_cancel,
        maybe_demote_pre_handshake_failure,
    };
    use crate::error::{AppError, UserFacingErrorKind};
    use crate::types::{SendEvent, SendPhase};
    use drift_core::protocol::{CancelPhase, TransferRole};
    use drift_core::transfer::TransferCancellation;
    use std::sync::{Arc, Mutex as StdMutex};
    use tokio::sync::{Mutex, mpsc, oneshot, watch};
    use tokio_stream::wrappers::UnboundedReceiverStream;

    fn synthetic_event(phase: SendPhase) -> SendEvent {
        SendEvent {
            phase,
            destination_label: "Receiver".to_owned(),
            status_message: String::new(),
            item_count: 0,
            total_size: 0,
            bytes_sent: 0,
            plan: None,
            snapshot: None,
            remote_device_type: None,
            remote_endpoint_id: None,
            remote_ticket: None,
            connection_path: None,
            error: None,
        }
    }

    fn synthetic_event_with_path(
        phase: SendPhase,
        path: drift_core::util::ConnectionPath,
    ) -> SendEvent {
        let mut e = synthetic_event(phase);
        e.connection_path = Some(path);
        e
    }

    #[test]
    fn demote_no_prior_event_yields_unreachable() {
        // Never observed any path → peer is probably offline.
        let last = Arc::new(StdMutex::new(None));
        let event = maybe_demote_pre_handshake_failure(&last, synthetic_event(SendPhase::Failed));
        let kind = event.error.expect("error").kind();
        assert_eq!(kind, UserFacingErrorKind::PeerUnreachable);
    }

    #[test]
    fn demote_connecting_with_unknown_path_yields_unreachable() {
        // Connecting phase reached but the watcher never saw any active path.
        let prior = synthetic_event_with_path(SendPhase::Connecting, drift_core::util::ConnectionPath::unknown());
        let last = Arc::new(StdMutex::new(Some(prior)));
        let event = maybe_demote_pre_handshake_failure(&last, synthetic_event(SendPhase::Failed));
        let kind = event.error.expect("error").kind();
        assert_eq!(kind, UserFacingErrorKind::PeerUnreachable);
    }

    #[test]
    fn demote_connecting_with_relay_path_yields_not_receiving() {
        // We did connect (relay path observed) but never made it to handshake.
        let prior = synthetic_event_with_path(
            SendPhase::Connecting,
            drift_core::util::ConnectionPath {
                kind: drift_core::util::ConnectionPathKind::Relay,
                relay_url: Some("https://relay.example/".to_owned()),
                direct_addr: None,
            },
        );
        let last = Arc::new(StdMutex::new(Some(prior)));
        let event = maybe_demote_pre_handshake_failure(&last, synthetic_event(SendPhase::Failed));
        let kind = event.error.expect("error").kind();
        assert_eq!(kind, UserFacingErrorKind::PeerNotReceiving);
    }

    #[test]
    fn demote_connecting_with_direct_path_yields_not_receiving() {
        let prior = synthetic_event_with_path(
            SendPhase::Connecting,
            drift_core::util::ConnectionPath {
                kind: drift_core::util::ConnectionPathKind::Direct,
                relay_url: None,
                direct_addr: Some("192.168.1.5:5000".to_owned()),
            },
        );
        let last = Arc::new(StdMutex::new(Some(prior)));
        let event = maybe_demote_pre_handshake_failure(&last, synthetic_event(SendPhase::Failed));
        let kind = event.error.expect("error").kind();
        assert_eq!(kind, UserFacingErrorKind::PeerNotReceiving);
    }

    #[test]
    fn keep_failed_classification_when_prior_reached_handshake() {
        let prior = synthetic_event(SendPhase::WaitingForDecision);
        let last = Arc::new(StdMutex::new(Some(prior)));
        let mut failed = synthetic_event(SendPhase::Failed);
        failed.error = Some(crate::error::UserFacingError::from_kind(
            UserFacingErrorKind::ConnectionLost,
        ));
        let event = maybe_demote_pre_handshake_failure(&last, failed);
        let kind = event.error.expect("error").kind();
        assert_eq!(kind, UserFacingErrorKind::ConnectionLost);
    }

    #[test]
    fn keep_failed_classification_when_prior_was_sending() {
        let last = Arc::new(StdMutex::new(Some(synthetic_event(SendPhase::Sending))));
        let mut failed = synthetic_event(SendPhase::Failed);
        failed.error = Some(crate::error::UserFacingError::from_kind(
            UserFacingErrorKind::ConnectionLost,
        ));
        let event = maybe_demote_pre_handshake_failure(&last, failed);
        let kind = event.error.expect("error").kind();
        assert_eq!(kind, UserFacingErrorKind::ConnectionLost);
    }

    #[test]
    fn passthrough_non_failed_events_unchanged() {
        let last = Arc::new(StdMutex::new(None));
        let event = maybe_demote_pre_handshake_failure(&last, synthetic_event(SendPhase::Sending));
        assert!(event.error.is_none(), "non-Failed events must not gain an error");
    }

    #[test]
    fn failed_event_uses_structured_error() {
        let error = AppError::Internal {
            message: "boom".to_owned(),
        };
        let event = failed_event_from_error("Remote", error.into());

        let error = event.error.expect("structured error");
        assert_eq!(error.kind(), UserFacingErrorKind::Internal);
        assert_eq!(error.title(), "Something went wrong");
        assert!(error.message().contains("boom"));
    }

    #[test]
    fn receiver_waiting_for_decision_cancel_is_treated_as_decline() {
        let cancellation = TransferCancellation {
            by: TransferRole::Receiver,
            phase: CancelPhase::WaitingForDecision,
            reason: "receiver cancelled before approval".to_owned(),
        };

        assert!(is_receiver_decline_cancel(&cancellation));
        let sender_cancel = TransferCancellation {
            by: TransferRole::Sender,
            phase: CancelPhase::WaitingForDecision,
            reason: "sender cancelled before approval".to_owned(),
        };
        assert!(!is_receiver_decline_cancel(&sender_cancel));
    }

    #[tokio::test]
    async fn send_run_cancel_transfer_signals_watch_channel() {
        let (_event_tx, event_rx) = mpsc::unbounded_channel();
        let (outcome_tx, outcome_rx) = oneshot::channel();
        let (cancel_tx, cancel_rx) = watch::channel(false);
        let run = SendRun {
            events: UnboundedReceiverStream::new(event_rx),
            cancel_tx: Arc::new(Mutex::new(Some(cancel_tx))),
            outcome_rx,
        };

        run.cancel_transfer().await.expect("cancel succeeds");

        assert!(*cancel_rx.borrow());
        drop(outcome_tx);
    }
}
