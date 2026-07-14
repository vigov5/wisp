use std::sync::{Arc, Mutex};
use std::time::Duration;

use iroh::Endpoint;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio_stream::StreamExt;
use wisp_core::protocol::DeviceType;
use wisp_core::transfer::{
    ReceiverDecision as CoreReceiverDecision, ReceiverEvent as CoreReceiverEvent,
    ReceiverRequest as CoreReceiverRequest, ReceiverSession as CoreReceiverSession,
    ReceiverStart as CoreReceiverStart, TransferOutcome as CoreTransferOutcome, TransferPhase,
    TransferPlan, TransferPlanFile, TransferSnapshot,
};
use wisp_core::util::{connection_path_from_info, human_size};

use crate::error::{UserFacingError, UserFacingErrorKind, format_error_chain};
use crate::types::{
    ConflictPolicy, ConnectionPath, ReceiverOfferEvent, ReceiverOfferFile, ReceiverOfferPhase,
};

use super::actor::ReceiverCommand;
use super::runtime::OfferResolution;

const PROGRESS_EVENT_MIN_INTERVAL: Duration = Duration::from_millis(100);
const PROGRESS_EVENT_MIN_BYTES: u64 = 4 * 1024 * 1024;
/// Interval between connection-path re-snapshots while a receiver session is active.
/// 1.5s balances UI latency vs `remote_info` lock churn on the iroh endpoint.
const CONNECTION_PATH_POLL_INTERVAL: Duration = Duration::from_millis(1500);

#[derive(Debug)]
pub(super) struct ReceiverSession {
    offer_id: u64,
    endpoint: Endpoint,
    connection: iroh::endpoint::Connection,
    out_dir: std::path::PathBuf,
    device_name: String,
    device_type: DeviceType,
    conflict_policy: ConflictPolicy,
    save_root_label: String,
    cmd_tx: mpsc::Sender<ReceiverCommand>,
}

#[derive(Debug)]
pub(super) struct ReceiverRun {
    pub(super) offer_id: u64,
    pub(super) decision_tx: oneshot::Sender<OfferResolution>,
    pub(super) cancel_tx: tokio::sync::watch::Sender<bool>,
}

impl ReceiverSession {
    pub(super) fn new(
        offer_id: u64,
        endpoint: Endpoint,
        connection: iroh::endpoint::Connection,
        out_dir: std::path::PathBuf,
        device_name: String,
        device_type: DeviceType,
        conflict_policy: ConflictPolicy,
        cmd_tx: mpsc::Sender<ReceiverCommand>,
    ) -> Self {
        let save_root_label = save_root_display(&out_dir);
        Self {
            offer_id,
            endpoint,
            connection,
            out_dir,
            device_name,
            device_type,
            conflict_policy,
            save_root_label,
            cmd_tx,
        }
    }

    pub(super) fn spawn(self) -> JoinHandle<()> {
        tokio::spawn(async move {
            self.run().await;
        })
    }

    async fn run(self) {
        let ReceiverSession {
            offer_id,
            endpoint,
            connection,
            out_dir,
            device_name,
            device_type,
            conflict_policy,
            save_root_label,
            cmd_tx,
        } = self;

        let remote_id = connection.remote_id();
        let remote_id_str = remote_id.to_string();
        // Weak handle to the live connection — used to read the *selected* path
        // (authoritative), not the endpoint address book (which can report a
        // stale direct candidate as Active). Captured before `session.start()`
        // consumes `connection`; it stays valid as long as the core session
        // keeps the connection alive.
        let conn_info = connection.to_info();
        let initial_path = connection_path_from_info(&conn_info);
        let current_path = Arc::new(Mutex::new(initial_path));
        // Kept past `session.start()` (which consumes `endpoint`) so we can look
        // up the peer's relay URL when synthesizing the send-back ticket, even
        // when the active path is direct. Without a relay in that ticket, a
        // later network change on the peer (Wi-Fi → 4G) leaves only an
        // unreachable direct IP and iroh has nothing to fall back to.
        let relay_lookup_endpoint = endpoint.clone();
        let session = CoreReceiverSession::new(build_core_receiver_request(
            device_name.clone(),
            device_type,
            out_dir,
            conflict_policy,
        ));
        let start = session.start(endpoint, connection);
        let CoreReceiverStart {
            mut events,
            mut offer_rx,
            outcome_rx,
            control,
        } = start;

        // While the Offer is in flight, drain the core event stream so we can
        // surface the sender the instant its Hello lands (`SenderConnected`):
        // the UI leaves the idle/QR screen for a "connecting from <X>" screen
        // instead of sitting silent. We also remember the sender's identity so
        // a pre-offer failure (offer never arrives / stalls) is attributable
        // — `failed_offer_event` with a non-empty sender_name surfaces in the
        // UI, whereas the old empty-name path was suppressed as a bogus
        // "unknown sender" card.
        let mut connecting_sender: Option<(String, DeviceType, bool, bool, String)> = None;
        let mut events_done = false;
        let offer = loop {
            // `biased`: drain pending events before resolving the offer so the
            // `SenderConnected` that precedes a (fast) offer still surfaces.
            tokio::select! {
                biased;
                maybe_event = events.next(), if !events_done => {
                    match maybe_event {
                        Some(CoreReceiverEvent::SenderConnected {
                            sender_device_name,
                            sender_device_type,
                            sender_web,
                            sender_ephemeral,
                            sender_endpoint_id,
                            ..
                        }) => {
                            let sender_label = display_sender_label(&sender_device_name);
                            let endpoint_id_str = sender_endpoint_id.to_string();
                            connecting_sender = Some((
                                sender_label.clone(),
                                sender_device_type,
                                sender_web,
                                sender_ephemeral,
                                endpoint_id_str.clone(),
                            ));
                            let _ = cmd_tx
                                .send(ReceiverCommand::OfferConnecting {
                                    event: connecting_offer_event(
                                        save_root_label.clone(),
                                        sender_label,
                                        sender_device_type,
                                        sender_web,
                                        sender_ephemeral,
                                        endpoint_id_str,
                                    ),
                                })
                                .await;
                        }
                        // Any other pre-offer event (e.g. Listening) is not
                        // relevant until the offer arrives; the post-offer loop
                        // below handles the rest.
                        Some(_) => {}
                        None => events_done = true,
                    }
                }
                offer_result = &mut offer_rx => {
                    match offer_result {
                        Ok(Ok(offer)) => break offer,
                        Ok(Err(error)) => {
                            send_pre_offer_failure(
                                &cmd_tx,
                                offer_id,
                                &save_root_label,
                                device_type,
                                connecting_sender.as_ref(),
                                UserFacingError::from(error),
                            )
                            .await;
                            return;
                        }
                        Err(error) => {
                            send_pre_offer_failure(
                                &cmd_tx,
                                offer_id,
                                &save_root_label,
                                device_type,
                                connecting_sender.as_ref(),
                                UserFacingError::internal(
                                    "Transfer failed",
                                    format!("{error}"),
                                ),
                            )
                            .await;
                            return;
                        }
                    }
                }
            }
        };

        let sender_label = display_sender_label(&offer.sender_device_name);
        let sender_device_type = offer.sender_device_type;
        let sender_web = offer.sender_web;
        let sender_ephemeral = offer.sender_ephemeral;
        let resume_from_bytes = offer.resume_from_bytes;
        let plan = match TransferPlan::try_new(
            offer.session_id.clone(),
            offer
                .items
                .iter()
                .enumerate()
                .map(|(index, file)| TransferPlanFile {
                    id: index as u32,
                    path: file.path.clone(),
                    size: file.size,
                })
                .collect(),
        ) {
            Ok(plan) => plan,
            Err(error) => {
                // Snapshot the connection path BEFORE we cross the await
                // below — the MutexGuard isn't Send so it can't be held
                // across an .await inside a tokio task.
                let path_snapshot = current_path.lock().unwrap().clone();
                let _ = cmd_tx
                    .send(ReceiverCommand::OfferFinished {
                        offer_id,
                        // Offer arrived but plan construction failed — we
                        // still know the file count + total size from the
                        // offer manifest, but the structured plan itself
                        // never materialised so leave it as None.
                        final_event: failed_offer_event(
                            save_root_label.clone(),
                            sender_label.clone(),
                            sender_device_type,
                            sender_web,
                            sender_ephemeral,
                            "Transfer failed.".to_owned(),
                            UserFacingError::internal(
                                "Transfer failed",
                                format_error_chain(&error),
                            ),
                            offer.file_count,
                            offer.total_size,
                            0,
                            None,
                            None,
                            Some(path_snapshot),
                            Some(remote_id_str.clone()),
                            None,
                            Vec::new(),
                        ),
                    })
                    .await;
                return;
            }
        };
        let files = offer
            .items
            .iter()
            .map(|file| ReceiverOfferFile {
                path: file.path.clone(),
                size: file.size,
            })
            .collect();

        let (decision_tx, decision_rx) = oneshot::channel();
        let core_decision_tx = control.decision_tx;
        tokio::spawn(async move {
            let decision = match decision_rx.await.unwrap_or(OfferResolution::Cancel) {
                OfferResolution::Accept => CoreReceiverDecision::Accept,
                OfferResolution::Decline | OfferResolution::Cancel => CoreReceiverDecision::Decline,
            };
            let _ = core_decision_tx.send(decision);
        });
        let run = ReceiverRun {
            offer_id,
            decision_tx,
            cancel_tx: control.cancel_tx,
        };
        let prepared_event = ReceiverOfferEvent {
            phase: ReceiverOfferPhase::OfferReady,
            sender_name: sender_label.clone(),
            sender_device_type: device_type_to_str(sender_device_type),
            sender_web,
            sender_ephemeral,
            destination_label: sender_label.clone(),
            save_root_label: save_root_label.clone(),
            status_message: format!("{sender_label} wants to send you files."),
            item_count: offer.file_count,
            total_size_bytes: offer.total_size,
            bytes_received: resume_from_bytes,
            plan: Some(plan.clone()),
            snapshot: None,
            connection_path: Some(current_path.lock().unwrap().clone()),
            sender_endpoint_id: Some(remote_id_str.clone()),
            sender_ticket: None,
            total_size_label: human_size(offer.total_size),
            files,
            inline_text: offer.inline_text.clone(),
            error: None,
        };
        tracing::info!(
            target: "wisp_app::receiver::session",
            offer_id,
            sender = %sender_label,
            file_count = offer.file_count,
            "dispatching OfferPrepared to actor"
        );
        if cmd_tx
            .send(ReceiverCommand::OfferPrepared {
                run,
                event: prepared_event,
            })
            .await
            .is_err()
        {
            tracing::warn!(
                target: "wisp_app::receiver::session",
                offer_id,
                "actor channel closed; offer event dropped"
            );
            return;
        }

        let (path_shutdown_tx, path_shutdown_rx) = oneshot::channel::<()>();
        let path_watcher = spawn_connection_path_watcher(
            conn_info,
            offer_id,
            cmd_tx.clone(),
            Arc::clone(&current_path),
            path_shutdown_rx,
        );

        let progress_cmd_tx = cmd_tx.clone();
        let mut last_progress_emit_at = std::time::Instant::now()
            .checked_sub(PROGRESS_EVENT_MIN_INTERVAL)
            .unwrap_or_else(std::time::Instant::now);
        let mut last_progress_bytes = resume_from_bytes;
        // Latest TransferSnapshot we observed.  drift#29: the Failed
        // path needs this so the UI can show how far the transfer got
        // (current bytes_transferred / file / phase) when it failed.
        let mut latest_snapshot: Option<TransferSnapshot> = None;
        while let Some(event) = events.next().await {
            match event {
                CoreReceiverEvent::TransferStarted {
                    session_id: _,
                    plan,
                } => {
                    let _ = progress_cmd_tx.try_send(ReceiverCommand::OfferProgress {
                        offer_id,
                        event: build_offer_event(
                            ReceiverOfferPhase::Receiving,
                            sender_label.clone(),
                            save_root_label.clone(),
                            sender_device_type,
                            sender_web,
                            sender_ephemeral,
                            Some(current_path.lock().unwrap().clone()),
                            Some(remote_id_str.clone()),
                            plan.total_files as u64,
                            plan.total_bytes,
                            resume_from_bytes,
                            Some(plan.clone()),
                            None,
                            Vec::new(),
                            None,
                        ),
                    });
                }
                CoreReceiverEvent::TransferProgress {
                    session_id: _,
                    snapshot,
                } => {
                    // Always track the latest snapshot regardless of whether
                    // we end up emitting a UI tick — needed so the Failed
                    // arm below can report how far we got (drift#29).
                    latest_snapshot = Some(snapshot.clone());
                    let now = std::time::Instant::now();
                    let interval_elapsed =
                        now.duration_since(last_progress_emit_at) >= PROGRESS_EVENT_MIN_INTERVAL;
                    let bytes_advanced = snapshot
                        .bytes_transferred
                        .saturating_sub(last_progress_bytes)
                        >= PROGRESS_EVENT_MIN_BYTES;
                    let phase_changed = matches!(snapshot.phase, TransferPhase::Finalizing)
                        || matches!(snapshot.phase, TransferPhase::Completed);
                    let is_complete = snapshot.total_bytes > 0
                        && snapshot.bytes_transferred >= snapshot.total_bytes;
                    if interval_elapsed || bytes_advanced || is_complete || phase_changed {
                        last_progress_emit_at = now;
                        last_progress_bytes = snapshot.bytes_transferred;
                        // `try_send` over `send().await` so a stalled actor
                        // can't backpressure the receiver transfer loop —
                        // throughput must not be capped by UI responsiveness.
                        // When the channel is full we log + drop; the UI just
                        // misses one progress tick, the transfer continues.
                        if let Err(err) = progress_cmd_tx.try_send(ReceiverCommand::OfferProgress {
                            offer_id,
                            event: build_offer_event(
                                ReceiverOfferPhase::Receiving,
                                sender_label.clone(),
                                save_root_label.clone(),
                                sender_device_type,
                                sender_web,
                                sender_ephemeral,
                                Some(current_path.lock().unwrap().clone()),
                                Some(remote_id_str.clone()),
                                snapshot.total_files as u64,
                                snapshot.total_bytes,
                                snapshot.bytes_transferred,
                                Some(plan.clone()),
                                Some(snapshot.clone()),
                                Vec::new(),
                                None,
                            ),
                        }) {
                            tracing::debug!(
                                target: "wisp_app::receiver::session",
                                offer_id,
                                bytes = snapshot.bytes_transferred,
                                ?err,
                                "dropping progress event — actor channel full \
                                 (UI may show stale speed/progress until next tick)"
                            );
                        }
                    }
                }
                CoreReceiverEvent::Listening { .. } => {}
                CoreReceiverEvent::TransferCompleted { .. } => {
                    break;
                }
                CoreReceiverEvent::Completed { .. } => {
                    break;
                }
                CoreReceiverEvent::Failed { error, .. } => {
                    let _ = progress_cmd_tx.try_send(ReceiverCommand::OfferFinished {
                        offer_id,
                        // drift#29: carry the plan + latest snapshot +
                        // last_progress_bytes + connection path so the UI
                        // can show "Transfer from <peer> failed at
                        // 12 / 50 MB" instead of dropping everything
                        // we already knew.
                        final_event: failed_offer_event(
                            save_root_label.clone(),
                            sender_label.clone(),
                            sender_device_type,
                            sender_web,
                            sender_ephemeral,
                            "Transfer failed.".to_owned(),
                            UserFacingError::from(error),
                            offer.file_count,
                            offer.total_size,
                            last_progress_bytes,
                            Some(plan.clone()),
                            latest_snapshot.clone(),
                            Some(current_path.lock().unwrap().clone()),
                            Some(remote_id_str.clone()),
                            None,
                            offer
                                .items
                                .iter()
                                .map(|file| ReceiverOfferFile {
                                    path: file.path.clone(),
                                    size: file.size,
                                })
                                .collect(),
                        ),
                    });
                    let _ = path_shutdown_tx.send(());
                    let _ = path_watcher.await;
                    return;
                }
                CoreReceiverEvent::OfferReceived { .. } => {}
                // Surfaced pre-offer in the select loop above; once we're
                // tracking an offer it carries no new information.
                CoreReceiverEvent::SenderConnected { .. } => {}
            }
        }

        let final_path = Some(current_path.lock().unwrap().clone());
        // Resolve the peer's relay URL for the send-back ticket. The active
        // path's `relay_url` is only set when iroh is *actually* relaying, so a
        // direct (LAN) transfer leaves it `None`. We fall back to the peer's
        // known relay candidate so the persisted ticket always carries a relay:
        // when the peer later changes networks (Wi-Fi → 4G) its saved direct IP
        // goes stale, and the relay is the only path iroh can still race onto.
        let relay_url = match final_path.as_ref().and_then(|p| p.relay_url.clone()) {
            Some(url) => Some(url),
            None => wisp_core::util::peer_relay_url(&relay_lookup_endpoint, remote_id).await,
        };
        // Synthesize a "send back" ticket so the receiver can later dial this
        // sender via the saved-devices fast path. Best effort: a parse error
        // on the snapshotted addr just drops the ticket. Stale addrs are
        // tolerated — iroh's pkarr discovery (presets::N0) re-resolves the
        // EndpointId on dial.  We compute this once and reuse across the
        // Completed and Cancelled arms — both terminal states represent a
        // peer we successfully reached, even if Cancelled didn't finish.
        let sender_ticket = final_path.as_ref().and_then(|path| {
            wisp_core::util::synthesize_ticket(
                remote_id,
                relay_url.as_deref(),
                path.direct_addr.as_deref(),
            )
            .map_err(|err| {
                tracing::debug!(
                    target: "wisp_app::receiver::session",
                    error = %err,
                    "failed to synthesize sender ticket; saved-device fast path won't work"
                );
                err
            })
            .ok()
        });
        let final_event = match outcome_rx.await {
            Ok(Ok(outcome)) => match outcome {
                CoreTransferOutcome::Completed => completed_offer_event(
                    sender_label,
                    save_root_label,
                    sender_device_type,
                    sender_web,
                    sender_ephemeral,
                    final_path.clone(),
                    Some(remote_id_str.clone()),
                    sender_ticket.clone(),
                    offer.session_id.clone(),
                    offer.file_count,
                    offer.total_size,
                    plan.clone(),
                ),
                CoreTransferOutcome::Declined { .. } => ReceiverOfferEvent {
                    phase: ReceiverOfferPhase::Declined,
                    sender_name: sender_label.clone(),
                    sender_device_type: device_type_to_str(sender_device_type),
                    sender_web,
                    sender_ephemeral,
                    destination_label: sender_label,
                    save_root_label,
                    status_message: "Transfer cancelled.".to_owned(),
                    item_count: offer.file_count,
                    total_size_bytes: offer.total_size,
                    bytes_received: 0,
                    plan: Some(plan.clone()),
                    snapshot: None,
                    connection_path: final_path.clone(),
                    sender_endpoint_id: Some(remote_id_str.clone()),
                    // Declined = peer explicitly rejected this offer. Don't
                    // persist — user probably doesn't want them in Recent.
                    sender_ticket: None,
                    total_size_label: human_size(offer.total_size),
                    files: Vec::new(),
                    inline_text: None,
                    error: None,
                },
                CoreTransferOutcome::Cancelled(cancellation) => ReceiverOfferEvent {
                    phase: ReceiverOfferPhase::Cancelled,
                    sender_name: sender_label.clone(),
                    sender_device_type: device_type_to_str(sender_device_type),
                    sender_web,
                    sender_ephemeral,
                    destination_label: sender_label,
                    save_root_label,
                    status_message: "Transfer cancelled.".to_owned(),
                    item_count: offer.file_count,
                    total_size_bytes: offer.total_size,
                    bytes_received: last_progress_bytes,
                    plan: Some(plan.clone()),
                    snapshot: None,
                    connection_path: final_path.clone(),
                    sender_endpoint_id: Some(remote_id_str.clone()),
                    // Cancelled mid-transfer: peer was real and reachable, we
                    // just didn't finish. Persist so the user can re-send.
                    sender_ticket: sender_ticket.clone(),
                    total_size_label: human_size(offer.total_size),
                    files: Vec::new(),
                    inline_text: None,
                    error: Some(UserFacingError::new(
                        UserFacingErrorKind::Cancelled,
                        "Transfer cancelled",
                        cancellation.reason,
                    )),
                },
            },
            Ok(Err(error)) => failed_offer_event(
                save_root_label,
                sender_label,
                sender_device_type,
                sender_web,
                sender_ephemeral,
                "Transfer failed.".to_owned(),
                UserFacingError::from(error),
                offer.file_count,
                offer.total_size,
                last_progress_bytes,
                Some(plan.clone()),
                latest_snapshot.clone(),
                final_path.clone(),
                Some(remote_id_str.clone()),
                sender_ticket.clone(),
                offer
                    .items
                    .iter()
                    .map(|file| ReceiverOfferFile {
                        path: file.path.clone(),
                        size: file.size,
                    })
                    .collect(),
            ),
            Err(error) => failed_offer_event(
                save_root_label,
                sender_label,
                sender_device_type,
                sender_web,
                sender_ephemeral,
                "Transfer failed.".to_owned(),
                UserFacingError::internal("Transfer failed", format!("{error}")),
                offer.file_count,
                offer.total_size,
                last_progress_bytes,
                Some(plan.clone()),
                latest_snapshot.clone(),
                final_path.clone(),
                Some(remote_id_str.clone()),
                sender_ticket.clone(),
                offer
                    .items
                    .iter()
                    .map(|file| ReceiverOfferFile {
                        path: file.path.clone(),
                        size: file.size,
                    })
                    .collect(),
            ),
        };

        let _ = cmd_tx
            .send(ReceiverCommand::OfferFinished {
                offer_id,
                final_event,
            })
            .await;

        let _ = path_shutdown_tx.send(());
        let _ = path_watcher.await;
    }
}

fn spawn_connection_path_watcher(
    conn_info: iroh::endpoint::ConnectionInfo,
    offer_id: u64,
    cmd_tx: mpsc::Sender<ReceiverCommand>,
    current_path: Arc<Mutex<ConnectionPath>>,
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
                    // Authoritative: the connection's selected path, not the
                    // endpoint address book (which can report a stale direct
                    // candidate as Active).
                    let snapshot = connection_path_from_info(&conn_info);
                    let changed = {
                        let mut guard = current_path.lock().unwrap();
                        // Don't downgrade a known Direct/Relay path to Unknown — that
                        // happens when iroh momentarily has no Active address (NAT
                        // rebind, path migration), and would cause the UI badge to
                        // flicker hide/show every poll.
                        let downgrade_to_unknown = matches!(
                            snapshot.kind,
                            wisp_core::util::ConnectionPathKind::Unknown
                        ) && !matches!(
                            guard.kind,
                            wisp_core::util::ConnectionPathKind::Unknown
                        );
                        if *guard != snapshot && !downgrade_to_unknown {
                            *guard = snapshot.clone();
                            true
                        } else {
                            false
                        }
                    };
                    if changed
                        && cmd_tx
                            .send(ReceiverCommand::OfferConnectionPathChanged {
                                offer_id,
                                connection_path: snapshot,
                            })
                            .await
                            .is_err()
                    {
                        break;
                    }
                }
            }
        }
    })
}

fn build_core_receiver_request(
    device_name: String,
    device_type: DeviceType,
    out_dir: std::path::PathBuf,
    conflict_policy: ConflictPolicy,
) -> CoreReceiverRequest {
    CoreReceiverRequest {
        device_name,
        device_type,
        out_dir,
        conflict_policy,
    }
}

fn completed_offer_event(
    sender_name: String,
    save_root_label: String,
    sender_device_type: DeviceType,
    sender_web: bool,
    sender_ephemeral: bool,
    connection_path: Option<ConnectionPath>,
    sender_endpoint_id: Option<String>,
    sender_ticket: Option<String>,
    session_id: String,
    item_count: u64,
    total_size: u64,
    plan: TransferPlan,
) -> ReceiverOfferEvent {
    ReceiverOfferEvent {
        phase: ReceiverOfferPhase::Completed,
        sender_name: sender_name.clone(),
        sender_device_type: device_type_to_str(sender_device_type),
        sender_web,
        sender_ephemeral,
        destination_label: save_root_label.clone(),
        save_root_label,
        status_message: "Files saved.".to_owned(),
        item_count,
        total_size_bytes: total_size,
        bytes_received: total_size,
        plan: Some(plan.clone()),
        snapshot: Some(TransferSnapshot {
            session_id,
            phase: TransferPhase::Completed,
            total_files: plan.total_files,
            completed_files: plan.total_files,
            total_bytes: plan.total_bytes,
            bytes_transferred: total_size,
            active_file_id: None,
            active_file_bytes: None,
            bytes_per_sec: None,
            eta_seconds: None,
        }),
        connection_path,
        sender_endpoint_id,
        sender_ticket,
        total_size_label: human_size(total_size),
        files: Vec::new(),
        inline_text: None,
        error: None,
    }
}

fn build_offer_event(
    phase: ReceiverOfferPhase,
    sender_label: String,
    save_root_label: String,
    sender_device_type: DeviceType,
    sender_web: bool,
    sender_ephemeral: bool,
    connection_path: Option<ConnectionPath>,
    sender_endpoint_id: Option<String>,
    file_count: u64,
    total_bytes: u64,
    bytes_received: u64,
    plan: Option<TransferPlan>,
    snapshot: Option<TransferSnapshot>,
    files: Vec<ReceiverOfferFile>,
    error: Option<UserFacingError>,
) -> ReceiverOfferEvent {
    ReceiverOfferEvent {
        phase,
        sender_name: sender_label.clone(),
        sender_device_type: device_type_to_str(sender_device_type),
        sender_web,
        sender_ephemeral,
        destination_label: sender_label,
        save_root_label,
        status_message: match phase {
            ReceiverOfferPhase::OfferReady => "Offer ready.".to_owned(),
            ReceiverOfferPhase::Receiving => "Receiving files…".to_owned(),
            ReceiverOfferPhase::Completed => "Files saved.".to_owned(),
            ReceiverOfferPhase::Cancelled => "Transfer cancelled.".to_owned(),
            ReceiverOfferPhase::Failed => "Transfer failed.".to_owned(),
            ReceiverOfferPhase::Declined => "Transfer declined.".to_owned(),
            ReceiverOfferPhase::Connecting => "Connecting...".to_owned(),
        },
        item_count: file_count,
        total_size_bytes: total_bytes,
        bytes_received,
        plan,
        snapshot,
        connection_path,
        sender_endpoint_id,
        sender_ticket: None,
        total_size_label: human_size(total_bytes),
        files,
        inline_text: None,
        error,
    }
}

/// Builds a final ReceiverOfferEvent for the `Failed` phase.
///
/// drift#29: failure events used to discard the plan / item counts /
/// bytes_received / connection path / files context the caller already
/// knew, leaving the UI to show "zero items, zero bytes, plan: None"
/// at the moment the user most needs that information.  All those
/// fields are now passed through explicitly — call sites that have the
/// context fill it; defensive call sites (offer never arrived) pass
/// zeros / `None`.
#[allow(clippy::too_many_arguments)]
fn failed_offer_event(
    save_root_label: String,
    sender_name: String,
    sender_device_type: DeviceType,
    sender_web: bool,
    sender_ephemeral: bool,
    status_message: String,
    error: UserFacingError,
    item_count: u64,
    total_size_bytes: u64,
    bytes_received: u64,
    plan: Option<TransferPlan>,
    snapshot: Option<TransferSnapshot>,
    connection_path: Option<ConnectionPath>,
    sender_endpoint_id: Option<String>,
    sender_ticket: Option<String>,
    files: Vec<ReceiverOfferFile>,
) -> ReceiverOfferEvent {
    // Mirror the Cancelled arm's behaviour: when we know who the peer was,
    // reflect it as the destination_label so the UI can show "Transfer
    // from <peer> failed" rather than an empty header.
    let destination_label = sender_name.clone();
    ReceiverOfferEvent {
        phase: ReceiverOfferPhase::Failed,
        sender_name,
        sender_device_type: device_type_to_str(sender_device_type),
        sender_web,
        sender_ephemeral,
        destination_label,
        save_root_label,
        status_message,
        item_count,
        total_size_bytes,
        bytes_received,
        plan,
        snapshot,
        connection_path,
        sender_endpoint_id,
        sender_ticket,
        total_size_label: human_size(total_size_bytes),
        files,
        inline_text: None,
        error: Some(error),
    }
}

/// Builds the `Connecting` offer event surfaced the moment a sender's Hello is
/// read (before its Offer arrives). Carries the sender identity but no
/// manifest — the UI renders a "connecting from <X>" screen with no
/// accept/decline yet.
fn connecting_offer_event(
    save_root_label: String,
    sender_name: String,
    sender_device_type: DeviceType,
    sender_web: bool,
    sender_ephemeral: bool,
    sender_endpoint_id: String,
) -> ReceiverOfferEvent {
    let destination_label = sender_name.clone();
    ReceiverOfferEvent {
        phase: ReceiverOfferPhase::Connecting,
        sender_name: sender_name.clone(),
        sender_device_type: device_type_to_str(sender_device_type),
        sender_web,
        sender_ephemeral,
        destination_label,
        save_root_label,
        status_message: format!("{sender_name} is connecting…"),
        item_count: 0,
        total_size_bytes: 0,
        bytes_received: 0,
        plan: None,
        snapshot: None,
        connection_path: None,
        sender_endpoint_id: Some(sender_endpoint_id),
        sender_ticket: None,
        total_size_label: human_size(0),
        files: Vec::new(),
        inline_text: None,
        error: None,
    }
}

/// Dispatch a terminal failure for a transfer that died *before* its Offer
/// arrived. When the sender's Hello was already read we attribute the failure
/// to that sender (so it surfaces in the UI rather than being suppressed as an
/// "unknown sender" card); otherwise we fall back to an empty name.
async fn send_pre_offer_failure(
    cmd_tx: &mpsc::Sender<ReceiverCommand>,
    offer_id: u64,
    save_root_label: &str,
    fallback_device_type: DeviceType,
    connecting_sender: Option<&(String, DeviceType, bool, bool, String)>,
    error: UserFacingError,
) {
    let (sender_name, sender_device_type, sender_web, sender_ephemeral, sender_endpoint_id) =
        match connecting_sender {
            Some((name, dt, web, eph, id)) => (name.clone(), *dt, *web, *eph, Some(id.clone())),
            None => (String::new(), fallback_device_type, false, false, None),
        };
    let _ = cmd_tx
        .send(ReceiverCommand::OfferFinished {
            offer_id,
            final_event: failed_offer_event(
                save_root_label.to_owned(),
                sender_name,
                sender_device_type,
                sender_web,
                sender_ephemeral,
                "Transfer failed.".to_owned(),
                error,
                0,
                0,
                0,
                None,
                None,
                None,
                sender_endpoint_id,
                None,
                Vec::new(),
            ),
        })
        .await;
}

#[cfg(test)]
mod tests {
    use super::{
        build_core_receiver_request, build_offer_event, completed_offer_event,
        connecting_offer_event, failed_offer_event, send_pre_offer_failure,
    };
    use crate::error::UserFacingErrorKind;
    use crate::receiver::actor::ReceiverCommand;
    use crate::types::{ConflictPolicy, ConnectionPath, ReceiverOfferFile, ReceiverOfferPhase};
    use wisp_core::protocol::DeviceType;
    use wisp_core::transfer::{TransferPhase, TransferPlan, TransferPlanFile, TransferSnapshot};
    use wisp_core::util::ConnectionPathKind;

    #[test]
    fn connecting_offer_event_carries_sender_and_empty_manifest() {
        // Emitted the instant the sender's Hello is read — identity is known
        // but the manifest isn't, so the UI shows "connecting from X" with no
        // files / plan / accept action.
        let event = connecting_offer_event(
            "Downloads".to_owned(),
            "Maya".to_owned(),
            DeviceType::Laptop,
            false,
            false,
            "endpoint-123".to_owned(),
        );

        assert_eq!(event.phase, ReceiverOfferPhase::Connecting);
        assert_eq!(event.sender_name, "Maya");
        assert_eq!(event.destination_label, "Maya");
        assert_eq!(event.item_count, 0);
        assert_eq!(event.total_size_bytes, 0);
        assert!(event.files.is_empty());
        assert!(event.plan.is_none());
        assert_eq!(event.sender_endpoint_id.as_deref(), Some("endpoint-123"));
        assert!(event.error.is_none());
    }

    #[test]
    fn offer_events_carry_web_and_ephemeral_flags() {
        // The browser peer's identity (web + ephemeral) must survive into the
        // UI event so the receiver can render a globe and skip saving it to
        // Recent.
        let web = connecting_offer_event(
            "Downloads".to_owned(),
            "Browser".to_owned(),
            DeviceType::Laptop,
            true,
            true,
            "endpoint-web".to_owned(),
        );
        assert!(web.sender_web);
        assert!(web.sender_ephemeral);

        // A native peer stays unflagged (globe/guard don't misfire).
        let native = connecting_offer_event(
            "Downloads".to_owned(),
            "Maya".to_owned(),
            DeviceType::Phone,
            false,
            false,
            "endpoint-maya".to_owned(),
        );
        assert!(!native.sender_web);
        assert!(!native.sender_ephemeral);
    }

    #[tokio::test]
    async fn pre_offer_failure_is_attributed_to_known_sender() {
        // When the Hello was read before the offer stalled, the failure must
        // name the sender so the actor surfaces it (the `identified_sender`
        // gate) instead of suppressing it as a bogus "unknown sender" card.
        let (tx, mut rx) = tokio::sync::mpsc::channel(4);
        let identity = (
            "Maya".to_owned(),
            DeviceType::Laptop,
            false,
            false,
            "endpoint-123".to_owned(),
        );
        send_pre_offer_failure(
            &tx,
            7,
            "Downloads",
            DeviceType::Phone,
            Some(&identity),
            crate::error::UserFacingError::internal("Transfer failed", "stalled"),
        )
        .await;

        match rx.recv().await.expect("a command was dispatched") {
            ReceiverCommand::OfferFinished {
                offer_id,
                final_event,
            } => {
                assert_eq!(offer_id, 7);
                assert_eq!(final_event.phase, ReceiverOfferPhase::Failed);
                assert_eq!(final_event.sender_name, "Maya");
                assert_eq!(
                    final_event.sender_endpoint_id.as_deref(),
                    Some("endpoint-123")
                );
            }
            _ => panic!("expected an OfferFinished command"),
        }
    }

    #[tokio::test]
    async fn pre_offer_failure_without_hello_has_empty_sender() {
        // Handshake died before the Hello — no identity to attribute, so the
        // sender_name stays empty and the actor suppresses the bogus card.
        let (tx, mut rx) = tokio::sync::mpsc::channel(4);
        send_pre_offer_failure(
            &tx,
            9,
            "Downloads",
            DeviceType::Phone,
            None,
            crate::error::UserFacingError::internal("Transfer failed", "no hello"),
        )
        .await;

        match rx.recv().await.expect("a command was dispatched") {
            ReceiverCommand::OfferFinished { final_event, .. } => {
                assert!(final_event.sender_name.is_empty());
                assert!(final_event.sender_endpoint_id.is_none());
            }
            _ => panic!("expected an OfferFinished command"),
        }
    }

    #[test]
    fn core_receiver_request_preserves_configured_conflict_policy() {
        let request = build_core_receiver_request(
            "Receiver".to_owned(),
            DeviceType::Laptop,
            std::path::PathBuf::from("downloads"),
            ConflictPolicy::Reject,
        );

        assert_eq!(request.conflict_policy, ConflictPolicy::Reject);
    }

    #[test]
    fn failed_offer_event_uses_structured_error() {
        let event = failed_offer_event(
            "Downloads".to_owned(),
            "Maya".to_owned(),
            DeviceType::Laptop,
            false,
            false,
            "Transfer failed.".to_owned(),
            crate::error::UserFacingError::internal("Transfer failed", "boom"),
            0,
            0,
            0,
            None,
            None,
            None,
            None,
            None,
            Vec::new(),
        );

        assert_eq!(event.sender_name, "Maya");
        let error = event.error.expect("structured error");
        assert_eq!(error.kind(), UserFacingErrorKind::Internal);
        assert_eq!(error.title(), "Transfer failed");
        assert_eq!(error.message(), "boom");
    }

    /// drift#29 regression: when failure fires after the receiver has
    /// already seen an offer + transfer progress, the final event must
    /// carry the plan, item counts, bytes_received, connection path,
    /// and file list rather than dropping all of them.
    #[test]
    fn failed_offer_event_preserves_plan_and_progress_context() {
        let plan = TransferPlan::try_new(
            "session-1".to_owned(),
            vec![
                TransferPlanFile {
                    id: 0,
                    path: "a.txt".to_owned(),
                    size: 4,
                },
                TransferPlanFile {
                    id: 1,
                    path: "b.txt".to_owned(),
                    size: 8,
                },
            ],
        )
        .unwrap();
        let snapshot = TransferSnapshot {
            session_id: "session-1".to_owned(),
            phase: TransferPhase::Transferring,
            total_files: 2,
            completed_files: 1,
            total_bytes: 12,
            bytes_transferred: 7,
            active_file_id: Some(1),
            active_file_bytes: Some(3),
            bytes_per_sec: Some(100),
            eta_seconds: Some(1),
        };
        let path = ConnectionPath {
            kind: ConnectionPathKind::Direct,
            relay_url: None,
            direct_addr: Some("192.168.1.5:5000".to_owned()),
        };
        let files = vec![
            ReceiverOfferFile {
                path: "a.txt".to_owned(),
                size: 4,
            },
            ReceiverOfferFile {
                path: "b.txt".to_owned(),
                size: 8,
            },
        ];

        let event = failed_offer_event(
            "Downloads".to_owned(),
            "Maya".to_owned(),
            DeviceType::Laptop,
            false,
            false,
            "Transfer failed.".to_owned(),
            crate::error::UserFacingError::internal("Transfer failed", "boom"),
            2,
            12,
            7,
            Some(plan.clone()),
            Some(snapshot.clone()),
            Some(path.clone()),
            Some("endpoint-abc".to_owned()),
            Some("wisp-pair:xyz".to_owned()),
            files.clone(),
        );

        assert_eq!(event.sender_name, "Maya");
        assert_eq!(event.destination_label, "Maya");
        assert_eq!(event.item_count, 2);
        assert_eq!(event.total_size_bytes, 12);
        assert_eq!(event.bytes_received, 7);
        assert_eq!(event.plan.as_ref(), Some(&plan));
        assert_eq!(event.snapshot.as_ref(), Some(&snapshot));
        assert_eq!(event.connection_path.as_ref(), Some(&path));
        assert_eq!(event.sender_endpoint_id.as_deref(), Some("endpoint-abc"));
        assert_eq!(event.sender_ticket.as_deref(), Some("wisp-pair:xyz"));
        assert_eq!(event.files, files);
        assert!(!event.total_size_label.is_empty());
        assert!(event.error.is_some());
    }

    #[test]
    fn build_offer_event_can_carry_structured_error() {
        let direct = ConnectionPath {
            kind: ConnectionPathKind::Direct,
            relay_url: None,
            direct_addr: Some("192.168.1.5:5000".to_owned()),
        };
        let event = build_offer_event(
            super::ReceiverOfferPhase::Failed,
            "Sender".to_owned(),
            "Downloads".to_owned(),
            DeviceType::Laptop,
            false,
            false,
            Some(direct.clone()),
            None,
            0,
            0,
            0,
            None,
            None,
            Vec::new(),
            Some(crate::error::UserFacingError::internal(
                "Transfer failed",
                "boom",
            )),
        );

        assert_eq!(event.connection_path, Some(direct));
        assert_eq!(
            event.error.as_ref().map(|error| error.kind()),
            Some(UserFacingErrorKind::Internal)
        );
    }

    #[test]
    fn completed_offer_event_uses_save_root_as_destination_label() {
        let plan = TransferPlan::try_new(
            "session-1",
            vec![wisp_core::transfer::TransferPlanFile {
                id: 0,
                path: "report.pdf".to_owned(),
                size: 1024,
            }],
        )
        .expect("plan");

        let event = completed_offer_event(
            "Maya".to_owned(),
            "Downloads".to_owned(),
            DeviceType::Laptop,
            false,
            false,
            Some(ConnectionPath {
                kind: ConnectionPathKind::Direct,
                relay_url: None,
                direct_addr: Some("192.168.1.5:5000".to_owned()),
            }),
            None,
            None,
            "session-1".to_owned(),
            1,
            1024,
            plan,
        );

        assert_eq!(event.destination_label, "Downloads");
        assert_eq!(event.save_root_label, "Downloads");
        assert_eq!(event.sender_name, "Maya");
        assert_eq!(event.phase, super::ReceiverOfferPhase::Completed);
    }
}

fn device_type_to_str(value: DeviceType) -> String {
    match value {
        DeviceType::Phone => "phone".to_owned(),
        DeviceType::Laptop => "laptop".to_owned(),
    }
}

pub(super) fn save_root_display(path: &std::path::Path) -> String {
    let file_name = path.file_name().and_then(|s| s.to_str());
    let parent_name = path
        .parent()
        .and_then(|parent| parent.file_name())
        .and_then(|s| s.to_str());
    if matches!(file_name, Some("Wisp")) && matches!(parent_name, Some("Download" | "Downloads")) {
        return "Downloads".to_owned();
    }
    path.file_name()
        .and_then(|s| s.to_str())
        .map(String::from)
        .unwrap_or_else(|| path.display().to_string())
}

fn display_sender_label(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return "Sender".to_owned();
    }
    // Collapse internal whitespace runs but otherwise preserve the name the
    // sender set — including '-' and '_'.  Previously we rewrote separators to
    // spaces, which silently turned "Alex - Laptop" into "Alex Laptop".
    let normalized = trimmed.split_whitespace().collect::<Vec<_>>().join(" ");

    // Placeholder detection flattens separators only for the comparison; the
    // value we return keeps the original punctuation.
    let flattened = normalized
        .replace(['_', '-'], " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase();
    if flattened.is_empty() || flattened == "unknown device" || flattened == "unknown" {
        return "Sender".to_owned();
    }
    normalized
}
