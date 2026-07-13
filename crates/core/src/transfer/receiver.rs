#![allow(dead_code)]

use futures_lite::StreamExt;
use iroh::{Endpoint, endpoint::Connection};
use iroh_blobs::{
    ALPN as BLOBS_ALPN,
    api::{blobs::ExportMode, blobs::ExportOptions},
    format::collection::Collection,
    store::fs::FsStore,
    ticket::BlobTicket,
};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::fs;
use tokio::io::AsyncRead;
use tokio::sync::{mpsc, oneshot, watch};
use tokio_stream::wrappers::UnboundedReceiverStream;
use tracing::{info, instrument, warn};

use crate::{
    blobs::receive::{BlobDownloadSession, BlobDownloadUpdate, BlobReceiver},
    fs_plan::ConflictPolicy,
    protocol::message as protocol_message,
    protocol::message::INLINE_TEXT_HARD_MAX_BYTES,
    protocol::message::MessageKind,
    protocol::wire as protocol_wire,
    protocol::{ALPN, ProtocolError},
    rendezvous::OfferManifest,
};

use super::error::{Result as TransferResult, TransferError};
use super::path::{
    RecordDirGuard, ensure_destination_available, local_record_dir, resolve_output_dir,
    resolve_transfer_destination,
};
use super::progress::ProgressTracker;
use super::record::{TransferRecord, TransferStatus};
use super::types::{
    TransferOutcome, TransferPhase, TransferPlan, TransferSnapshot, wait_for_cancel,
};

type Result<T> = TransferResult<T>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReceiverRequest {
    pub device_name: String,
    pub device_type: crate::protocol::DeviceType,
    pub out_dir: std::path::PathBuf,
    pub conflict_policy: ConflictPolicy,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReceiverDecision {
    Accept,
    Decline,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReceiverOfferItem {
    pub path: String,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReceiverOffer {
    pub session_id: String,
    pub collection_hash: iroh_blobs::Hash,
    pub resume_from_bytes: u64,
    pub sender_device_name: String,
    pub sender_device_type: crate::protocol::DeviceType,
    pub sender_endpoint_id: iroh::EndpointId,
    pub items: Vec<ReceiverOfferItem>,
    pub file_count: u64,
    pub total_size: u64,
    /// Plain text carried inline in the offer for a text-only transfer (no
    /// blobs).  `None` for ordinary file transfers.
    pub inline_text: Option<String>,
}

#[derive(Debug)]
pub enum ReceiverEvent {
    Listening {
        endpoint_id: iroh::EndpointId,
    },
    /// A sender opened a connection and completed the Hello exchange, but the
    /// transfer Offer hasn't arrived yet. Emitted as soon as the sender's
    /// identity is known so the UI can switch to a "connecting from <X>"
    /// screen instead of sitting silently on the QR/idle screen while the
    /// offer is in flight (or stalled).
    SenderConnected {
        session_id: String,
        sender_device_name: String,
        sender_device_type: crate::protocol::DeviceType,
        sender_endpoint_id: iroh::EndpointId,
    },
    OfferReceived {
        session_id: String,
        sender_device_name: String,
        sender_endpoint_id: iroh::EndpointId,
        file_count: u64,
        total_size: u64,
        resume_from_bytes: u64,
    },
    TransferStarted {
        session_id: String,
        plan: TransferPlan,
    },
    TransferProgress {
        session_id: String,
        snapshot: TransferSnapshot,
    },
    TransferCompleted {
        session_id: String,
        snapshot: TransferSnapshot,
    },
    Failed {
        session_id: String,
        error: TransferError,
    },
    Completed {
        session_id: String,
    },
}

pub type ReceiverEventStream = UnboundedReceiverStream<ReceiverEvent>;

#[derive(Debug)]
pub struct ReceiverControl {
    pub decision_tx: oneshot::Sender<ReceiverDecision>,
    pub cancel_tx: watch::Sender<bool>,
}

#[derive(Debug)]
pub struct ReceiverStart {
    pub events: ReceiverEventStream,
    pub offer_rx: oneshot::Receiver<Result<ReceiverOffer>>,
    pub outcome_rx: oneshot::Receiver<TransferResult<TransferOutcome>>,
    pub control: ReceiverControl,
}

#[derive(Debug)]
pub struct ReceiverSession {
    request: ReceiverRequest,
}

#[derive(Debug, Clone)]
pub struct ExpectedTransferFile {
    pub path: String,
    pub size: u64,
    pub destination: PathBuf,
}

impl ReceiverSession {
    pub fn new(request: ReceiverRequest) -> Self {
        Self { request }
    }

    pub fn start(self, endpoint: Endpoint, connection: Connection) -> ReceiverStart
    where
        Self: Send + 'static,
    {
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let (offer_tx, offer_rx) = oneshot::channel();
        let (decision_tx, decision_rx) = oneshot::channel();
        let (cancel_tx, cancel_rx) = watch::channel(false);
        let (outcome_tx, outcome_rx) = oneshot::channel();
        let request = self.request.clone();

        tokio::spawn(async move {
            let outcome = run_session(
                endpoint,
                connection,
                request,
                Some(event_tx),
                offer_tx,
                decision_rx,
                cancel_rx,
            )
            .await
            .map_err(|error| TransferError::other("running receiver session", error));
            let _ = outcome_tx.send(outcome);
        });

        ReceiverStart {
            events: UnboundedReceiverStream::new(event_rx),
            offer_rx,
            outcome_rx,
            control: ReceiverControl {
                decision_tx,
                cancel_tx,
            },
        }
    }
}

#[instrument(skip_all, fields(remote = %connection.remote_id()))]
async fn run_session(
    endpoint: Endpoint,
    connection: Connection,
    mut request: ReceiverRequest,
    event_tx: Option<mpsc::UnboundedSender<ReceiverEvent>>,
    offer_tx: oneshot::Sender<Result<ReceiverOffer>>,
    decision_rx: oneshot::Receiver<ReceiverDecision>,
    mut cancel_rx: watch::Receiver<bool>,
) -> Result<TransferOutcome> {
    request.out_dir = resolve_output_dir(&request.out_dir)?;

    emit_receiver_event(
        &event_tx,
        ReceiverEvent::Listening {
            endpoint_id: endpoint.addr().id,
        },
    );

    // --- Phase 1: Handshake ---
    let (mut control_send, mut control_recv, peer_hello, offer) =
        match do_handshake(&endpoint, &request, &connection, &event_tx, &mut cancel_rx).await? {
            HandshakeResult::Ok(s, r, h, o) => (s, r, h, o),
            HandshakeResult::Cancelled(outcome) => {
                let _ = offer_tx.send(Err(TransferError::other(
                    "cancelled during handshake",
                    std::io::Error::other("cancelled during handshake"),
                )));
                return Ok(outcome);
            }
        };

    let session_id = peer_hello.session_id.clone();
    tracing::Span::current().record("session_id", &session_id);

    // --- Phase 2: Offer Processing ---
    let manifest = to_offer_manifest(&offer);
    info!(
        %session_id,
        collection_hash = %offer.collection_hash,
        file_count = manifest.file_count,
        total_size = manifest.total_size,
        "received manifest"
    );
    let plan = TransferPlan::from_manifest(session_id.clone(), &offer.manifest)?;
    let record_dir =
        local_record_dir(&request.out_dir, offer.collection_hash).map_err(TransferError::from)?;

    // RAII guard for the per-transfer record dir under `<out_dir>/.wisp/
    // transfers/<hash>/`.  Default `delete_on_drop = false` (keep state in
    // case the user wants to resume); we flip it to `true` below for
    // terminal outcomes where there's no point keeping resume state
    // (Completed / Cancelled / Declined).  Any `Err(_)` exit — including
    // `TransferError::ConnectionClosed` from a transient network drop —
    // leaves the guard alone so the data sticks around for a retry.  Stale
    // dirs from those failure paths get garbage-collected by
    // `sweep_stale_transfer_records` at next receiver-service startup.
    let mut record_guard = RecordDirGuard::new(record_dir.clone());

    let outcome: Result<TransferOutcome> = async {
    let resume_record = load_resume_record(&record_dir, offer.collection_hash, &offer.manifest);
    let resume_from_bytes = resume_record
        .as_ref()
        .map(|record| resume_offset_for_record(record, plan.total_bytes))
        .unwrap_or(0);
    let expected_files = match build_expected_files(
        &manifest,
        &request.out_dir,
        request.conflict_policy,
        resume_record.as_ref(),
    )
    .await
    {
        Ok(f) => f,
        Err(err) => {
            let reason = err.to_string();
            let _ = send_receiver_decline(&mut control_send, &session_id, reason.clone()).await;
            finish_control_stream(&mut control_send).await;
            let _ = offer_tx.send(Err(TransferError::other(
                "building receiver offer",
                std::io::Error::other(reason.clone()),
            )));
            return Err(err);
        }
    };

    let expected_transfer_files = build_expected_transfer_files(&manifest, expected_files)?;
    let receiver_offer = ReceiverOffer {
        session_id: session_id.clone(),
        collection_hash: offer.collection_hash,
        resume_from_bytes,
        sender_device_name: peer_hello.identity.device_name.clone(),
        sender_device_type: to_local_device_type(peer_hello.identity.device_type),
        sender_endpoint_id: peer_hello.identity.endpoint_id,
        items: manifest
            .files
            .iter()
            .map(|item| ReceiverOfferItem {
                path: item.path.clone(),
                size: item.size,
            })
            .collect(),
        file_count: manifest.file_count,
        total_size: manifest.total_size,
        inline_text: offer.inline_text.clone(),
    };

    emit_receiver_event(
        &event_tx,
        ReceiverEvent::OfferReceived {
            session_id: session_id.clone(),
            sender_device_name: receiver_offer.sender_device_name.clone(),
            sender_endpoint_id: receiver_offer.sender_endpoint_id,
            file_count: receiver_offer.file_count,
            total_size: receiver_offer.total_size,
            resume_from_bytes,
        },
    );
    let _ = offer_tx.send(Ok(receiver_offer.clone()));

    // --- Inline text: no blobs, no record dir, no progress stream ---
    if let Some(text) = offer.inline_text.clone() {
        // Guard against a malicious/buggy peer shipping an unbounded frame.
        if text.len() > INLINE_TEXT_HARD_MAX_BYTES {
            let reason = format!(
                "inline text exceeds {INLINE_TEXT_HARD_MAX_BYTES} byte limit"
            );
            let _ = send_receiver_decline(&mut control_send, &session_id, reason.clone()).await;
            finish_control_stream(&mut control_send).await;
            return Err(TransferError::other(
                "inline text too large",
                std::io::Error::other(reason),
            ));
        }

        let decision = tokio::select! {
            res = decision_rx => res.map_err(|_| TransferError::channel_closed("waiting for receiver decision"))?,
            _ = wait_for_cancel(&mut cancel_rx) => return abort_session(&mut control_send, &session_id, protocol_message::CancelPhase::WaitingForDecision).await,
            _ = connection.closed() => return Err(TransferError::connection_closed("before receiver decision")),
            _ = tokio::time::sleep(Duration::from_secs(120)) => return Err(TransferError::timeout("waiting for receiver decision")),
        };

        if decision == ReceiverDecision::Decline {
            let _ = send_receiver_decline(
                &mut control_send,
                &session_id,
                "declined by user".to_owned(),
            )
            .await;
            finish_control_stream(&mut control_send).await;
            return Ok(TransferOutcome::Declined {
                reason: "receiver declined".to_owned(),
            });
        }

        // Confirm receipt: Accept, then a successful TransferResult — the
        // sender reads both, acks, and we're done.  Reuses the same control
        // messages as the file path.
        protocol_wire::write_receiver_message(
            &mut control_send,
            &protocol_message::ReceiverMessage::Accept(protocol_message::Accept {
                session_id: session_id.clone(),
            }),
        )
        .await?;
        let _ = send_transfer_result(
            &mut control_send,
            &session_id,
            protocol_message::TransferStatus::Ok,
        )
        .await;

        let byte_len = text.len() as u64;
        let snapshot = TransferSnapshot {
            session_id: session_id.clone(),
            phase: TransferPhase::Completed,
            total_files: 1,
            completed_files: 1,
            total_bytes: byte_len,
            bytes_transferred: byte_len,
            active_file_id: None,
            active_file_bytes: None,
            bytes_per_sec: None,
            eta_seconds: None,
        };

        await_final_sender_ack(&mut control_recv, &session_id).await;
        finish_control_stream(&mut control_send).await;
        emit_receiver_event(
            &event_tx,
            ReceiverEvent::TransferCompleted {
                session_id: session_id.clone(),
                snapshot,
            },
        );
        emit_receiver_event(
            &event_tx,
            ReceiverEvent::Completed {
                session_id: session_id.clone(),
            },
        );
        return Ok(TransferOutcome::Completed);
    }

    // --- Phase 3: User Decision ---
    let decision = tokio::select! {
        res = decision_rx => res.map_err(|_| TransferError::channel_closed("waiting for receiver decision"))?,
        _ = wait_for_cancel(&mut cancel_rx) => return abort_session(&mut control_send, &session_id, protocol_message::CancelPhase::WaitingForDecision).await,
        _ = connection.closed() => return Err(TransferError::connection_closed("before receiver decision")),
        _ = tokio::time::sleep(Duration::from_secs(120)) => return Err(TransferError::timeout("waiting for receiver decision")),
    };

    if decision == ReceiverDecision::Decline {
        let _ = send_receiver_decline(
            &mut control_send,
            &session_id,
            "declined by user".to_owned(),
        )
        .await;
        finish_control_stream(&mut control_send).await;
        return Ok(TransferOutcome::Declined {
            reason: "receiver declined".to_owned(),
        });
    }

    // --- Phase 4: Data Transfer ---
    fs::create_dir_all(&record_dir)
        .await
        .map_err(|e| TransferError::other("creating record directory", e))?;

    let mut record = match resume_record {
        Some(r) => {
            info!(
                "found existing transfer record for {}, resuming",
                receiver_offer.collection_hash
            );
            r
        }
        None => {
            let r = TransferRecord::new(
                receiver_offer.collection_hash,
                request.out_dir.clone(),
                request.conflict_policy,
                offer.manifest.clone(),
            );
            r.save(&record_dir)
                .map_err(|e| TransferError::other("saving initial record", e))?;
            r
        }
    };

    let _ = protocol_wire::write_receiver_message(
        &mut control_send,
        &protocol_message::ReceiverMessage::Accept(protocol_message::Accept {
            session_id: session_id.clone(),
        }),
    )
    .await?;

    let mut progress_send = connection
        .open_uni()
        .await
        .map_err(|source| TransferError::other("opening progress stream", source))?;

    let (_transfer_outcome, mut tracker) = if matches!(
        record.status,
        TransferStatus::DataComplete | TransferStatus::Finalizing | TransferStatus::Completed
    ) {
        info!(
            "data already complete for {}, skipping download",
            receiver_offer.collection_hash
        );
        let _ = match tokio::select! {
            res = read_sender_blob_ticket_message(&mut control_recv, &session_id) => res?,
            _ = wait_for_cancel(&mut cancel_rx) => return abort_session(&mut control_send, &session_id, protocol_message::CancelPhase::Transferring).await,
            _ = connection.closed() => return Err(TransferError::connection_closed("waiting for resume blob ticket")),
            _ = tokio::time::sleep(Duration::from_secs(30)) => return Err(TransferError::timeout("waiting for resume blob ticket")),
        } {
            SenderBlobTicketRead::Ticket(ticket) => ticket,
            SenderBlobTicketRead::Cancel(cancel) => {
                return Ok(TransferOutcome::from_remote_cancel(cancel, &session_id)?);
            }
        };
        let mut tracker = ProgressTracker::new(plan.clone());
        tracker.set_bytes_transferred(resume_from_bytes, std::time::Instant::now());
        (TransferOutcome::Completed, tracker)
    } else {
        let ticket_message = match tokio::select! {
            res = read_sender_blob_ticket_message(&mut control_recv, &session_id) => res?,
            _ = wait_for_cancel(&mut cancel_rx) => return abort_session(&mut control_send, &session_id, protocol_message::CancelPhase::Transferring).await,
            _ = connection.closed() => return Err(TransferError::connection_closed("waiting for blob ticket")),
            _ = tokio::time::sleep(Duration::from_secs(30)) => return Err(TransferError::timeout("waiting for blob ticket")),
        } {
            SenderBlobTicketRead::Ticket(ticket) => ticket,
            SenderBlobTicketRead::Cancel(cancel) => {
                return Ok(TransferOutcome::from_remote_cancel(cancel, &session_id)?);
            }
        };

        let blob_ticket: BlobTicket = ticket_message
            .ticket
            .parse()
            .map_err(|source| TransferError::other("parsing blob ticket", source))?;
        let blob_receiver = BlobReceiver::new(endpoint.clone());
        let mut blob_download = blob_receiver
            .start(record_dir.join("store"), blob_ticket.clone(), false)
            .await
            .map_err(|source| TransferError::other("starting blob download", source))?;

        let (outcome, tracker) = match do_transfer(
            &session_id,
            &plan,
            &mut blob_download,
            &mut progress_send,
            &mut control_recv,
            &mut cancel_rx,
            &event_tx,
            &mut record,
            &record_dir,
            resume_from_bytes,
        )
        .await
        {
            Ok(v) => v,
            Err(error) => {
                if let TransferError::Protocol(protocol_error) = &error {
                    let _ = send_transfer_result(
                        &mut control_send,
                        &session_id,
                        protocol_error.transfer_status(),
                    )
                    .await;
                }
                blob_download.abort();
                let _ = blob_download.shutdown().await;
                return Err(error);
            }
        };

        if let TransferOutcome::Cancelled(c) = &outcome {
            let _ = send_receiver_cancel(
                &mut control_send,
                &session_id,
                c.by,
                c.phase,
                c.reason.clone(),
            )
            .await;
            blob_download.abort();
            let _ = blob_download.shutdown().await;
            return Ok(outcome);
        }

        record.status = TransferStatus::DataComplete;
        record.bytes_received = plan.total_bytes;
        record
            .save(&record_dir)
            .map_err(|e| TransferError::other("saving record after download", e))?;
        let _ = blob_download.shutdown().await;
        (outcome, tracker)
    };

    // --- Phase 5: Export & Acknowledgement ---
    info!(%session_id, "exporting files to {}", request.out_dir.display());
    record.status = TransferStatus::Finalizing;
    record
        .save(&record_dir)
        .map_err(|e| TransferError::other("saving record before export", e))?;

    tracker.mark_finalizing(std::time::Instant::now());
    let finalizing_snapshot = tracker.snapshot(std::time::Instant::now());
    // Cumulative bytes written out by the export step, shared with the
    // heartbeat below so the receiver's own UI can animate a "saving to
    // disk" bar (0 → total) during Phase 5 instead of sitting frozen at
    // 100% for the whole export.  The wire frames keep reporting the
    // canonical finalizing snapshot (bytes == total) purely as a QUIC
    // keepalive — the sender already showed 100% and shouldn't see the
    // bar walk backwards.
    let exported_bytes = AtomicU64::new(0);
    // Local event: the save just started, so the receiver bar resets to 0
    // and climbs as files land.  Skipped entirely on the fast path
    // (`ExportMode::TryReference` reflink/hardlink finishes before the
    // first heartbeat tick), so there's no visible reset in practice.
    emit_receiver_event(
        &event_tx,
        ReceiverEvent::TransferProgress {
            session_id: session_id.clone(),
            snapshot: TransferSnapshot {
                bytes_transferred: 0,
                ..finalizing_snapshot.clone()
            },
        },
    );
    let _ = protocol_wire::write_receiver_message(
        &mut progress_send,
        &protocol_message::ReceiverMessage::TransferProgress(protocol_message::TransferProgress {
            session_id: session_id.clone(),
            snapshot: to_wire_snapshot(&finalizing_snapshot),
        }),
    )
    .await;

    let blob_store = FsStore::load(record_dir.join("store"))
        .await
        .map_err(|e| TransferError::other("loading blob store for export", e))?;

    // Drive `export_downloaded_collection` to completion while keeping the
    // application-level wire busy with periodic Finalizing progress frames.
    // Without these the export window has no app traffic, so iroh's tight
    // path keepalive (6_000 ms idle / 4_500 ms ping) is the only thing
    // keeping the QUIC path up — a single lost keepalive on flaky mobile
    // networks then tears the path and the sender side errors with a
    // generic "Protocol mismatch" the moment it tries to read the
    // post-export TransferResult.  Heartbeating real progress messages
    // here gives the keepalive a backup and lets the sender's UI show
    // "Finalizing" instead of looking frozen.  See
    // docs/upstream-bug-audit.md for the full analysis.
    let final_snapshot = {
        let export_fut = export_downloaded_collection(
            &blob_store,
            receiver_offer.collection_hash,
            &expected_transfer_files,
            &mut record,
            &record_dir,
            &exported_bytes,
        );
        match run_with_progress_heartbeat(
            export_fut,
            &mut progress_send,
            &session_id,
            &mut tracker,
            &mut cancel_rx,
            EXPORT_HEARTBEAT_INTERVAL,
            &event_tx,
            &exported_bytes,
        )
        .await
        {
            HeartbeatOutcome::Completed => {
                tracker.mark_completed(std::time::Instant::now());
                tracker.snapshot(std::time::Instant::now())
            }
            HeartbeatOutcome::Failed(error) => return Err(error),
            HeartbeatOutcome::Cancelled => {
                return abort_session(
                    &mut control_send,
                    &session_id,
                    protocol_message::CancelPhase::Transferring,
                )
                .await;
            }
        }
    };

    record.status = TransferStatus::Completed;
    record
        .save(&record_dir)
        .map_err(|e| TransferError::other("saving final record", e))?;

    let _ = protocol_wire::write_receiver_message(
        &mut progress_send,
        &protocol_message::ReceiverMessage::TransferCompleted(
            protocol_message::TransferCompleted {
                session_id: session_id.clone(),
                snapshot: to_wire_snapshot(&final_snapshot),
            },
        ),
    )
    .await;

    // Finish progress stream
    let _ = progress_send.finish();

    let _ = send_transfer_result(
        &mut control_send,
        &session_id,
        protocol_message::TransferStatus::Ok,
    )
    .await;

    // Final wait for Sender to acknowledge our result
    await_final_sender_ack(&mut control_recv, &session_id).await;
    finish_control_stream(&mut control_send).await;
    emit_receiver_event(
        &event_tx,
        ReceiverEvent::TransferCompleted {
            session_id: session_id.clone(),
            snapshot: final_snapshot,
        },
    );
    emit_receiver_event(
        &event_tx,
        ReceiverEvent::Completed {
            session_id: session_id.clone(),
        },
    );
    Ok(TransferOutcome::Completed)
    }
    .await;

    // Terminal outcomes where the user clearly doesn't want to resume:
    // mark the record dir for deletion before the guard drops.  Any
    // `Err(_)` (including transient ConnectionClosed) leaves the guard
    // alone — the receiver-service startup sweep will GC it after the
    // TTL elapses if no retry comes through.
    if matches!(
        &outcome,
        Ok(TransferOutcome::Completed)
            | Ok(TransferOutcome::Cancelled(_))
            | Ok(TransferOutcome::Declined { .. })
    ) {
        record_guard.mark_for_delete();
    }

    outcome
}

enum HandshakeResult {
    Ok(
        iroh::endpoint::SendStream,
        iroh::endpoint::RecvStream,
        protocol_message::Hello,
        protocol_message::Offer,
    ),
    Cancelled(TransferOutcome),
}

/// Per-step timeout for the machine handshake (accept stream, read/write the
/// Hello). No human is involved at this point, so each step completes at
/// network speed; this only elapses when the connection never produces the
/// expected frame.
const HANDSHAKE_STEP_TIMEOUT: Duration = Duration::from_secs(30);

/// How long the receiver waits for the sender's Offer *after* the Hello
/// exchange has completed. The offer is machine-generated and sent
/// immediately, so this only elapses when the connection silently stalls
/// (e.g. a path that carried the tiny Hello frames but can't sustain the
/// larger Offer). On elapse the receiver surfaces a recoverable "sender
/// connected but never sent the offer" failure rather than hanging — and
/// because [`ReceiverEvent::SenderConnected`] already fired, the UI is on the
/// "connecting from <X>" screen and can flip to that error with the sender
/// named.
const OFFER_WAIT_TIMEOUT: Duration = Duration::from_secs(30);

async fn do_handshake(
    endpoint: &Endpoint,
    request: &ReceiverRequest,
    conn: &Connection,
    event_tx: &Option<mpsc::UnboundedSender<ReceiverEvent>>,
    cancel_rx: &mut watch::Receiver<bool>,
) -> Result<HandshakeResult> {
    tokio::select! {
        res = async {
            let (mut send, mut recv) = tokio::time::timeout(HANDSHAKE_STEP_TIMEOUT, conn.accept_bi())
                .await
                .map_err(|source| TransferError::other("handshake stream timeout", source))?
                .map_err(|source| TransferError::other("accepting bi-stream", source))?;
            let hello = match tokio::time::timeout(HANDSHAKE_STEP_TIMEOUT, protocol_wire::read_sender_message(&mut recv))
                .await
                .map_err(|_| TransferError::timeout("waiting for sender hello"))??
            {
                protocol_message::SenderMessage::Hello(h) => h,
                protocol_message::SenderMessage::Cancel(c) => {
                    return Ok(HandshakeResult::Cancelled(TransferOutcome::from_remote_cancel(c, "")?))
                }
                other => {
                    return Err(ProtocolError::unexpected_message_kind(
                        "sender handshake",
                        MessageKind::Hello,
                        other.kind(),
                    )
                    .into())
                }
            };
            protocol_wire::write_receiver_message(&mut send, &protocol_message::ReceiverMessage::Hello(protocol_message::Hello {
                version: protocol_message::PROTOCOL_VERSION,
                session_id: hello.session_id.clone(),
                identity: protocol_message::Identity {
                    role: protocol_message::TransferRole::Receiver,
                    endpoint_id: endpoint.addr().id,
                    device_name: request.device_name.clone(),
                    device_type: to_protocol_device_type(request.device_type),
                    web: false,
                    ephemeral: false,
                }
            })).await?;
            // The sender is now known (Hello carries its identity). Surface it
            // immediately so the receiver UI leaves the idle/QR screen for a
            // "connecting from <X>" screen while the Offer is in flight.
            emit_receiver_event(
                event_tx,
                ReceiverEvent::SenderConnected {
                    session_id: hello.session_id.clone(),
                    sender_device_name: hello.identity.device_name.clone(),
                    sender_device_type: to_local_device_type(hello.identity.device_type),
                    sender_endpoint_id: hello.identity.endpoint_id,
                },
            );
            let offer = match tokio::time::timeout(OFFER_WAIT_TIMEOUT, protocol_wire::read_sender_message(&mut recv))
                .await
                .map_err(|_| TransferError::timeout("sender connected but never sent the transfer offer"))??
            {
                protocol_message::SenderMessage::Offer(o) => o,
                protocol_message::SenderMessage::Cancel(c) => {
                    return Ok(HandshakeResult::Cancelled(TransferOutcome::from_remote_cancel(c, &hello.session_id)?))
                }
                other => {
                    return Err(ProtocolError::unexpected_message_kind(
                        "sender offer",
                        MessageKind::Offer,
                        other.kind(),
                    )
                    .into())
                }
            };
            Ok(HandshakeResult::Ok(send, recv, hello, offer))
        } => res,
        _ = wait_for_cancel(cancel_rx) => Ok(HandshakeResult::Cancelled(TransferOutcome::local_cancel(protocol_message::TransferRole::Receiver, protocol_message::CancelPhase::WaitingForDecision))),
        _ = conn.closed() => return Err(TransferError::connection_closed("during handshake")),
    }
}

async fn do_transfer(
    session_id: &str,
    plan: &TransferPlan,
    download: &mut BlobDownloadSession,
    progress_send: &mut iroh::endpoint::SendStream,
    control_recv: &mut iroh::endpoint::RecvStream,
    cancel_rx: &mut watch::Receiver<bool>,
    event_tx: &Option<mpsc::UnboundedSender<ReceiverEvent>>,
    record: &mut TransferRecord,
    record_dir: &Path,
    resume_from_bytes: u64,
) -> Result<(TransferOutcome, ProgressTracker)> {
    let mut tracker = ProgressTracker::new(plan.clone());
    tracker.set_bytes_transferred(resume_from_bytes, std::time::Instant::now());
    tracker.set_phase(TransferPhase::Transferring, std::time::Instant::now());
    emit_receiver_event(
        event_tx,
        ReceiverEvent::TransferStarted {
            session_id: session_id.to_owned(),
            plan: plan.clone(),
        },
    );

    let initial_snapshot = tracker.snapshot(std::time::Instant::now());
    let _ = protocol_wire::write_receiver_message(
        progress_send,
        &protocol_message::ReceiverMessage::TransferProgress(protocol_message::TransferProgress {
            session_id: session_id.to_owned(),
            snapshot: to_wire_snapshot(&initial_snapshot),
        }),
    )
    .await;

    loop {
        tokio::select! {
            item = download.events_mut().next() => match item {
                Some(BlobDownloadUpdate::Progress { bytes_received: offset }) => {
                    let now = std::time::Instant::now();
                    let bytes_received = (resume_from_bytes + offset).min(plan.total_bytes);
                    tracker.set_bytes_transferred(bytes_received, now);
                    if record.bytes_received != bytes_received {
                        record.bytes_received = bytes_received;
                        record
                            .save(record_dir)
                            .map_err(|e| TransferError::other("saving record progress", e))?;
                    }
                    let snapshot = tracker.snapshot(now);
                    emit_receiver_event(event_tx, ReceiverEvent::TransferProgress {
                        session_id: session_id.to_owned(),
                        snapshot: snapshot.clone(),
                    });
                    let _ = protocol_wire::write_receiver_message(
                        progress_send,
                        &protocol_message::ReceiverMessage::TransferProgress(protocol_message::TransferProgress {
                            session_id: session_id.to_owned(),
                            snapshot: to_wire_snapshot(&snapshot),
                        }),
                    ).await;
                }
                Some(BlobDownloadUpdate::Done) => {
                    return Ok((TransferOutcome::Completed, tracker));
                }
                None => {
                    download.abort();
                    return Err(TransferError::channel_closed("blob download stream"));
                }
                Some(BlobDownloadUpdate::Failed { error }) => {
                    download.abort();
                    return Err(error.into());
                }
            },
            msg = protocol_wire::read_sender_message(control_recv) => match msg? {
                protocol_message::SenderMessage::Cancel(c) => {
                    download.abort();
                    return Ok(TransferOutcome::from_remote_cancel(c, session_id).map(|outcome| (outcome, tracker))?);
                }
                other => {
                    return Err(ProtocolError::unexpected_message_kind(
                        "sender transfer",
                        MessageKind::Cancel,
                        other.kind(),
                    )
                    .into())
                }
            },
            _ = wait_for_cancel(cancel_rx) => {
                download.abort();
                return Ok((
                    TransferOutcome::local_cancel(
                        protocol_message::TransferRole::Receiver,
                        protocol_message::CancelPhase::Transferring,
                    ),
                    tracker,
                ))
            },
        }
    }
}

async fn abort_session(
    send: &mut iroh::endpoint::SendStream,
    session_id: &str,
    phase: protocol_message::CancelPhase,
) -> Result<TransferOutcome> {
    let outcome = TransferOutcome::local_cancel(protocol_message::TransferRole::Receiver, phase);
    if let TransferOutcome::Cancelled(c) = &outcome {
        let _ = send_receiver_cancel(send, session_id, c.by, c.phase, c.reason.clone()).await;
        finish_control_stream(send).await;
    }
    Ok(outcome)
}

pub async fn export_downloaded_collection(
    store: &FsStore,
    root_hash: iroh_blobs::Hash,
    expected_files: &[ExpectedTransferFile],
    record: &mut TransferRecord,
    record_dir: &std::path::Path,
    // Cumulative bytes exported so far.  Bumped after each file (and for
    // already-exported files on a resume) so the caller's progress
    // heartbeat can animate the finalize bar.  Pass a throwaway
    // `&AtomicU64::new(0)` when progress isn't observed.
    exported_bytes: &AtomicU64,
) -> Result<()> {
    let collection = Collection::load(root_hash, store.as_ref())
        .await
        .map_err(|source| TransferError::other("loading downloaded collection", source))?;
    let hashes: BTreeMap<_, _> = collection.into_iter().collect();
    let mut exported_total = 0_u64;
    for exp in expected_files {
        if record.exported_files.contains(&exp.path) {
            info!("skipping already exported file: {}", exp.path);
            exported_total = exported_total.saturating_add(exp.size);
            exported_bytes.store(exported_total, Ordering::Relaxed);
            continue;
        }

        let hash = *hashes.get(&exp.path).ok_or_else(|| {
            TransferError::other(
                "exporting downloaded collection",
                std::io::Error::other(format!("missing file in collection: {}", exp.path)),
            )
        })?;
        ensure_destination_available(&record.output_dir, &exp.destination).await?;
        if let Some(p) = exp.destination.parent() {
            fs::create_dir_all(p)
                .await
                .map_err(|source| TransferError::other("creating export directory", source))?;
        }
        store
            .export_with_opts(ExportOptions {
                hash,
                target: exp.destination.clone(),
                // `TryReference` moves the blob out of the store and
                // references it from the DB instead of copying every byte a
                // second time (`Copy` was the bulk of the receiver's
                // post-download lag — each byte hit the disk twice: once
                // into `<out>/.wisp/transfers/<hash>/store`, then again to
                // the destination).  The store lives on the same volume as
                // the destination, so this is a reflink/hardlink/rename and
                // finishes near-instantly.  iroh is free to fall back to a
                // copy for tiny/inline blobs or stores that can't reference,
                // so correctness is unchanged — only the common large-file
                // path gets faster.  Safe against the post-completion record
                // dir cleanup: the data already lives at the destination, so
                // deleting the store afterwards just drops the now-redundant
                // reference.
                mode: ExportMode::TryReference,
            })
            .finish()
            .await
            .map_err(|source| TransferError::other("exporting downloaded file", source))?;

        record.exported_files.insert(exp.path.clone());
        record
            .save(record_dir)
            .map_err(|e| TransferError::other("saving record during export", e))?;
        exported_total = exported_total.saturating_add(exp.size);
        exported_bytes.store(exported_total, Ordering::Relaxed);
    }
    Ok(())
}

async fn build_expected_files(
    manifest: &OfferManifest,
    out_dir: &Path,
    conflict_policy: ConflictPolicy,
    resume_record: Option<&TransferRecord>,
) -> Result<BTreeMap<String, ExpectedTransferFile>> {
    let mut expected = BTreeMap::new();
    for file in &manifest.files {
        let destination =
            resolve_expected_destination(out_dir, &file.path, conflict_policy, resume_record)
                .await?;
        expected.insert(
            file.path.clone(),
            ExpectedTransferFile {
                path: file.path.clone(),
                size: file.size,
                destination,
            },
        );
    }
    Ok(expected)
}

async fn resolve_expected_destination(
    out_dir: &Path,
    transfer_path: &str,
    conflict_policy: ConflictPolicy,
    resume_record: Option<&TransferRecord>,
) -> Result<PathBuf> {
    let destination = resolve_transfer_destination(out_dir, transfer_path)?;
    match ensure_destination_available(out_dir, &destination).await {
        Ok(()) => Ok(destination),
        Err(super::path::TransferPathError::DestinationExists { path })
            if resume_record
                .map(|record| record.exported_files.contains(transfer_path))
                .unwrap_or(false)
                && path == destination =>
        {
            Ok(destination)
        }
        Err(super::path::TransferPathError::DestinationExists { .. }) => match conflict_policy {
            ConflictPolicy::Reject => {
                Err(super::path::TransferPathError::DestinationExists { path: destination }.into())
            }
            ConflictPolicy::Rename => {
                let resolved = conflict_policy
                    .resolve(&destination)
                    .await
                    .map_err(|error| {
                        TransferError::other("resolving destination conflict", error)
                    })?;
                ensure_destination_available(out_dir, &resolved).await?;
                Ok(resolved)
            }
            ConflictPolicy::Overwrite => Ok(destination),
        },
        Err(error) => Err(error.into()),
    }
}

fn load_resume_record(
    record_dir: &Path,
    collection_hash: iroh_blobs::Hash,
    manifest: &protocol_message::TransferManifest,
) -> Option<TransferRecord> {
    let record = TransferRecord::load(record_dir).ok()?;
    if record.collection_hash == collection_hash && record.manifest == *manifest {
        Some(record)
    } else {
        None
    }
}

fn resume_offset_for_record(record: &TransferRecord, total_bytes: u64) -> u64 {
    match record.status {
        TransferStatus::DataComplete | TransferStatus::Finalizing | TransferStatus::Completed => {
            total_bytes
        }
        TransferStatus::Transferring | TransferStatus::Paused | TransferStatus::Failed => {
            record.bytes_received.min(total_bytes)
        }
    }
}

enum SenderBlobTicketRead {
    Ticket(protocol_message::BlobTicketMessage),
    Cancel(protocol_message::Cancel),
}

async fn read_sender_blob_ticket_message<R>(
    recv: &mut R,
    session_id: &str,
) -> Result<SenderBlobTicketRead>
where
    R: AsyncRead + Unpin,
{
    match protocol_wire::read_sender_message(recv).await? {
        protocol_message::SenderMessage::BlobTicket(msg) if msg.session_id == session_id => {
            Ok(SenderBlobTicketRead::Ticket(msg))
        }
        protocol_message::SenderMessage::BlobTicket(msg) => {
            Err(ProtocolError::session_id_mismatch(session_id, msg.session_id).into())
        }
        protocol_message::SenderMessage::Cancel(cancel) => Ok(SenderBlobTicketRead::Cancel(cancel)),
        other => Err(ProtocolError::unexpected_message_kind(
            "sender ticket",
            MessageKind::BlobTicket,
            other.kind(),
        )
        .into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blobs::send::PreparedStore;
    use crate::fs_plan::ConflictPolicy;
    use crate::protocol::message::{
        BlobTicketMessage, ManifestItem, SenderMessage, TransferAck, TransferManifest,
    };
    use crate::protocol::wire::{read_sender_message, write_sender_message};
    use tokio::io::duplex;

    fn manifest_with(paths: &[(&str, u64)]) -> OfferManifest {
        OfferManifest {
            files: paths
                .iter()
                .map(|(path, size)| crate::rendezvous::OfferFile {
                    path: (*path).to_owned(),
                    size: *size,
                })
                .collect(),
            collection_hash: Some([7u8; 32].into()),
            file_count: paths.len() as u64,
            total_size: paths.iter().map(|(_, size)| *size).sum(),
        }
    }

    fn record_with_exported(paths: &[&str]) -> TransferRecord {
        let mut record = TransferRecord::new(
            [7u8; 32].into(),
            PathBuf::from("/tmp/out"),
            ConflictPolicy::Rename,
            TransferManifest {
                items: vec![ManifestItem::File {
                    path: "already.txt".to_owned(),
                    size: 5,
                }],
            },
        );
        record.status = TransferStatus::Finalizing;
        record.exported_files = paths.iter().map(|path| (*path).to_owned()).collect();
        record
    }

    /// Regression: `run_with_progress_heartbeat` must keep pumping
    /// TransferProgress frames onto the progress stream while the wrapped
    /// future is pending, so iroh's QUIC keepalive isn't the only thing
    /// holding the path up during a slow `export_downloaded_collection`.
    /// Without these heartbeats the sender often errors out with a
    /// generic "Protocol mismatch" the moment it tries to read the
    /// post-export TransferResult on a flaky mobile network — see the
    /// audit doc entry "Sender errors with Protocol mismatch while
    /// receiver completes".
    #[tokio::test]
    async fn run_with_progress_heartbeat_emits_periodic_frames_during_slow_future() {
        use crate::protocol::message::{ReceiverMessage, TransferProgress};
        use crate::protocol::wire::read_receiver_message;
        use crate::transfer::types::TransferPlan;

        let plan = TransferPlan::try_new(
            "session-1".to_owned(),
            vec![super::super::types::TransferPlanFile {
                id: 0,
                path: "f.bin".to_owned(),
                size: 100,
            }],
        )
        .unwrap();
        let mut tracker = ProgressTracker::new(plan);
        tracker.set_phase(TransferPhase::Finalizing, std::time::Instant::now());

        let (mut sender_recv, mut receiver_send) = duplex(8192);
        let (_cancel_tx, mut cancel_rx) = watch::channel(false);

        // Fake export that takes ~250 ms and reports half its bytes
        // exported partway through, so we can assert the local frames carry
        // the climbing exported-byte count.  At a 50 ms heartbeat that
        // gives us at least 4 ticks (with set_missed_tick_behavior::Skip
        // we'll see fewer if the runtime stalls — pin the assertion at
        // ≥2 to keep CI green on slow runners).
        let exported_bytes = AtomicU64::new(0);
        let slow_export = async {
            tokio::time::sleep(Duration::from_millis(120)).await;
            exported_bytes.store(50, Ordering::Relaxed);
            tokio::time::sleep(Duration::from_millis(130)).await;
            exported_bytes.store(100, Ordering::Relaxed);
            Result::Ok(())
        };

        let (event_tx, mut event_rx) = mpsc::unbounded_channel();
        let outcome = run_with_progress_heartbeat(
            slow_export,
            &mut receiver_send,
            "session-1",
            &mut tracker,
            &mut cancel_rx,
            Duration::from_millis(50),
            &Some(event_tx),
            &exported_bytes,
        )
        .await;

        // Future succeeded → outcome is Completed.
        assert!(
            matches!(outcome, HeartbeatOutcome::Completed),
            "{outcome:?}"
        );

        // Close the writer so the reader sees EOF and the read loop
        // terminates instead of blocking.
        drop(receiver_send);

        let mut heartbeats = 0u32;
        while let Ok(msg) = read_receiver_message(&mut sender_recv).await {
            match msg {
                ReceiverMessage::TransferProgress(TransferProgress { snapshot, .. }) => {
                    assert_eq!(snapshot.phase, TransferPhase::Finalizing);
                    heartbeats += 1;
                }
                other => panic!(
                    "expected only TransferProgress heartbeats, got {:?}",
                    other.kind()
                ),
            }
        }
        assert!(
            heartbeats >= 2,
            "expected at least 2 heartbeat frames during the 250 ms export, got {heartbeats}",
        );

        // Local frames must mirror the climbing exported-byte count so the
        // receiver's save bar animates instead of freezing.  The wire frames
        // above stay pinned at the canonical snapshot (keepalive), but the
        // local ones track `exported_bytes`.
        let mut local_bytes = Vec::new();
        while let Ok(event) = event_rx.try_recv() {
            match event {
                ReceiverEvent::TransferProgress { snapshot, .. } => {
                    assert_eq!(snapshot.phase, TransferPhase::Finalizing);
                    local_bytes.push(snapshot.bytes_transferred);
                }
                other => panic!("expected only local TransferProgress, got {other:?}"),
            }
        }
        assert!(
            local_bytes.iter().any(|&b| b > 0),
            "expected at least one local frame with exported bytes > 0, got {local_bytes:?}",
        );
        assert!(
            local_bytes.windows(2).all(|w| w[0] <= w[1]),
            "local exported-byte count must be monotonic, got {local_bytes:?}",
        );
    }

    #[test]
    fn resume_offset_uses_persisted_progress_for_incomplete_records() {
        let mut record = record_with_exported(&[]);
        record.status = TransferStatus::Transferring;
        record.bytes_received = 42;

        assert_eq!(resume_offset_for_record(&record, 100), 42);
    }

    #[test]
    fn resume_offset_clamps_progress_and_treats_data_complete_as_total() {
        let mut record = record_with_exported(&[]);
        record.status = TransferStatus::Paused;
        record.bytes_received = 120;
        assert_eq!(resume_offset_for_record(&record, 100), 100);

        record.status = TransferStatus::DataComplete;
        record.bytes_received = 40;
        assert_eq!(resume_offset_for_record(&record, 100), 100);
    }

    #[tokio::test]
    async fn resumed_export_allows_destinations_for_already_exported_files() {
        let out = tempfile::tempdir().unwrap();
        std::fs::write(out.path().join("already.txt"), b"done").unwrap();
        let manifest = manifest_with(&[("already.txt", 4), ("remaining.txt", 9)]);
        let record = record_with_exported(&["already.txt"]);

        let expected =
            build_expected_files(&manifest, out.path(), ConflictPolicy::Rename, Some(&record))
                .await
                .unwrap();

        assert_eq!(
            expected
                .get("already.txt")
                .map(|file| file.destination.as_path()),
            Some(out.path().join("already.txt").as_path())
        );
        assert_eq!(
            expected
                .get("remaining.txt")
                .map(|file| file.destination.as_path()),
            Some(out.path().join("remaining.txt").as_path())
        );
    }

    #[tokio::test]
    async fn rename_policy_renames_existing_destinations() {
        let out = tempfile::tempdir().unwrap();
        std::fs::write(out.path().join("report.txt"), b"existing").unwrap();
        let manifest = manifest_with(&[("report.txt", 4)]);

        let expected = build_expected_files(&manifest, out.path(), ConflictPolicy::Rename, None)
            .await
            .unwrap();

        assert_eq!(
            expected
                .get("report.txt")
                .map(|file| file.destination.as_path()),
            Some(out.path().join("report (1).txt").as_path())
        );
    }

    #[tokio::test]
    async fn rename_policy_keeps_nested_parent_directory() {
        let out = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(out.path().join("reports")).unwrap();
        std::fs::write(out.path().join("reports/report.txt"), b"existing").unwrap();
        let manifest = manifest_with(&[("reports/report.txt", 4)]);

        let expected = build_expected_files(&manifest, out.path(), ConflictPolicy::Rename, None)
            .await
            .unwrap();

        assert_eq!(
            expected
                .get("reports/report.txt")
                .map(|file| file.destination.as_path()),
            Some(out.path().join("reports/report (1).txt").as_path())
        );
    }

    #[tokio::test]
    async fn reject_policy_fails_existing_destinations() {
        let out = tempfile::tempdir().unwrap();
        std::fs::write(out.path().join("report.txt"), b"existing").unwrap();
        let manifest = manifest_with(&[("report.txt", 4)]);

        let err = build_expected_files(&manifest, out.path(), ConflictPolicy::Reject, None)
            .await
            .expect_err("expected reject policy to fail");

        assert!(
            err.to_string().contains("destination already exists"),
            "unexpected error: {err:#}"
        );
    }

    #[tokio::test]
    async fn resume_control_path_consumes_blob_ticket_before_waiting_for_ack() {
        let (local, remote) = duplex(4096);
        let (mut local_read, _) = tokio::io::split(local);
        let (_, mut remote_write) = tokio::io::split(remote);

        tokio::spawn(async move {
            write_sender_message(
                &mut remote_write,
                &SenderMessage::BlobTicket(BlobTicketMessage {
                    session_id: "session-1".to_owned(),
                    ticket: "ticket-text".to_owned(),
                }),
            )
            .await
            .unwrap();
            write_sender_message(
                &mut remote_write,
                &SenderMessage::TransferAck(TransferAck {
                    session_id: "session-1".to_owned(),
                }),
            )
            .await
            .unwrap();
        });

        let ticket = match read_sender_blob_ticket_message(&mut local_read, "session-1")
            .await
            .unwrap()
        {
            SenderBlobTicketRead::Ticket(ticket) => ticket,
            SenderBlobTicketRead::Cancel(_) => panic!("expected blob ticket"),
        };
        assert_eq!(ticket.ticket, "ticket-text");

        let next = read_sender_message(&mut local_read).await.unwrap();
        assert!(matches!(next, SenderMessage::TransferAck(_)));
    }

    #[tokio::test]
    async fn final_sender_ack_is_optional_after_successful_receive() {
        let (local, remote) = duplex(4096);
        drop(remote);
        let (mut control_recv, _) = tokio::io::split(local);

        await_final_sender_ack(&mut control_recv, "session-1").await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn export_downloaded_collection_rejects_symlinked_parents() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let source_root = temp.path().join("source");
        let download_root = temp.path().join("downloads");
        let escape_root = temp.path().join("escape");
        let record_dir = temp.path().join("record");
        let source_link = source_root.join("link");
        let expected_destination = download_root.join("link/owned.txt");

        std::fs::create_dir_all(&source_link).unwrap();
        std::fs::create_dir_all(&download_root).unwrap();
        std::fs::create_dir_all(&escape_root).unwrap();
        std::fs::create_dir_all(&record_dir).unwrap();
        std::fs::write(source_link.join("owned.txt"), b"owned").unwrap();
        symlink(&escape_root, download_root.join("link")).unwrap();

        let prepared = PreparedStore::prepare(&source_link, vec![source_link.clone()])
            .await
            .unwrap();
        let mut record = TransferRecord::new(
            prepared.collection_hash(),
            download_root.clone(),
            ConflictPolicy::Rename,
            prepared.manifest(),
        );
        let expected = vec![ExpectedTransferFile {
            path: "link/owned.txt".to_owned(),
            size: 5,
            destination: expected_destination,
        }];

        let err = export_downloaded_collection(
            prepared.store(),
            prepared.collection_hash(),
            &expected,
            &mut record,
            &record_dir,
            &AtomicU64::new(0),
        )
        .await
        .unwrap_err();

        assert!(format!("{err:#}").contains("symbolic link"));
        assert!(!escape_root.join("owned.txt").exists());
    }

    #[tokio::test]
    async fn cleanup_transfer_workspace_removes_existing_directory() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path().join("workspace");
        tokio::fs::create_dir_all(&workspace).await.unwrap();
        tokio::fs::write(workspace.join("record.json"), b"{}")
            .await
            .unwrap();

        cleanup_transfer_workspace(&workspace).await;

        assert!(!workspace.exists());
    }

    #[tokio::test]
    async fn cleanup_transfer_workspace_ignores_missing_directory() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path().join("workspace");

        cleanup_transfer_workspace(&workspace).await;

        assert!(!workspace.exists());
    }
}

fn build_expected_transfer_files(
    manifest: &OfferManifest,
    mut expected_files: BTreeMap<String, ExpectedTransferFile>,
) -> Result<Vec<ExpectedTransferFile>> {
    manifest
        .files
        .iter()
        .map(|f| {
            expected_files.remove(&f.path).ok_or_else(|| {
                TransferError::other(
                    "building expected transfer files",
                    std::io::Error::other(format!("missing expected file for {}", f.path)),
                )
            })
        })
        .collect()
}

fn to_protocol_device_type(dt: crate::protocol::DeviceType) -> protocol_message::DeviceType {
    match dt {
        crate::protocol::DeviceType::Phone => protocol_message::DeviceType::Phone,
        crate::protocol::DeviceType::Laptop => protocol_message::DeviceType::Laptop,
    }
}

fn to_local_device_type(dt: protocol_message::DeviceType) -> crate::protocol::DeviceType {
    match dt {
        protocol_message::DeviceType::Phone => crate::protocol::DeviceType::Phone,
        protocol_message::DeviceType::Laptop => crate::protocol::DeviceType::Laptop,
    }
}

fn to_offer_manifest(offer: &protocol_message::Offer) -> OfferManifest {
    OfferManifest {
        files: offer
            .manifest
            .items
            .iter()
            .map(|item| match item {
                protocol_message::ManifestItem::File { path, size } => {
                    crate::rendezvous::OfferFile {
                        path: path.clone(),
                        size: *size,
                    }
                }
            })
            .collect(),
        collection_hash: Some(offer.collection_hash),
        file_count: offer.manifest.count() as u64,
        total_size: offer.manifest.total_size(),
    }
}

fn to_wire_snapshot(snapshot: &TransferSnapshot) -> protocol_message::TransferProgressPayload {
    protocol_message::TransferProgressPayload {
        phase: snapshot.phase,
        completed_files: snapshot.completed_files,
        total_files: snapshot.total_files,
        bytes_transferred: snapshot.bytes_transferred,
        total_bytes: snapshot.total_bytes,
        active_file_id: snapshot.active_file_id,
        active_file_bytes: snapshot.active_file_bytes,
    }
}

fn emit_receiver_event(tx: &Option<mpsc::UnboundedSender<ReceiverEvent>>, event: ReceiverEvent) {
    if let Some(tx) = tx {
        let _ = tx.send(event);
    }
}

async fn send_receiver_cancel(
    send: &mut iroh::endpoint::SendStream,
    session_id: &str,
    by: protocol_message::TransferRole,
    phase: protocol_message::CancelPhase,
    reason: String,
) -> Result<()> {
    protocol_wire::write_receiver_message(
        send,
        &protocol_message::ReceiverMessage::Cancel(protocol_message::Cancel {
            session_id: session_id.to_owned(),
            by,
            phase,
            reason,
        }),
    )
    .await
    .map_err(Into::into)
}

async fn send_transfer_result(
    send: &mut iroh::endpoint::SendStream,
    session_id: &str,
    status: protocol_message::TransferStatus,
) -> Result<()> {
    protocol_wire::write_receiver_message(
        send,
        &protocol_message::ReceiverMessage::TransferResult(protocol_message::TransferResult {
            session_id: session_id.to_owned(),
            status,
        }),
    )
    .await
    .map_err(Into::into)
}

async fn send_receiver_decline(
    send: &mut iroh::endpoint::SendStream,
    session_id: &str,
    reason: String,
) -> Result<()> {
    protocol_wire::write_receiver_message(
        send,
        &protocol_message::ReceiverMessage::Decline(protocol_message::Decline {
            session_id: session_id.to_owned(),
            reason,
        }),
    )
    .await
    .map_err(Into::into)
}

/// How often the receiver pumps a Finalizing TransferProgress frame onto
/// the progress stream while `export_downloaded_collection` runs.
///
/// Lower bound: must be safely below iroh's QUIC keepalive idle timeout
/// (6 000 ms — see `crates/app/src/quic_keepalive.rs`) so a single
/// dropped ping doesn't kill the path during a slow export.  2 s gives
/// plenty of headroom while keeping the UI's Finalizing indicator
/// responsive.
pub(super) const EXPORT_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(2);

/// Result of running a long-running future under
/// [`run_with_progress_heartbeat`].
#[derive(Debug)]
pub(super) enum HeartbeatOutcome {
    /// The future resolved successfully.
    Completed,
    /// The future returned an error — propagated as-is.
    Failed(TransferError),
    /// `cancel_rx` flipped to `true` before the future resolved.
    Cancelled,
}

/// Drives a long-running future to completion while pumping periodic
/// `TransferProgress` frames onto the receiver's progress stream.
///
/// This exists to keep application traffic flowing during the receiver's
/// Phase-5 export window.  Without it the export window has no app data
/// on either stream, so iroh's tight QUIC keepalive (≤6 s idle / ≤5 s
/// ping) is the only thing holding the path up — a single lost ping on
/// flaky mobile networks then tears the path and the sender hits a
/// generic "Protocol mismatch" the moment it tries to read the
/// post-export `TransferResult`.  See `docs/upstream-bug-audit.md` for
/// the full root-cause analysis.
///
/// Heartbeat writes use `let _ = ...` semantics — a dropped frame is
/// purely a missed keepalive backup; the actual completion outcome is
/// still determined by `fut`.
#[allow(clippy::too_many_arguments)]
pub(super) async fn run_with_progress_heartbeat<F, W>(
    fut: F,
    progress_send: &mut W,
    session_id: &str,
    tracker: &mut ProgressTracker,
    cancel_rx: &mut watch::Receiver<bool>,
    interval: Duration,
    // Local receiver-event sink: each tick also emits a `TransferProgress`
    // carrying `exported_bytes` so the receiver's own UI animates the
    // finalize/save bar instead of freezing at 100% during the export.
    event_tx: &Option<mpsc::UnboundedSender<ReceiverEvent>>,
    // Cumulative bytes exported by the wrapped future so far (see
    // `export_downloaded_collection`).
    exported_bytes: &AtomicU64,
) -> HeartbeatOutcome
where
    F: std::future::Future<Output = Result<()>>,
    W: tokio::io::AsyncWrite + Unpin,
{
    let mut fut = Box::pin(fut);
    let mut heartbeat = tokio::time::interval(interval);
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    // Skip the initial immediate tick — callers normally write a
    // finalizing snapshot just before invoking this helper, so the wire
    // already has a fresh frame when we enter the loop.
    heartbeat.tick().await;

    loop {
        tokio::select! {
            res = &mut fut => {
                return match res {
                    Ok(()) => HeartbeatOutcome::Completed,
                    Err(error) => HeartbeatOutcome::Failed(error),
                };
            }
            _ = wait_for_cancel(cancel_rx) => {
                return HeartbeatOutcome::Cancelled;
            }
            _ = heartbeat.tick() => {
                let snapshot = tracker.snapshot(std::time::Instant::now());
                // Wire frame: canonical finalizing snapshot (bytes == total)
                // — purely a QUIC keepalive backup for the sender, which
                // already shows 100% and must not see the bar walk backwards.
                let _ = protocol_wire::write_receiver_message(
                    progress_send,
                    &protocol_message::ReceiverMessage::TransferProgress(
                        protocol_message::TransferProgress {
                            session_id: session_id.to_owned(),
                            snapshot: to_wire_snapshot(&snapshot),
                        },
                    ),
                )
                .await;
                // Local frame: animate the receiver's save-to-disk bar with
                // the real exported-byte count.
                let exported = exported_bytes.load(Ordering::Relaxed);
                emit_receiver_event(
                    event_tx,
                    ReceiverEvent::TransferProgress {
                        session_id: session_id.to_owned(),
                        snapshot: TransferSnapshot {
                            bytes_transferred: exported.min(snapshot.total_bytes),
                            ..snapshot
                        },
                    },
                );
            }
        }
    }
}

async fn await_final_sender_ack<R>(recv: &mut R, session_id: &str)
where
    R: AsyncRead + Unpin,
{
    match protocol_wire::read_sender_message(recv).await {
        Ok(protocol_message::SenderMessage::TransferAck(msg)) if msg.session_id == session_id => {}
        Ok(protocol_message::SenderMessage::TransferAck(msg)) => {
            warn!(
                expected_session_id = %session_id,
                actual_session_id = %msg.session_id,
                "ignoring unexpected final sender ack"
            );
        }
        Ok(other) => {
            warn!(
                expected_session_id = %session_id,
                message_kind = ?other.kind(),
                "ignoring unexpected final sender message"
            );
        }
        Err(error) => {
            warn!(
                expected_session_id = %session_id,
                error = %error,
                "missing final sender ack"
            );
        }
    }
}

async fn finish_control_stream(send: &mut iroh::endpoint::SendStream) {
    let _ = send.finish();
    let _ = tokio::time::timeout(Duration::from_secs(2), send.stopped()).await;
}

async fn cleanup_transfer_workspace(record_dir: &Path) {
    match fs::remove_dir_all(record_dir).await {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => {
            warn!(
                path = %record_dir.display(),
                error = %err,
                "failed to clean up transfer workspace"
            );
        }
    }
}

pub async fn bind_endpoint() -> Result<Endpoint> {
    iroh::Endpoint::builder(iroh::endpoint::presets::N0)
        .alpns(vec![ALPN.to_vec(), BLOBS_ALPN.to_vec()])
        .relay_mode(iroh::RelayMode::Default)
        .bind()
        .await
        .map_err(|source| TransferError::other("binding iroh endpoint", source))
}
