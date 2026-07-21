#![allow(dead_code)]

use iroh::{
    Endpoint, EndpointAddr, EndpointId,
    endpoint::{Connection, ConnectionInfo},
};
use rand::random;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::{mpsc, oneshot, watch};
use tokio::time::Duration;
use tokio_stream::wrappers::UnboundedReceiverStream;
use tracing::{info, instrument};

use crate::{
    blobs::send::{BlobService, BlobServingStrategy, PreparedStore},
    protocol::message::{DeviceType, INLINE_TEXT_MAX_BYTES, MessageKind},
    protocol::wire as protocol_wire,
    protocol::{ALPN, ProtocolError},
    protocol::{message as protocol_message, send as protocol_sender},
};

use super::error::{Result as TransferResult, TransferError};
use super::path::ScratchDir;
use super::progress::SpeedCalculator;
use super::types::{
    TransferOutcome, TransferPhase, TransferPlan, TransferSnapshot, wait_for_cancel,
};

type Result<T> = TransferResult<T>;

/// Timeout for the machine-to-machine handshake — open the bi-stream, exchange
/// hellos, and put the offer on the wire.  No human is in this loop, so it
/// stays tight.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(30);

/// How long the sender waits for the receiver's accept/decline once the offer
/// is delivered.  This is a human-in-the-loop decision, so it must outlast the
/// receiver's own 120s decision window (see
/// `crates/core/src/transfer/receiver.rs`) — that way the receiver-side timer
/// is the one that fires first.  QUIC keepalive holds the path up while we
/// wait.  Folding this into [`HANDSHAKE_TIMEOUT`] was the "shared clipboard
/// auto-closes after a few seconds" bug: a slow tap dropped the connection and
/// the receiver reported "connection closed before receiver decision".
const DECISION_WAIT: Duration = Duration::from_secs(130);

/// The peer dial is retried a few times. Over a freshly-established link — most
/// notably the USB tunnel, where iroh needs a moment to bind and advertise the
/// new `10.42.0.x` interface after the VPN comes up — the first attempt can
/// race ahead of the peer being reachable. A bounded retry lets that transient
/// miss self-heal instead of surfacing as a connect failure the user has to
/// manually retry. Each attempt is capped so a genuinely dead address fails
/// fast (and frees the next attempt) rather than hanging on iroh's own timeout;
/// the whole loop honours cancellation between and during attempts.
const CONNECT_ATTEMPTS: usize = 4;
const CONNECT_ATTEMPT_TIMEOUT: Duration = Duration::from_secs(6);
const CONNECT_RETRY_DELAY: Duration = Duration::from_millis(400);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendRequest {
    pub peer_endpoint_addr: EndpointAddr,
    pub peer_endpoint_id: EndpointId,
    pub files: Vec<std::path::PathBuf>,
    /// Optional plain-text payload for a text-only send.  When set, `files`
    /// is ignored: text at or below [`INLINE_TEXT_MAX_BYTES`] rides inline on
    /// the control stream (no blobs); larger text falls back to a synthetic
    /// `message.txt` sent through the normal blob pipeline.
    pub inline_text: Option<String>,
}

#[derive(Debug)]
pub enum SenderEvent {
    Connecting {
        session_id: String,
        peer_endpoint_id: EndpointId,
        prepared_plan: TransferPlan,
    },
    WaitingForDecision {
        session_id: String,
        receiver_device_name: String,
        receiver_device_type: DeviceType,
        receiver_endpoint_id: EndpointId,
        receiver_web: bool,
        receiver_ephemeral: bool,
        prepared_plan: TransferPlan,
    },
    Accepted {
        session_id: String,
        receiver_device_name: String,
        receiver_device_type: DeviceType,
        receiver_endpoint_id: EndpointId,
        receiver_web: bool,
        receiver_ephemeral: bool,
        prepared_plan: TransferPlan,
    },
    Declined {
        session_id: String,
        reason: String,
        prepared_plan: TransferPlan,
    },
    Failed {
        session_id: String,
        error: TransferError,
        prepared_plan: TransferPlan,
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
}

pub type SenderEventStream = UnboundedReceiverStream<SenderEvent>;

#[derive(Debug)]
pub struct SenderRun {
    pub events: SenderEventStream,
    pub cancel_tx: watch::Sender<bool>,
    pub outcome_rx: oneshot::Receiver<TransferResult<TransferOutcome>>,
    /// Resolves once the outbound connection is established, with a weak handle
    /// for reading the *selected* connection path (authoritative, vs the
    /// endpoint address book). Never resolves if the dial is cancelled/fails
    /// before connecting — callers should treat that as "no path info".
    pub conn_info_rx: oneshot::Receiver<ConnectionInfo>,
}

impl SenderRun {
    pub fn into_parts(
        self,
    ) -> (
        SenderEventStream,
        watch::Sender<bool>,
        oneshot::Receiver<TransferResult<TransferOutcome>>,
        oneshot::Receiver<ConnectionInfo>,
    ) {
        (
            self.events,
            self.cancel_tx,
            self.outcome_rx,
            self.conn_info_rx,
        )
    }
}

#[derive(Debug, Clone)]
struct SenderEventSink {
    session_id: String,
    tx: Option<mpsc::UnboundedSender<SenderEvent>>,
}

impl SenderEventSink {
    fn new(session_id: String, tx: Option<mpsc::UnboundedSender<SenderEvent>>) -> Self {
        Self { session_id, tx }
    }
    fn emit(&self, e: SenderEvent) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(e);
        }
    }
    fn fail(&self, error: TransferError) {
        self.emit(SenderEvent::Failed {
            session_id: self.session_id.clone(),
            error,
            prepared_plan: TransferPlan {
                session_id: self.session_id.clone(),
                total_files: 0,
                total_bytes: 0,
                files: Vec::new(),
            },
        });
    }
}

pub struct Sender {
    endpoint: Endpoint,
    session_id: String,
    identity: protocol_message::Identity,
    request: SendRequest,
    blob_strategy: BlobServingStrategy,
}

impl Sender {
    pub fn new(
        endpoint: Endpoint,
        identity: protocol_message::Identity,
        request: SendRequest,
    ) -> Self {
        Self {
            endpoint,
            identity,
            session_id: format!("{:016x}", random::<u64>()),
            request,
            blob_strategy: BlobServingStrategy::Internal,
        }
    }

    /// Override the default Internal-router blob serving strategy.  See
    /// [`BlobServingStrategy::External`] for when this is needed (sharing
    /// the receiver-service endpoint to avoid relay duplicate-id).
    pub fn with_blob_strategy(mut self, strategy: BlobServingStrategy) -> Self {
        self.blob_strategy = strategy;
        self
    }

    pub fn run_with_events(self) -> SenderRun
    where
        Self: Send + 'static,
    {
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let (outcome_tx, outcome_rx) = oneshot::channel();
        let (cancel_tx, cancel_rx) = watch::channel(false);
        let (conn_info_tx, conn_info_rx) = oneshot::channel();
        let events = SenderEventSink::new(self.session_id.clone(), Some(event_tx));

        let Sender {
            endpoint,
            session_id,
            identity,
            request,
            blob_strategy,
        } = self;

        tokio::spawn(async move {
            let session = SenderSession {
                endpoint,
                session_id,
                identity,
                request,
                events: events.clone(),
                blob_strategy,
                conn_info_tx: Some(conn_info_tx),
            };
            let outcome = session.run(cancel_rx).await;
            let _ = outcome_tx.send(outcome);
        });

        SenderRun {
            events: UnboundedReceiverStream::new(event_rx),
            cancel_tx,
            outcome_rx,
            conn_info_rx,
        }
    }
}

struct SenderSession {
    endpoint: Endpoint,
    session_id: String,
    identity: protocol_message::Identity,
    request: SendRequest,
    events: SenderEventSink,
    blob_strategy: BlobServingStrategy,
    conn_info_tx: Option<oneshot::Sender<ConnectionInfo>>,
}

impl SenderSession {
    #[instrument(skip_all, fields(session_id = %self.session_id, peer = %self.request.peer_endpoint_id))]
    async fn run(mut self, mut cancel_rx: watch::Receiver<bool>) -> Result<TransferOutcome> {
        let scratch = ScratchDir::new("wisp-send", &self.session_id).await?;

        // Decide how the payload travels.  Short text rides inline on the
        // control stream (no blobs); larger text falls back to a synthetic
        // `message.txt`; everything else is the regular file path.
        let (manifest, collection_hash, prepared_plan, offer_inline_text, prepared) =
            match self.request.inline_text.clone() {
                Some(text) if text.len() <= INLINE_TEXT_MAX_BYTES => {
                    let prepared_plan = synthetic_text_plan(&self.session_id, text.len() as u64);
                    (
                        protocol_message::TransferManifest { items: Vec::new() },
                        iroh_blobs::Hash::from([0u8; 32]),
                        prepared_plan,
                        Some(text),
                        None,
                    )
                }
                Some(text) => {
                    // Over the inline cap: spill to a scratch `message.txt` and
                    // hand it to the normal blob pipeline as a single file.
                    let text_path = scratch.path.join("message.txt");
                    tokio::fs::write(&text_path, text.as_bytes())
                        .await
                        .map_err(|source| {
                            TransferError::other("writing oversized inline text to scratch", source)
                        })?;
                    let prepared = PreparedStore::prepare(&scratch.path, vec![text_path]).await?;
                    let prepared_plan = build_prepared_plan(&self.session_id, &prepared)?;
                    let manifest = prepared.manifest();
                    let collection_hash = prepared.collection_hash();
                    (
                        manifest,
                        collection_hash,
                        prepared_plan,
                        None,
                        Some(prepared),
                    )
                }
                None => {
                    let prepared =
                        PreparedStore::prepare(&scratch.path, self.request.files.clone()).await?;
                    let prepared_plan = build_prepared_plan(&self.session_id, &prepared)?;
                    let manifest = prepared.manifest();
                    let collection_hash = prepared.collection_hash();
                    (
                        manifest,
                        collection_hash,
                        prepared_plan,
                        None,
                        Some(prepared),
                    )
                }
            };

        info!(
            session_id = %self.session_id,
            collection_hash = %collection_hash,
            file_count = manifest.count(),
            total_size = manifest.total_size(),
            inline_text = offer_inline_text.is_some(),
            "prepared manifest"
        );

        self.events.emit(SenderEvent::Connecting {
            session_id: self.session_id.clone(),
            peer_endpoint_id: self.request.peer_endpoint_id,
            prepared_plan: prepared_plan.clone(),
        });
        let connection = match self.connect_with_retry(&mut cancel_rx).await? {
            Some(conn) => conn,
            // Cancelled while dialing — no connection was ever opened.
            None => {
                return Ok(TransferOutcome::local_cancel(
                    protocol_message::TransferRole::Sender,
                    protocol_message::CancelPhase::WaitingForDecision,
                ));
            }
        };

        // Hand the app a weak handle to read the *selected* connection path
        // (the authoritative carrying path) for the connection-path badge.
        if let Some(tx) = self.conn_info_tx.take() {
            let _ = tx.send(connection.to_info());
        }

        // --- Handshake ---
        let handshake_res = do_handshake(
            &self.session_id,
            &self.identity,
            manifest,
            collection_hash,
            offer_inline_text.clone(),
            &connection,
            &mut cancel_rx,
            &self.events,
            prepared_plan.clone(),
        )
        .await?;
        let (mut control_send, mut control_recv, outcome) = match handshake_res {
            HandshakeResult::Ok(s, r, o) => (s, r, o),
            HandshakeResult::Cancelled(outcome) => {
                // Notify the peer immediately: a receiver still blocked reading
                // our Hello/Offer sees the disconnect in ~1 RTT instead of
                // waiting out its 30s handshake / QUIC idle timeout. When the
                // control stream was healthy do_handshake already wrote a Cancel
                // frame; this close also covers a wedged offer stream.
                close_connection_gracefully(&self.endpoint, &connection, &self.blob_strategy).await;
                return Ok(outcome);
            }
        };

        match outcome {
            protocol_sender::SenderControlOutcome::Accepted(peer) => {
                self.events.emit(SenderEvent::Accepted {
                    session_id: self.session_id.clone(),
                    receiver_device_name: peer.identity.device_name.clone(),
                    receiver_device_type: peer.identity.device_type,
                    receiver_endpoint_id: peer.identity.endpoint_id,
                    receiver_web: peer.identity.web,
                    receiver_ephemeral: peer.identity.ephemeral,
                    prepared_plan: prepared_plan.clone(),
                });
            }
            protocol_sender::SenderControlOutcome::Declined(declined) => {
                self.events.emit(SenderEvent::Declined {
                    session_id: self.session_id.clone(),
                    reason: declined.reason,
                    prepared_plan: prepared_plan.clone(),
                });
                return Ok(TransferOutcome::Declined {
                    reason: "receiver declined".to_owned(),
                });
            }
        }

        // --- Inline text: no blobs.  The text already reached the receiver in
        // the offer, so wrap up the control exchange and finish. ---
        if let Some(text) = offer_inline_text {
            return self
                .run_inline_completion(
                    text,
                    &mut control_send,
                    &mut control_recv,
                    prepared_plan,
                    &mut cancel_rx,
                )
                .await;
        }

        // --- Data Transfer ---
        let prepared = prepared.expect("file send path always prepares a blob store");
        let blob_service = BlobService::new(self.endpoint.clone());
        let registration = blob_service
            .register_with_strategy(prepared, &self.blob_strategy)
            .await
            .map_err(|source| {
                TransferError::other("registering files with blob service", source)
            })?;

        protocol_wire::write_sender_message(
            &mut control_send,
            &protocol_message::SenderMessage::BlobTicket(protocol_message::BlobTicketMessage {
                session_id: self.session_id.clone(),
                ticket: registration.ticket().to_string(),
            }),
        )
        .await?;

        // Wait for the receiver to open its progress stream — but stay responsive
        // to a user cancel and a peer disconnect. A conforming receiver opens (and
        // writes to) this stream promptly, but a QUIC uni stream isn't observable
        // until the peer writes to it, so a receiver that defers its first
        // progress frame would otherwise park us here with Cancel unable to fire
        // and the blob provider still serving (the receiver keeps pulling bytes).
        let mut progress_recv = tokio::select! {
            res = connection.accept_uni() => {
                res.map_err(|source| TransferError::other("accepting progress stream", source))?
            }
            _ = wait_for_cancel(&mut cancel_rx) => {
                let _ = protocol_wire::write_sender_message(
                    &mut control_send,
                    &protocol_message::SenderMessage::Cancel(protocol_message::Cancel {
                        session_id: self.session_id.clone(),
                        by: protocol_message::TransferRole::Sender,
                        phase: protocol_message::CancelPhase::Transferring,
                        reason: "cancelled by user".to_owned(),
                    }),
                ).await;
                // Tear the provider down so a pull-model receiver can't keep
                // fetching, then close the connection in ~1 RTT.
                let _ = registration.shutdown().await;
                close_connection_gracefully(&self.endpoint, &connection, &self.blob_strategy).await;
                return Ok(TransferOutcome::local_cancel(
                    protocol_message::TransferRole::Sender,
                    protocol_message::CancelPhase::Transferring,
                ));
            }
            _ = connection.closed() => {
                return Err(TransferError::other(
                    "accepting progress stream",
                    std::io::Error::other("receiver disconnected before the transfer started"),
                ));
            }
        };

        let outcome = tokio::select! {
            res = do_transfer(&self.session_id, &prepared_plan, &mut progress_recv, &mut control_recv, &self.events) => res?,
            _ = wait_for_cancel(&mut cancel_rx) => {
                let _ = protocol_wire::write_sender_message(
                    &mut control_send,
                    &protocol_message::SenderMessage::Cancel(protocol_message::Cancel {
                        session_id: self.session_id.clone(),
                        by: protocol_message::TransferRole::Sender,
                        phase: protocol_message::CancelPhase::Transferring,
                        reason: "cancelled by user".to_owned(),
                    }),
                ).await;
                TransferOutcome::local_cancel(protocol_message::TransferRole::Sender, protocol_message::CancelPhase::Transferring)
            }
        };

        // --- Final Acknowledgement ---
        if matches!(outcome, TransferOutcome::Completed) {
            let _ = protocol_wire::write_sender_message(
                &mut control_send,
                &protocol_message::SenderMessage::TransferAck(protocol_message::TransferAck {
                    session_id: self.session_id.clone(),
                }),
            )
            .await;
            finish_control_stream(&mut control_send).await;
        }

        let _ = registration.shutdown().await;

        // Close gracefully so the peer sees the session end in ~1 RTT (a
        // CONNECTION_CLOSE frame) instead of waiting out the QUIC idle timeout.
        // This matters most for the browser receiver: on a Ctrl+C cancel it is
        // blocked reading the blob stream (not the control channel), so a
        // dangling connection leaves it stuck on a dead relay path for 5-10s.
        close_connection_gracefully(&self.endpoint, &connection, &self.blob_strategy).await;
        Ok(outcome)
    }

    /// Dial the peer with a bounded retry (see [`CONNECT_ATTEMPTS`]). Returns
    /// `Ok(None)` if the user cancelled mid-dial, so the caller can finish as a
    /// clean local cancel rather than a connect error.
    async fn connect_with_retry(
        &self,
        cancel_rx: &mut watch::Receiver<bool>,
    ) -> Result<Option<Connection>> {
        let mut last_err: Option<TransferError> = None;
        for attempt in 0..CONNECT_ATTEMPTS {
            let connect = self
                .endpoint
                .connect(self.request.peer_endpoint_addr.clone(), ALPN);
            tokio::select! {
                res = tokio::time::timeout(CONNECT_ATTEMPT_TIMEOUT, connect) => match res {
                    Ok(Ok(conn)) => return Ok(Some(conn)),
                    Ok(Err(source)) => {
                        last_err = Some(TransferError::other("connecting to peer", source));
                    }
                    Err(_elapsed) => {
                        last_err = Some(TransferError::timeout("connecting to peer"));
                    }
                },
                _ = wait_for_cancel(cancel_rx) => return Ok(None),
            }
            if attempt + 1 < CONNECT_ATTEMPTS {
                tokio::select! {
                    _ = tokio::time::sleep(CONNECT_RETRY_DELAY) => {}
                    _ = wait_for_cancel(cancel_rx) => return Ok(None),
                }
            }
        }
        Err(last_err.unwrap_or_else(|| TransferError::timeout("connecting to peer")))
    }

    /// Finish a text-only (inline) send.  By this point the receiver has the
    /// text (it travelled in the offer) and has Accepted.  No blobs flow:
    /// we just confirm the receiver's result and ack, reusing the same
    /// `TransferResult` / `TransferAck` control messages the file path uses.
    async fn run_inline_completion(
        &self,
        text: String,
        control_send: &mut iroh::endpoint::SendStream,
        control_recv: &mut iroh::endpoint::RecvStream,
        prepared_plan: TransferPlan,
        cancel_rx: &mut watch::Receiver<bool>,
    ) -> Result<TransferOutcome> {
        self.events.emit(SenderEvent::TransferStarted {
            session_id: self.session_id.clone(),
            plan: prepared_plan,
        });

        let message = tokio::select! {
            res = protocol_wire::read_receiver_message(control_recv) => res?,
            _ = wait_for_cancel(cancel_rx) => {
                let _ = protocol_wire::write_sender_message(
                    control_send,
                    &protocol_message::SenderMessage::Cancel(protocol_message::Cancel {
                        session_id: self.session_id.clone(),
                        by: protocol_message::TransferRole::Sender,
                        phase: protocol_message::CancelPhase::Transferring,
                        reason: "cancelled by user".to_owned(),
                    }),
                ).await;
                return Ok(TransferOutcome::local_cancel(
                    protocol_message::TransferRole::Sender,
                    protocol_message::CancelPhase::Transferring,
                ));
            }
            _ = tokio::time::sleep(Duration::from_secs(30)) => {
                return Err(TransferError::timeout("waiting for inline text result"));
            }
        };

        match message {
            protocol_message::ReceiverMessage::TransferResult(result) => match result.status {
                protocol_message::TransferStatus::Ok => {}
                protocol_message::TransferStatus::Error { code, message } => {
                    return Err(TransferError::other(
                        "inline text error from receiver",
                        std::io::Error::other(format!("{code:?}: {message}")),
                    ));
                }
            },
            protocol_message::ReceiverMessage::Cancel(cancel) => {
                return Ok(TransferOutcome::from_remote_cancel(
                    cancel,
                    &self.session_id,
                )?);
            }
            other => {
                return Err(ProtocolError::unexpected_message_kind(
                    "receiver inline result",
                    MessageKind::TransferResult,
                    other.kind(),
                )
                .into());
            }
        }

        self.events.emit(SenderEvent::TransferCompleted {
            session_id: self.session_id.clone(),
            snapshot: synthetic_text_snapshot(&self.session_id, text.len() as u64),
        });

        let _ = protocol_wire::write_sender_message(
            control_send,
            &protocol_message::SenderMessage::TransferAck(protocol_message::TransferAck {
                session_id: self.session_id.clone(),
            }),
        )
        .await;
        finish_control_stream(control_send).await;
        Ok(TransferOutcome::Completed)
    }
}

/// Build a one-item plan describing an inline-text send so the sender UI can
/// render "1 item · <size>" without a real prepared collection.
fn synthetic_text_plan(session_id: &str, byte_len: u64) -> TransferPlan {
    TransferPlan {
        session_id: session_id.to_owned(),
        total_files: 1,
        total_bytes: byte_len,
        files: vec![super::types::TransferPlanFile {
            id: 0,
            path: "Text snippet".to_owned(),
            size: byte_len,
        }],
    }
}

/// Terminal snapshot for an inline-text send (everything done, no byte stream).
fn synthetic_text_snapshot(session_id: &str, byte_len: u64) -> TransferSnapshot {
    TransferSnapshot {
        session_id: session_id.to_owned(),
        phase: TransferPhase::Completed,
        total_files: 1,
        completed_files: 1,
        total_bytes: byte_len,
        bytes_transferred: byte_len,
        active_file_id: None,
        active_file_bytes: None,
        bytes_per_sec: None,
        eta_seconds: None,
    }
}

async fn finish_control_stream(send: &mut iroh::endpoint::SendStream) {
    let _ = send.finish();
    let _ = tokio::time::timeout(Duration::from_secs(2), send.stopped()).await;
}

/// End the session's transport promptly so the peer isn't left waiting on the
/// QUIC idle timeout to notice we're done/cancelled.
///
/// - **Internal** endpoint (we bound it ourselves, e.g. the CLI): close the
///   whole endpoint and await the flush, guaranteeing the CONNECTION_CLOSE
///   frame is transmitted before the endpoint is dropped on return.
/// - **External/shared** endpoint (e.g. the Flutter app's receiver-service
///   endpoint): only close *this* connection — the endpoint is long-lived and
///   flushes on its own; closing it would break everything else sharing it.
async fn close_connection_gracefully(
    endpoint: &Endpoint,
    connection: &Connection,
    blob_strategy: &BlobServingStrategy,
) {
    match blob_strategy {
        BlobServingStrategy::Internal => endpoint.close().await,
        _ => connection.close(0u32.into(), b"session complete"),
    }
}

enum HandshakeResult {
    Ok(
        iroh::endpoint::SendStream,
        iroh::endpoint::RecvStream,
        protocol_sender::SenderControlOutcome,
    ),
    Cancelled(TransferOutcome),
}

async fn do_handshake(
    session_id: &str,
    identity: &protocol_message::Identity,
    manifest: protocol_message::TransferManifest,
    collection_hash: iroh_blobs::Hash,
    inline_text: Option<String>,
    connection: &Connection,
    cancel_rx: &mut watch::Receiver<bool>,
    events: &SenderEventSink,
    prepared_plan: TransferPlan,
) -> Result<HandshakeResult> {
    // Phase 1 — the machine handshake.  Open the stream, exchange hellos, and
    // put the offer on the wire under a tight timeout; the receiver isn't
    // waiting on a human yet.
    let (mut send, mut recv, mut handler) = tokio::select! {
        res = async {
            let (mut send, mut recv) = connection
                .open_bi()
                .await
                .map_err(|source| TransferError::other("opening bi-stream", source))?;

            let handler = run_offer_phase(
                session_id,
                identity,
                manifest,
                collection_hash,
                inline_text,
                &mut send,
                &mut recv,
                events,
                prepared_plan,
            )
            .await?;

            Ok::<_, TransferError>((send, recv, handler))
        } => res?,
        _ = wait_for_cancel(cancel_rx) => {
            return Ok(HandshakeResult::Cancelled(TransferOutcome::local_cancel(
                protocol_message::TransferRole::Sender,
                protocol_message::CancelPhase::WaitingForDecision,
            )));
        }
        _ = tokio::time::sleep(HANDSHAKE_TIMEOUT) => return Err(TransferError::timeout("handshake")),
    };

    // Phase 2 — wait for the receiver's accept/decline.  A human is in this
    // loop now, so the wait gets its own generous window ([`DECISION_WAIT`])
    // that outlasts the receiver's 120s decision timeout.  Keeping it on the
    // short handshake timeout was what auto-closed the share before the user
    // could tap.  The connection stays alive via QUIC keepalive; cancel and a
    // dropped connection still short-circuit the wait.
    tokio::select! {
        res = handler.await_decision(&mut recv) => {
            let outcome = res?;
            Ok(HandshakeResult::Ok(send, recv, outcome))
        }
        _ = wait_for_cancel(cancel_rx) => {
            // Best-effort tell the receiver on the control stream it's already
            // reading, so it settles on a clean "sender cancelled" rather than a
            // bare disconnect. (The call site also closes the connection, which
            // covers the case where the offer stream itself is wedged.)
            let _ = protocol_wire::write_sender_message(
                &mut send,
                &protocol_message::SenderMessage::Cancel(protocol_message::Cancel {
                    session_id: session_id.to_owned(),
                    by: protocol_message::TransferRole::Sender,
                    phase: protocol_message::CancelPhase::WaitingForDecision,
                    reason: "cancelled by sender".to_owned(),
                }),
            )
            .await;
            finish_control_stream(&mut send).await;
            Ok(HandshakeResult::Cancelled(
                TransferOutcome::local_cancel(
                    protocol_message::TransferRole::Sender,
                    protocol_message::CancelPhase::WaitingForDecision,
                ),
            ))
        }
        _ = connection.closed() => Err(TransferError::connection_closed("before receiver decision")),
        _ = tokio::time::sleep(DECISION_WAIT) => {
            Err(TransferError::timeout("waiting for receiver decision"))
        }
    }
}

/// Run the handshake up to (and including) the offer, returning the protocol
/// `Sender` so the caller can await the receiver's decision separately —
/// crucially, off the short handshake timeout.  Emits `WaitingForDecision`
/// once the offer is on the wire.
async fn run_offer_phase<R, W>(
    session_id: &str,
    identity: &protocol_message::Identity,
    manifest: protocol_message::TransferManifest,
    collection_hash: iroh_blobs::Hash,
    inline_text: Option<String>,
    send: &mut W,
    recv: &mut R,
    events: &SenderEventSink,
    prepared_plan: TransferPlan,
) -> Result<protocol_sender::Sender>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut handler = protocol_sender::Sender::new(session_id.to_owned(), identity.clone());
    handler.send_hello(send).await?;
    let peer_hello = handler.read_peer_hello(recv).await?;
    handler
        .send_offer(send, manifest, collection_hash, inline_text)
        .await?;
    events.emit(SenderEvent::WaitingForDecision {
        session_id: session_id.to_owned(),
        receiver_device_name: peer_hello.identity.device_name,
        receiver_device_type: peer_hello.identity.device_type,
        receiver_endpoint_id: peer_hello.identity.endpoint_id,
        receiver_web: peer_hello.identity.web,
        receiver_ephemeral: peer_hello.identity.ephemeral,
        prepared_plan,
    });
    Ok(handler)
}

/// Offer phase + decision wait on a single pair of streams.  Used by the
/// in-memory handshake tests, where the receiver accepts immediately so the
/// two phases need no separate timeouts.
#[cfg(test)]
async fn run_handshake_on_streams<R, W>(
    session_id: &str,
    identity: &protocol_message::Identity,
    manifest: protocol_message::TransferManifest,
    collection_hash: iroh_blobs::Hash,
    inline_text: Option<String>,
    send: &mut W,
    recv: &mut R,
    events: &SenderEventSink,
    prepared_plan: TransferPlan,
) -> Result<protocol_sender::SenderControlOutcome>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut handler = run_offer_phase(
        session_id,
        identity,
        manifest,
        collection_hash,
        inline_text,
        send,
        recv,
        events,
        prepared_plan,
    )
    .await?;
    Ok(handler.await_decision(recv).await?)
}

async fn do_transfer<R, C>(
    session_id: &str,
    plan: &TransferPlan,
    progress_recv: &mut R,
    control_recv: &mut C,
    events: &SenderEventSink,
) -> Result<TransferOutcome>
where
    R: AsyncRead + Unpin,
    C: AsyncRead + Unpin,
{
    events.emit(SenderEvent::TransferStarted {
        session_id: session_id.to_owned(),
        plan: plan.clone(),
    });

    let mut progress_active = true;
    let mut control_done = false;
    // Set when the receiver has successfully signalled TransferCompleted on
    // the progress stream — at that point bytes + export are confirmed done
    // on the receiver side, so a subsequent control-stream read error
    // (idle-timeout, transient network loss before the explicit
    // TransferResult arrived) shouldn't fail the whole transfer.  See
    // docs/upstream-bug-audit.md for the "Sender errors with Protocol
    // mismatch while receiver completes" entry.
    let mut seen_transfer_completed = false;
    // The wire protocol doesn't carry bytes_per_sec / eta — both peers measure
    // their own throughput against their own clock. We record each progress
    // tick locally so the sender's UI can show the same speed/ETA the receiver
    // does.
    let mut speed = SpeedCalculator::new();

    let mut progress_fut = Box::pin(protocol_wire::read_receiver_message(progress_recv));
    let mut control_fut = Box::pin(protocol_wire::read_receiver_message(control_recv));

    loop {
        if control_done && !progress_active {
            return Ok(TransferOutcome::Completed);
        }

        let mut reload_progress = false;
        tokio::select! {
            msg = &mut progress_fut, if progress_active => {
                reload_progress = true;
                match msg {
                    Ok(protocol_message::ReceiverMessage::TransferProgress(p)) => {
                        events.emit(SenderEvent::TransferProgress {
                            session_id: session_id.to_owned(),
                            snapshot: from_wire_snapshot(
                                p.snapshot,
                                session_id,
                                plan.total_bytes,
                                &mut speed,
                            ),
                        });
                    }
                    Ok(protocol_message::ReceiverMessage::TransferCompleted(c)) => {
                        let snapshot = from_wire_snapshot(
                            c.snapshot,
                            session_id,
                            plan.total_bytes,
                            &mut speed,
                        );
                        seen_transfer_completed = true;
                        events.emit(SenderEvent::TransferCompleted {
                            session_id: session_id.to_owned(),
                            snapshot,
                        });
                    }
                    Ok(other) => return Err(ProtocolError::unexpected_message_kind("receiver progress", MessageKind::TransferProgress, other.kind()).into()),
                    Err(_) => {
                        progress_active = false;
                        reload_progress = false;
                    }
                }
            }
            msg = &mut control_fut, if !control_done => {
                match msg {
                    Ok(protocol_message::ReceiverMessage::TransferResult(r)) => {
                        match r.status {
                            protocol_message::TransferStatus::Ok => {
                                control_done = true;
                            },
                            protocol_message::TransferStatus::Error { code, message } => {
                                return Err(TransferError::other("transfer error from receiver", std::io::Error::other(format!("{code:?}: {message}"))));
                            }
                        }
                    }
                    Ok(protocol_message::ReceiverMessage::Cancel(c)) => {
                        return Ok(TransferOutcome::from_remote_cancel(c, session_id)?);
                    }
                    Ok(other) => return Err(ProtocolError::unexpected_message_kind("receiver control", MessageKind::TransferResult, other.kind()).into()),
                    // If we've already seen TransferCompleted on the progress
                    // stream, the byte transfer + receiver-side export are
                    // confirmed done — a subsequent control-stream read error
                    // is most likely the QUIC keepalive losing the path during
                    // the brief post-export window (see audit doc).  Treat
                    // it as a successful end-of-control rather than failing
                    // the whole transfer.  Errors before TransferCompleted
                    // still propagate.
                    Err(error) => {
                        if seen_transfer_completed {
                            control_done = true;
                        } else {
                            return Err(error.into());
                        }
                    }
                }
            }
        }

        if reload_progress {
            drop(progress_fut);
            progress_fut = Box::pin(protocol_wire::read_receiver_message(progress_recv));
        }
    }
}

fn build_prepared_plan(
    session_id: &str,
    prepared: &crate::blobs::send::PreparedStore,
) -> Result<TransferPlan> {
    Ok(TransferPlan::from_manifest(
        session_id.to_owned(),
        &prepared.manifest(),
    )?)
}

fn from_wire_snapshot(
    snapshot: protocol_message::TransferProgressPayload,
    session_id: &str,
    plan_total_bytes: u64,
    speed: &mut SpeedCalculator,
) -> TransferSnapshot {
    let now = std::time::Instant::now();
    let total_bytes = plan_total_bytes.max(snapshot.total_bytes);
    let bytes_transferred = snapshot.bytes_transferred.min(total_bytes);

    let (bytes_per_sec, eta_seconds) = if matches!(snapshot.phase, TransferPhase::Transferring) {
        speed.record(now, bytes_transferred);
        let rate = speed.bytes_per_sec(now);
        let eta = rate.and_then(|r| {
            if r == 0 {
                return None;
            }
            let remaining = total_bytes.saturating_sub(bytes_transferred);
            if remaining == 0 {
                None
            } else {
                Some(((remaining as f64) / r as f64).ceil() as u64)
            }
        });
        (rate, eta)
    } else {
        speed.reset();
        (None, None)
    };

    TransferSnapshot {
        session_id: session_id.to_owned(),
        phase: snapshot.phase,
        total_files: snapshot.total_files,
        completed_files: snapshot.completed_files,
        total_bytes: snapshot.total_bytes,
        bytes_transferred: snapshot.bytes_transferred,
        active_file_id: snapshot.active_file_id,
        active_file_bytes: snapshot.active_file_bytes,
        bytes_per_sec,
        eta_seconds,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::message::{
        Accept, DeviceType, Hello, Identity, ManifestItem, PROTOCOL_VERSION, ReceiverMessage,
        SenderMessage, TransferCompleted, TransferManifest, TransferProgress,
        TransferProgressPayload, TransferResult as TransferResultMessage, TransferRole,
        TransferStatus,
    };
    use crate::protocol::wire::{read_sender_message, write_receiver_message};
    use crate::transfer::TransferPhase;
    use iroh::SecretKey;
    use tokio::io::{AsyncWriteExt, duplex};
    use tokio::sync::mpsc;
    use tokio::time::{Duration, sleep};

    type TestResult<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

    #[tokio::test]
    async fn handshake_emits_waiting_for_decision_after_offer() -> TestResult<()> {
        let (sender_io, receiver_io) = duplex(4096);
        let (mut sender_read, mut sender_write) = tokio::io::split(sender_io);
        let (mut receiver_read, mut receiver_write) = tokio::io::split(receiver_io);
        let expected_receiver_endpoint_id = SecretKey::from_bytes(&[2; 32]).public();

        let receiver_task = tokio::spawn(async move {
            let hello = match read_sender_message(&mut receiver_read).await? {
                SenderMessage::Hello(hello) => hello,
                other => panic!("expected sender hello, got {:?}", other.kind()),
            };
            write_receiver_message(
                &mut receiver_write,
                &ReceiverMessage::Hello(Hello {
                    version: PROTOCOL_VERSION,
                    session_id: hello.session_id.clone(),
                    identity: Identity {
                        role: TransferRole::Receiver,
                        endpoint_id: expected_receiver_endpoint_id,
                        device_name: "receiver".to_owned(),
                        device_type: DeviceType::Laptop,
                        web: false,
                        ephemeral: false,
                    },
                }),
            )
            .await?;

            match read_sender_message(&mut receiver_read).await? {
                SenderMessage::Offer(_) => {}
                other => panic!("expected sender offer, got {:?}", other.kind()),
            }
            write_receiver_message(
                &mut receiver_write,
                &ReceiverMessage::Accept(Accept {
                    session_id: hello.session_id.clone(),
                }),
            )
            .await?;
            TestResult::Ok(())
        });

        let events = event_sink("session-1");
        let test_manifest = manifest();
        let plan = TransferPlan::from_manifest("session-1".to_owned(), &test_manifest)?;
        let outcome = run_handshake_on_streams(
            "session-1",
            &sender_identity(),
            test_manifest,
            [3u8; 32].into(),
            None,
            &mut sender_write,
            &mut sender_read,
            &events.sink,
            plan.clone(),
        )
        .await?;

        assert!(matches!(
            outcome,
            protocol_sender::SenderControlOutcome::Accepted(_)
        ));
        receiver_task.await??;

        let observed = events.collect();
        assert!(matches!(
            observed.as_slice(),
            [SenderEvent::WaitingForDecision {
                session_id,
                receiver_device_name,
                receiver_device_type: _,
                receiver_endpoint_id: endpoint_id,
                prepared_plan,
                ..
            }] if session_id == "session-1"
                && receiver_device_name == "receiver"
                && *endpoint_id == expected_receiver_endpoint_id
                && prepared_plan == &plan
        ));

        Ok(())
    }

    #[tokio::test]
    async fn transfer_started_event_precedes_progress_and_carries_plan() -> TestResult<()> {
        let (mut progress_write, mut progress_read) = duplex(4096);
        let (mut control_write, mut control_read) = duplex(4096);
        let plan = TransferPlan::from_manifest("session-1", &manifest())?;

        let receiver_task = tokio::spawn(async move {
            write_receiver_message(
                &mut progress_write,
                &ReceiverMessage::TransferProgress(TransferProgress {
                    session_id: "session-1".to_owned(),
                    snapshot: TransferProgressPayload {
                        phase: TransferPhase::Transferring,
                        completed_files: 1,
                        total_files: 2,
                        bytes_transferred: 5,
                        total_bytes: 11,
                        active_file_id: Some(1),
                        active_file_bytes: Some(0),
                    },
                }),
            )
            .await?;
            write_receiver_message(
                &mut progress_write,
                &ReceiverMessage::TransferCompleted(TransferCompleted {
                    session_id: "session-1".to_owned(),
                    snapshot: TransferProgressPayload {
                        phase: TransferPhase::Completed,
                        completed_files: 2,
                        total_files: 2,
                        bytes_transferred: 11,
                        total_bytes: 11,
                        active_file_id: None,
                        active_file_bytes: None,
                    },
                }),
            )
            .await?;
            progress_write.shutdown().await?;
            sleep(Duration::from_millis(10)).await;

            write_receiver_message(
                &mut control_write,
                &ReceiverMessage::TransferResult(TransferResultMessage {
                    session_id: "session-1".to_owned(),
                    status: TransferStatus::Ok,
                }),
            )
            .await?;
            TestResult::Ok(())
        });

        let events = event_sink("session-1");
        let outcome = do_transfer(
            "session-1",
            &plan,
            &mut progress_read,
            &mut control_read,
            &events.sink,
        )
        .await?;

        assert!(matches!(outcome, TransferOutcome::Completed));
        receiver_task.await??;

        let observed = events.collect();
        assert!(matches!(
            observed.as_slice(),
            [
                SenderEvent::TransferStarted { plan: started_plan, .. },
                SenderEvent::TransferProgress { .. },
                SenderEvent::TransferCompleted { .. },
            ] if started_plan == &plan
        ));
        Ok(())
    }

    /// Regression: if the receiver finishes the byte transfer, ships
    /// TransferCompleted on progress, and then the control stream goes
    /// dark before TransferResult arrives (idle timeout / lost keepalive
    /// during the post-export window), the sender must still return
    /// Completed instead of failing with ProtocolIncompatible.
    #[tokio::test]
    async fn control_stream_close_after_transfer_completed_yields_completed() -> TestResult<()> {
        let (mut progress_write, mut progress_read) = duplex(4096);
        let (mut control_write, mut control_read) = duplex(4096);
        let plan = TransferPlan::from_manifest("session-1", &manifest())?;

        let receiver_task = tokio::spawn(async move {
            write_receiver_message(
                &mut progress_write,
                &ReceiverMessage::TransferProgress(TransferProgress {
                    session_id: "session-1".to_owned(),
                    snapshot: TransferProgressPayload {
                        phase: TransferPhase::Transferring,
                        completed_files: 1,
                        total_files: 2,
                        bytes_transferred: 5,
                        total_bytes: 11,
                        active_file_id: Some(1),
                        active_file_bytes: Some(0),
                    },
                }),
            )
            .await?;
            write_receiver_message(
                &mut progress_write,
                &ReceiverMessage::TransferCompleted(TransferCompleted {
                    session_id: "session-1".to_owned(),
                    snapshot: TransferProgressPayload {
                        phase: TransferPhase::Completed,
                        completed_files: 2,
                        total_files: 2,
                        bytes_transferred: 11,
                        total_bytes: 11,
                        active_file_id: None,
                        active_file_bytes: None,
                    },
                }),
            )
            .await?;
            progress_write.shutdown().await?;
            sleep(Duration::from_millis(10)).await;
            // Simulate the lost keepalive / dead path: receiver writes
            // nothing on the control stream and drops the write half.
            drop(control_write);
            TestResult::Ok(())
        });

        let events = event_sink("session-1");
        let outcome = do_transfer(
            "session-1",
            &plan,
            &mut progress_read,
            &mut control_read,
            &events.sink,
        )
        .await?;

        assert!(matches!(outcome, TransferOutcome::Completed));
        receiver_task.await??;

        let observed = events.collect();
        assert!(
            observed
                .iter()
                .any(|e| matches!(e, SenderEvent::TransferCompleted { .. })),
            "expected TransferCompleted event before control close, got {observed:?}",
        );
        Ok(())
    }

    /// Negative pairing for the previous test: if the control stream closes
    /// BEFORE TransferCompleted has been observed on progress, the sender
    /// has no proof the receiver finished and must still fail.  This
    /// guards against silently swallowing a real protocol error.
    #[tokio::test]
    async fn control_stream_close_before_transfer_completed_still_fails() -> TestResult<()> {
        let (mut progress_write, mut progress_read) = duplex(4096);
        let (control_write, mut control_read) = duplex(4096);
        let plan = TransferPlan::from_manifest("session-1", &manifest())?;

        let receiver_task = tokio::spawn(async move {
            // Only a mid-transfer progress message — never TransferCompleted.
            write_receiver_message(
                &mut progress_write,
                &ReceiverMessage::TransferProgress(TransferProgress {
                    session_id: "session-1".to_owned(),
                    snapshot: TransferProgressPayload {
                        phase: TransferPhase::Transferring,
                        completed_files: 0,
                        total_files: 2,
                        bytes_transferred: 1,
                        total_bytes: 11,
                        active_file_id: Some(0),
                        active_file_bytes: Some(1),
                    },
                }),
            )
            .await?;
            sleep(Duration::from_millis(10)).await;
            // Tear down both streams without delivering TransferCompleted /
            // TransferResult — simulates a real connection failure mid-flow.
            drop(progress_write);
            drop(control_write);
            TestResult::Ok(())
        });

        let events = event_sink("session-1");
        let result = do_transfer(
            "session-1",
            &plan,
            &mut progress_read,
            &mut control_read,
            &events.sink,
        )
        .await;

        assert!(
            result.is_err(),
            "expected Err when control closes before TransferCompleted, got {result:?}",
        );
        receiver_task.await??;
        Ok(())
    }

    struct EventHarness {
        sink: SenderEventSink,
        rx: mpsc::UnboundedReceiver<SenderEvent>,
    }

    impl EventHarness {
        fn collect(mut self) -> Vec<SenderEvent> {
            drop(self.sink);
            let mut observed = Vec::new();
            while let Ok(event) = self.rx.try_recv() {
                observed.push(event);
            }
            observed
        }
    }

    fn event_sink(session_id: &str) -> EventHarness {
        let (tx, rx) = mpsc::unbounded_channel();
        EventHarness {
            sink: SenderEventSink::new(session_id.to_owned(), Some(tx)),
            rx,
        }
    }

    fn sender_identity() -> Identity {
        Identity {
            role: TransferRole::Sender,
            endpoint_id: SecretKey::from_bytes(&[1; 32]).public(),
            device_name: "sender".to_owned(),
            device_type: DeviceType::Laptop,
            web: false,
            ephemeral: false,
        }
    }

    fn manifest() -> TransferManifest {
        TransferManifest {
            items: vec![
                ManifestItem::File {
                    path: "album/a.txt".to_owned(),
                    size: 5,
                },
                ManifestItem::File {
                    path: "album/b.txt".to_owned(),
                    size: 6,
                },
            ],
        }
    }
}
