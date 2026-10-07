use std::time::Duration;

use tokio::sync::{broadcast, mpsc, oneshot, watch};
use tokio::time::{MissedTickBehavior, interval};

use crate::error::{AppError, AppResult};
use crate::types::{
    ConnectionPath, PairingCodeState, ReceiverOfferEvent, ReceiverOfferPhase, ReceiverRegistration,
};

use super::runtime::{
    ReceiverRuntime, RegistrationCommand, RegistrationCommandKind, RegistrationJob,
    RegistrationOutcome,
};
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
    /// A sender connected and finished the Hello exchange, but its Offer
    /// hasn't arrived yet. Relayed straight to the UI (phase `Connecting`) so
    /// the receiver shows "connecting from <X>" without yet tracking an offer
    /// in the runtime — there's nothing to accept/decline until the Offer
    /// lands (which arrives as a later [`OfferPrepared`]) or the wait fails.
    OfferConnecting {
        event: ReceiverOfferEvent,
        offer_id: u64,
        cancel_tx: tokio::sync::watch::Sender<bool>,
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

    // Registration maintenance runs on its own task and reports back here,
    // because only this loop may touch `runtime` — see [`RegistrationJob`] for
    // why it must not be awaited inline.
    let (job_tx, mut job_rx) = mpsc::channel::<RegistrationDone>(4);
    let mut registration_running = false;
    let mut registration_failures = 0u32;
    let mut registration_cooldown = 0u32;

    loop {
        tokio::select! {
            _ = advert_reconcile.tick() => {
                runtime.reconcile_advertising().await;
            }
            _ = maintenance.tick() => {
                if registration_cooldown > 0 {
                    registration_cooldown -= 1;
                } else if !registration_running
                    && let Some(job) = runtime.registration_job()
                {
                    registration_running = true;
                    spawn_registration_job(job, &job_tx);
                }
                let _ = publish_snapshot(&state_tx, &runtime, ReceiverLifecycle::Ready);
            }
            Some(done) = job_rx.recv() => {
                let outcome = match done {
                    RegistrationDone::Maintenance(outcome) => outcome,
                    RegistrationDone::Command { kind, was_active, result, reply } => {
                        registration_running = false;
                        let result = runtime
                            .finish_registration_command(
                                kind,
                                was_active,
                                result,
                                &pairing_tx,
                                &event_tx,
                            )
                            .await;
                        let _ = publish_snapshot(&state_tx, &runtime, ReceiverLifecycle::Ready);
                        let _ = reply.send(result);
                        continue;
                    }
                };
                registration_running = false;
                if runtime
                    .apply_registration_outcome(
                        outcome,
                        registration_failures,
                        &pairing_tx,
                        &event_tx,
                    )
                    .await
                {
                    registration_failures = 0;
                    registration_cooldown = 0;
                    let _ = publish_snapshot(&state_tx, &runtime, ReceiverLifecycle::Ready);
                } else {
                    registration_failures = registration_failures.saturating_add(1);
                    registration_cooldown = registration_backoff_ticks(registration_failures);
                    let _ = publish_snapshot(&state_tx, &runtime, ReceiverLifecycle::Ready);
                    let _ = event_tx.send(ReceiverEvent::DiscoverabilityChanged {
                        requested: runtime.discoverable_requested,
                        active: runtime.advertising_active(),
                    });
                }
            }
            maybe_command = cmd_rx.recv() => {
                let Some(command) = maybe_command else {
                    break;
                };
                match command {
                    // Both of these reach the network, and this task is the
                    // only one that can put an incoming offer on screen, so
                    // neither waits here. `registration_running` is taken so a
                    // maintenance tick doesn't mint a second code alongside.
                    ReceiverCommand::Setup { server_url, reply } => {
                        let command = runtime
                            .begin_registration_command(server_url, RegistrationCommandKind::Setup);
                        registration_running = true;
                        spawn_registration_command(command, &job_tx, reply);
                        let _ = publish_snapshot(&state_tx, &runtime, ReceiverLifecycle::Ready);
                    }
                    ReceiverCommand::EnsureRegistered { server_url, reply } => {
                        let command = runtime.begin_registration_command(
                            server_url,
                            RegistrationCommandKind::EnsureRegistered,
                        );
                        registration_running = true;
                        spawn_registration_command(command, &job_tx, reply);
                        let _ = publish_snapshot(&state_tx, &runtime, ReceiverLifecycle::Ready);
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
                    ReceiverCommand::OfferConnecting {
                        event,
                        offer_id,
                        cancel_tx,
                    } => {
                        // Surface the "connecting from <X>" state to the UI and
                        // track the session's cancel handle so the user can bail
                        // out of a stalled connect. There's still no offer to
                        // accept/decline — that arrives with `OfferPrepared`.
                        runtime.handle_offer_connecting(offer_id, cancel_tx);
                        let _ = event_tx.send(ReceiverEvent::OfferUpdated(event));
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
                        if !registration_running
                            && let Some(job) = runtime.forced_registration_job()
                        {
                            registration_running = true;
                            registration_cooldown = 0;
                            spawn_registration_job(job, &job_tx);
                        }
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

/// Upper bound on a background registration attempt.
///
/// Nothing in it is urgent, but it holds the one-at-a-time slot, and the
/// rendezvous client has no request timeout of its own — so without this an
/// unbounded hang would stop registration maintenance for the rest of the
/// session.
const REGISTRATION_JOB_TIMEOUT: Duration = Duration::from_secs(30);

/// What a finished off-task registration attempt hands back to the actor.
#[derive(Debug)]
pub(super) enum RegistrationDone {
    /// The periodic maintenance attempt.
    Maintenance(RegistrationOutcome),
    /// A Setup / EnsureRegistered the caller is still waiting on.
    Command {
        kind: RegistrationCommandKind,
        was_active: bool,
        result: AppResult<ReceiverRegistration>,
        reply: oneshot::Sender<AppResult<ReceiverRegistration>>,
    },
}

fn spawn_registration_job(job: RegistrationJob, job_tx: &mpsc::Sender<RegistrationDone>) {
    let job_tx = job_tx.clone();
    tokio::spawn(async move {
        let outcome = tokio::time::timeout(REGISTRATION_JOB_TIMEOUT, job.run())
            .await
            .unwrap_or(RegistrationOutcome::Failed);
        let _ = job_tx.send(RegistrationDone::Maintenance(outcome)).await;
    });
}

/// Runs a Setup / EnsureRegistered off the actor task.
///
/// The reply travels with it so the caller is answered either way: a request
/// that outlives [`REGISTRATION_JOB_TIMEOUT`] gets an error rather than
/// nothing, which is what a caller awaiting a oneshot would otherwise get.
fn spawn_registration_command(
    command: RegistrationCommand,
    job_tx: &mpsc::Sender<RegistrationDone>,
    reply: oneshot::Sender<AppResult<ReceiverRegistration>>,
) {
    let job_tx = job_tx.clone();
    let kind = command.kind();
    let was_active = command.was_active();
    tokio::spawn(async move {
        let result = tokio::time::timeout(REGISTRATION_JOB_TIMEOUT, command.run())
            .await
            .unwrap_or_else(|_| {
                Err(AppError::ReceiverUnavailable {
                    action: "registering with the discovery server",
                })
            });
        let _ = job_tx
            .send(RegistrationDone::Command {
                kind,
                was_active,
                result,
                reply,
            })
            .await;
    });
}

/// Maintenance ticks to sit out after `failures` consecutive failures: 1, 2,
/// 4, then 8 — two minutes at the 15-second tick, and no further.
///
/// A receiver that is simply offline — a handheld serving its own hotspot, a
/// laptop on an isolated LAN — can never register, and retrying every 15
/// seconds forever only burns radio and fills the log. The ceiling stays low
/// because nothing resets the count when the network comes back: the attempt
/// itself is how we find out, so a long one would leave a usable network
/// without a pairing code for minutes.
pub(super) fn registration_backoff_ticks(failures: u32) -> u32 {
    const MAX_TICKS: u32 = 8;
    match failures {
        0 => 0,
        _ => (1u32 << (failures - 1).min(5)).min(MAX_TICKS),
    }
}
