use std::path::PathBuf;

use iroh::SecretKey;
use tokio::sync::{broadcast, oneshot, watch};

use crate::error::{AppError, AppResult};

use super::actor::registration_backoff_ticks;
use super::runtime::{
    OfferResolution, ReceiverRuntime, RegistrationOutcome, registration_needs_refresh,
    should_advertise,
};
use super::session::ReceiverRun;
use super::{
    OfferDecision, PairingCodeState, ReceiverEvent, ReceiverLifecycle, ReceiverRegistration,
    ReceiverService,
};
use crate::types::{ConflictPolicy, ReceiverConfig};

fn test_config() -> ReceiverConfig {
    ReceiverConfig {
        device_name: "Test Receiver".to_owned(),
        device_type: "laptop".to_owned(),
        download_root: PathBuf::from("downloads"),
        conflict_policy: ConflictPolicy::Reject,
        secret_key: SecretKey::from_bytes(&rand::random()),
    }
}

async fn try_start_service() -> AppResult<Option<ReceiverService>> {
    match ReceiverService::start(test_config()).await {
        Ok(service) => Ok(Some(service)),
        Err(error) if bind_unavailable(&error) => Ok(None),
        Err(error) => Err(error),
    }
}

async fn try_bind_endpoint() -> AppResult<Option<iroh::Endpoint>> {
    match iroh::Endpoint::builder(iroh::endpoint::presets::N0)
        .secret_key(SecretKey::from_bytes(&rand::random()))
        .bind()
        .await
    {
        Ok(endpoint) => Ok(Some(endpoint)),
        Err(error) => {
            let error = AppError::BindingFailed {
                context: error.to_string(),
            };
            if bind_unavailable(&error) {
                Ok(None)
            } else {
                Err(error)
            }
        }
    }
}

fn bind_unavailable(error: &AppError) -> bool {
    let chain = format!("{error:#}");
    chain.contains("Failed to bind sockets") || chain.contains("Operation not permitted")
}

#[tokio::test]
async fn service_starts_with_unavailable_pairing_code() -> AppResult<()> {
    let Some(service) = try_start_service().await? else {
        return Ok(());
    };
    assert_eq!(service.pairing_code(), PairingCodeState::Unavailable);
    assert_eq!(service.snapshot().lifecycle, ReceiverLifecycle::Ready);
    service.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn respond_to_offer_fails_without_pending_offer() -> AppResult<()> {
    let Some(service) = try_start_service().await? else {
        return Ok(());
    };
    let error = service
        .respond_to_offer(OfferDecision::Accept(Default::default()))
        .await
        .unwrap_err();
    assert!(matches!(error, AppError::NoPendingOffer));
    service.shutdown().await?;
    Ok(())
}

#[test]
fn registration_refreshes_when_expired() {
    let registration = ReceiverRegistration {
        code: "ABC123".to_owned(),
        expires_at: "2000-01-01T00:00:00Z".to_owned(),
    };
    assert!(registration_needs_refresh(&registration));
}

#[test]
fn registration_stays_valid_when_future_expiry_parses() {
    let registration = ReceiverRegistration {
        code: "ABC123".to_owned(),
        expires_at: "2999-01-01T00:00:00Z".to_owned(),
    };
    assert!(!registration_needs_refresh(&registration));
}

#[test]
fn discoverability_only_requires_opt_in() {
    assert!(should_advertise(true, false));
    assert!(!should_advertise(false, true));
    assert!(should_advertise(true, true));
}

#[tokio::test]
async fn stale_offer_updates_are_ignored() -> AppResult<()> {
    let Some(endpoint) = try_bind_endpoint().await? else {
        return Ok(());
    };
    let mut runtime = ReceiverRuntime::new_for_test(test_config(), endpoint);

    let (tx, _rx) = oneshot::channel::<OfferResolution>();
    let (cancel_tx, _cancel_rx) = watch::channel(false);
    let run = ReceiverRun {
        offer_id: 7,
        decision_tx: tx,
        cancel_tx,
    };
    assert!(runtime.handle_offer_prepared(run));
    assert!(!runtime.handle_offer_progress(8));
    assert!(!runtime.handle_offer_finished(8));
    Ok(())
}

#[tokio::test]
async fn busy_runtime_rejects_second_offer() -> AppResult<()> {
    let Some(endpoint) = try_bind_endpoint().await? else {
        return Ok(());
    };
    let mut runtime = ReceiverRuntime::new_for_test(test_config(), endpoint);

    let (tx1, _rx1) = oneshot::channel::<OfferResolution>();
    let (tx2, rx2) = oneshot::channel::<OfferResolution>();
    let (cancel_tx1, _cancel_rx1) = watch::channel(false);
    let (cancel_tx2, _cancel_rx2) = watch::channel(false);
    assert!(runtime.handle_offer_prepared(ReceiverRun {
        offer_id: 1,
        decision_tx: tx1,
        cancel_tx: cancel_tx1,
    }));
    assert!(!runtime.handle_offer_prepared(ReceiverRun {
        offer_id: 2,
        decision_tx: tx2,
        cancel_tx: cancel_tx2,
    }));
    assert!(matches!(rx2.await.unwrap(), OfferResolution::Decline));
    Ok(())
}

#[tokio::test]
async fn no_registration_job_without_a_server_url() -> AppResult<()> {
    let Some(endpoint) = try_bind_endpoint().await? else {
        return Ok(());
    };
    let runtime = ReceiverRuntime::new_for_test(test_config(), endpoint);

    assert!(
        runtime.registration_job().is_none(),
        "there is nothing to maintain without a rendezvous server"
    );
    assert!(
        runtime.forced_registration_job().is_none(),
        "a post-transfer rotation has nothing to rotate either"
    );
    Ok(())
}

#[test]
fn the_registration_backoff_doubles_and_settles_at_two_minutes() {
    // Ticks are 15s apart, so 8 ticks is the two-minute ceiling: long enough
    // to stop hammering a network that cannot register, short enough that a
    // network coming back is noticed promptly — the attempt is the only thing
    // that notices.
    assert_eq!(registration_backoff_ticks(0), 0);
    assert_eq!(registration_backoff_ticks(1), 1);
    assert_eq!(registration_backoff_ticks(2), 2);
    assert_eq!(registration_backoff_ticks(4), 8);
    assert_eq!(registration_backoff_ticks(6), 8);
    assert_eq!(registration_backoff_ticks(u32::MAX), 8);
}

#[tokio::test]
async fn a_fresh_code_survives_a_maintenance_pass() -> AppResult<()> {
    // Regression for the "code rotates while sender still connecting" bug.
    //
    // Setup: receiver has a registration whose TTL has NOT expired.  A
    // maintenance pass polls `pair_status` and only rotates on a definite 404
    // — the code the user is reading must not change underneath a sender that
    // is still connecting, least of all because one poll failed.
    //
    // Setting a bogus server URL is deliberate: the poll cannot reach it, and
    // a failed poll must leave everything exactly as it was. If the buggy
    // code path resurfaces, the test fails by sending a rotation event.
    let Some(endpoint) = try_bind_endpoint().await? else {
        return Ok(());
    };
    let mut runtime = ReceiverRuntime::new_for_test(test_config(), endpoint);

    let registration = ReceiverRegistration {
        code: "ABC123".to_owned(),
        expires_at: "2999-01-01T00:00:00Z".to_owned(),
    };
    runtime.set_server_url_for_test(Some(
        "http://127.0.0.1:1/__unreachable_test_host__".to_owned(),
    ));
    runtime.set_registration_for_test(Some(registration.clone()));

    let (pairing_tx, mut pairing_rx) = watch::channel(PairingCodeState::Unavailable);
    let (event_tx, mut event_rx) = broadcast::channel::<ReceiverEvent>(8);
    pairing_rx.borrow_and_update();

    let outcome = runtime
        .registration_job()
        .expect("a server url is configured")
        .run()
        .await;
    assert_eq!(
        outcome,
        RegistrationOutcome::Unchanged,
        "an unreachable rendezvous server must not change the visible code"
    );
    assert!(
        runtime
            .apply_registration_outcome(outcome, &pairing_tx, &event_tx)
            .await
    );

    assert_eq!(
        runtime.registration_for_test(),
        Some(&registration),
        "registration must stay intact while TTL is in the future, even if the \
         sender has already claimed the code (server would return 404 on pair_status)"
    );
    assert!(
        !pairing_rx.has_changed().unwrap(),
        "no PairingCodeState update should be emitted when the existing \
         registration is still fresh"
    );
    assert!(
        matches!(
            event_rx.try_recv(),
            Err(broadcast::error::TryRecvError::Empty)
        ),
        "no RegistrationUpdated event should be emitted when the existing \
         registration is still fresh"
    );
    Ok(())
}
