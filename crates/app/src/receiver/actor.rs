use std::time::Duration;

use tokio::sync::{broadcast, mpsc, oneshot, watch};
use tokio::time::{MissedTickBehavior, interval};

use crate::error::{AppError, AppResult};
use crate::types::{
    ConnectionPath, NearbyReceiver, PairingCodeState, ReceiverOfferEvent, ReceiverOfferPhase,
    ReceiverRegistration,
};

use super::runtime::ReceiverRuntime;
use super::{OfferDecision, ReceiverEvent, ReceiverLifecycle, ReceiverSnapshot};

#[derive(Debug)]
pub(super) enum ReceiverCommand {
    Setup {
        server_url: Option<String>,
        reply: oneshot::Sender<AppResult<ReceiverRegistration>>,
    },
    EnsureRegistered {
        server_url: Option<String>,
        reply: oneshot::Sender<AppResult<ReceiverRegistration>>,
    },
    SetDiscoverable {
        enabled: bool,
        reply: oneshot::Sender<AppResult<()>>,
    },
    RespondToOffer {
        decision: OfferDecision,
        reply: oneshot::Sender<AppResult<()>>,
    },
    CancelTransfer {
        reply: oneshot::Sender<AppResult<()>>,
    },
    OfferPrepared {
        run: super::session::ReceiverRun,
        event: ReceiverOfferEvent,
    },
    OfferProgress {
        offer_id: u64,
        event: ReceiverOfferEvent,
    },
    OfferFinished {
        offer_id: u64,
        final_event: ReceiverOfferEvent,
    },
    OfferConnectionPathChanged {
        offer_id: u64,
        connection_path: ConnectionPath,
    },
    ScanNearby {
        timeout: Duration,
        reply: oneshot::Sender<AppResult<Vec<NearbyReceiver>>>,
    },
    Shutdown {
        reply: oneshot::Sender<AppResult<()>>,
    },
}

pub(super) async fn run_receiver_actor(
    mut runtime: ReceiverRuntime,
    mut cmd_rx: mpsc::Receiver<ReceiverCommand>,
    state_tx: watch::Sender<ReceiverSnapshot>,
    pairing_tx: watch::Sender<PairingCodeState>,
    event_tx: broadcast::Sender<ReceiverEvent>,
) {
    let _ = publish_snapshot(&state_tx, &runtime, ReceiverLifecycle::Ready);
    let mut maintenance = interval(Duration::from_secs(15));
    maintenance.set_missed_tick_behavior(MissedTickBehavior::Delay);
    maintenance.tick().await;

    loop {
        tokio::select! {
            _ = maintenance.tick() => {
                if runtime.maintain_registration(&pairing_tx, &event_tx).await.is_err() {
                    let _ = pairing_tx.send(PairingCodeState::Unavailable);
                    let _ = publish_snapshot(&state_tx, &runtime, ReceiverLifecycle::Ready);
                    let _ = event_tx.send(ReceiverEvent::DiscoverabilityChanged {
                        requested: runtime.discoverable_requested,
                        active: runtime.advertising_active(),
                    });
                } else {
                    let _ = publish_snapshot(&state_tx, &runtime, ReceiverLifecycle::Ready);
                }
            }
            maybe_command = cmd_rx.recv() => {
                let Some(command) = maybe_command else {
                    break;
                };
                match command {
                    ReceiverCommand::Setup { server_url, reply } => {
                        let result = runtime.handle_setup(server_url, &pairing_tx, &event_tx).await;
                        let _ = publish_snapshot(&state_tx, &runtime, ReceiverLifecycle::Ready);
                        let _ = reply.send(result);
                    }
                    ReceiverCommand::EnsureRegistered { server_url, reply } => {
                        let result = runtime.handle_ensure_registered(server_url, &pairing_tx, &event_tx).await;
                        let _ = publish_snapshot(&state_tx, &runtime, ReceiverLifecycle::Ready);
                        let _ = reply.send(result);
                    }
                    ReceiverCommand::SetDiscoverable { enabled, reply } => {
                        let was_active = runtime.advertising_active();
                        let result = runtime.set_discoverable(enabled).await;
                        runtime.publish_discoverability_change_if_needed(was_active, &event_tx);
                        let _ = publish_snapshot(&state_tx, &runtime, ReceiverLifecycle::Ready);
                        let _ = reply.send(result);
                    }
                    ReceiverCommand::RespondToOffer { decision, reply } => {
                        let result = runtime.respond_to_offer(decision);
                        let _ = publish_snapshot(&state_tx, &runtime, ReceiverLifecycle::Ready);
                        let _ = reply.send(result);
                    }
                    ReceiverCommand::CancelTransfer { reply } => {
                        let result = runtime.cancel_active_transfer();
                        let _ = publish_snapshot(&state_tx, &runtime, ReceiverLifecycle::Ready);
                        let _ = reply.send(result);
                    }
                    ReceiverCommand::OfferPrepared { run, event } => {
                        if runtime.handle_offer_prepared(run) {
                            let _ = event_tx.send(ReceiverEvent::OfferUpdated(event));
                            // Code rotation deliberately deferred to
                            // `OfferFinished` (below).  Rotating here
                            // (when the manifest arrives but before the
                            // user accepts) used to mean the visible
                            // code changed silently mid-flow, confusing
                            // users who read the rotated code thinking
                            // it was current and then got 404'd by the
                            // server (which had already consumed the
                            // original code).
                        }
                        let _ = publish_snapshot(&state_tx, &runtime, ReceiverLifecycle::Ready);
                    }
                    ReceiverCommand::OfferProgress { offer_id, event } => {
                        if runtime.handle_offer_progress(offer_id) {
                            let _ = event_tx.send(ReceiverEvent::OfferUpdated(event));
                        }
                    }
                    ReceiverCommand::OfferFinished { offer_id, final_event } => {
                        let phase = final_event.phase;
                        if runtime.handle_offer_finished(offer_id)
                            || matches!(
                                phase,
                                ReceiverOfferPhase::Failed | ReceiverOfferPhase::Declined
                            )
                        {
                            let _ = event_tx.send(ReceiverEvent::OfferUpdated(final_event));
                        }
                        // Rotate the pairing code now that the transfer
                        // has settled.  The previous code was claimed by
                        // the sender and is dead on the server regardless
                        // of outcome, so a new code must be visible
                        // before the user attempts another send.
                        let _ = runtime
                            .refresh_registration_after_offer(&pairing_tx, &event_tx)
                            .await;
                        let _ = publish_snapshot(&state_tx, &runtime, ReceiverLifecycle::Ready);
                    }
                    ReceiverCommand::OfferConnectionPathChanged { offer_id, connection_path } => {
                        let _ = event_tx.send(ReceiverEvent::ConnectionPathChanged {
                            offer_id,
                            connection_path,
                        });
                    }
                    ReceiverCommand::ScanNearby { timeout, reply } => {
                        let exclude = Some(runtime.endpoint_id());
                        let result = tokio::task::spawn_blocking(move || {
                            wisp_core::lan::browse_nearby_receivers(timeout, exclude)
                        })
                        .await
                        .map_err(|e| AppError::Internal {
                            message: format!("receiver v2 nearby scan task: {e}"),
                        })
                        .and_then(|result| {
                            result.map_err(|e| AppError::Internal {
                                message: format!("receiver v2 nearby scan error: {e}"),
                            })
                        })
                        .map(|receivers| {
                            receivers
                                .into_iter()
                                .map(|receiver| {
                                    let endpoint_id =
                                        wisp_core::util::decode_ticket(&receiver.ticket)
                                            .map(|a| a.id.to_string())
                                            .unwrap_or_default();
                                    NearbyReceiver {
                                        fullname: receiver.fullname,
                                        label: receiver.label,
                                        device_type: match receiver.device_type {
                                            wisp_core::protocol::DeviceType::Phone => {
                                                "phone".to_owned()
                                            }
                                            wisp_core::protocol::DeviceType::Laptop => {
                                                "laptop".to_owned()
                                            }
                                        },
                                        code: receiver.code,
                                        ticket: receiver.ticket,
                                        endpoint_id,
                                    }
                                })
                                .collect()
                        });
                        let _ = reply.send(result);
                    }
                    ReceiverCommand::Shutdown { reply } => {
                        runtime.clear_advertising();
                        // Shut down the Router first so no new inbound ALPN
                        // connections are accepted, *then* close the endpoint
                        // so iroh unregisters from the relay cleanly.
                        runtime.shutdown_router().await;
                        runtime.close_endpoint().await;
                        let _ = pairing_tx.send(PairingCodeState::Unavailable);
                        let _ = publish_snapshot(&state_tx, &runtime, ReceiverLifecycle::Stopped);
                        let _ = event_tx.send(ReceiverEvent::Shutdown);
                        let _ = reply.send(Ok(()));
                        break;
                    }
                }
            }
        }
    }
}

fn publish_snapshot(
    state_tx: &watch::Sender<ReceiverSnapshot>,
    runtime: &ReceiverRuntime,
    lifecycle: ReceiverLifecycle,
) -> AppResult<()> {
    state_tx
        .send(ReceiverSnapshot {
            lifecycle,
            discoverable_requested: runtime.discoverable_requested,
            advertising_active: runtime.advertising_active(),
            has_registration: runtime.has_registration(),
            has_pending_offer: runtime.has_pending_offer(),
        })
        .map_err(|_| AppError::SnapshotChannelClosed)?;
    Ok(())
}
