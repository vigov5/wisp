use std::time::Duration;

use tokio::sync::{broadcast, mpsc, oneshot, watch};
use tokio::time::{MissedTickBehavior, interval};

use crate::error::{AppError, AppResult};
use crate::types::{
    ConnectionPath, PairingCodeState, ReceiverOfferEvent, ReceiverOfferPhase, ReceiverRegistration,
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

    // Separate, faster cadence purely for re-publishing the LAN advertisement
    // when our local addresses change (e.g. the USB-cable tunnel coming up adds a
    // 10.42.0.x address). Kept off the 15s `maintenance` tick so the rendezvous
    // HTTP poll there stays infrequent — reconcile_advertising is a cheap no-op
    // (interface scan + ticket-string compare) when nothing changed.
    let mut advert_reconcile = interval(Duration::from_secs(5));
    advert_reconcile.set_missed_tick_behavior(MissedTickBehavior::Delay);
    advert_reconcile.tick().await;

    loop {
        tokio::select! {
            _ = advert_reconcile.tick() => {
                runtime.reconcile_advertising().await;
            }
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
                        // `handle_offer_finished` returns true only when this
                        // offer_id was actually tracked (Pending/Receiving), so
                        // its terminal event corresponds to a card the user has
                        // already seen.  We still force terminal Failed/Declined
                        // events through for *untracked* offers so an
                        // already-surfaced offer that fails late isn't swallowed
                        // — BUT only when we know who the sender was.  A
                        // handshake that dies before the offer is produced (e.g.
                        // the sender cancels while the receiver is blocked
                        // reading the Offer frame) carries an empty
                        // `sender_name`; surfacing it rendered a bogus "Unknown
                        // sender" failed-transfer card for a transfer the user
                        // never saw begin.  Suppress those.
                        let tracked = runtime.handle_offer_finished(offer_id);
                        let identified_sender = !final_event.sender_name.trim().is_empty();
                        if tracked
                            || (matches!(
                                phase,
                                ReceiverOfferPhase::Failed | ReceiverOfferPhase::Declined
                            ) && identified_sender)
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
