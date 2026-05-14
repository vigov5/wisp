use drift_core::lan::LanReceiveAdvertisement;
use drift_core::rendezvous::{RendezvousClient, resolve_server_url};
use drift_core::util::make_ticket;
use iroh::protocol::Router;
use iroh::{Endpoint, EndpointId};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use tokio::sync::{broadcast, watch};
use tracing::warn;

use crate::error::{AppError, AppResult};
use crate::types::{PairingCodeState, ReceiverConfig, ReceiverRegistration};

use super::session::ReceiverRun;
use super::{OfferDecision, ReceiverEvent, parse_device_type};

pub(super) struct ReceiverRuntime {
    config: ReceiverConfig,
    endpoint: Endpoint,
    /// Router that multiplexes inbound ALPNs on `endpoint`.  Held here so
    /// shutdown can tear it down (and stop accepting) before closing the
    /// endpoint.  Wrapped in `Option` only so tests using the bare
    /// `ReceiverRuntime::new` constructor (which doesn't go through
    /// `ReceiverService::start`) can supply a stub.
    router: Option<Router>,
    server_url: Option<String>,
    registration: Option<ReceiverRegistration>,
    pub(super) discoverable_requested: bool,
    advertising: Option<LanReceiveAdvertisement>,
    offer_state: OfferState,
}

#[derive(Debug)]
pub(super) enum OfferResolution {
    Accept,
    Decline,
    Cancel,
}

pub(super) enum OfferState {
    Idle,
    Pending(PendingOfferState),
    Receiving {
        offer_id: u64,
        cancel_tx: watch::Sender<bool>,
    },
}

pub(super) struct PendingOfferState {
    run: ReceiverRun,
}

impl ReceiverRuntime {
    pub(super) fn new(config: ReceiverConfig, endpoint: Endpoint, router: Router) -> Self {
        Self {
            config,
            endpoint,
            router: Some(router),
            server_url: None,
            registration: None,
            discoverable_requested: false,
            advertising: None,
            offer_state: OfferState::Idle,
        }
    }

    /// Test-only constructor for unit tests that just need a runtime to
    /// poke at state — skips spinning up a real Router.
    #[cfg(test)]
    pub(super) fn new_for_test(config: ReceiverConfig, endpoint: Endpoint) -> Self {
        Self {
            config,
            endpoint,
            router: None,
            server_url: None,
            registration: None,
            discoverable_requested: false,
            advertising: None,
            offer_state: OfferState::Idle,
        }
    }

    pub(super) fn endpoint_id(&self) -> EndpointId {
        self.endpoint.addr().id
    }

    /// Test-only setters that bypass the rendezvous server.  Production
    /// callers must go through `handle_setup` / `ensure_registered`, which
    /// register via HTTP — those paths can't be hit in a unit test without a
    /// running server.  These helpers let the unit tests build a runtime
    /// already in the "registered" state so they can exercise downstream
    /// logic (e.g. `maintain_registration`) in isolation.
    #[cfg(test)]
    pub(super) fn set_server_url_for_test(&mut self, url: Option<String>) {
        self.server_url = url;
    }

    #[cfg(test)]
    pub(super) fn set_registration_for_test(&mut self, registration: Option<ReceiverRegistration>) {
        self.registration = registration;
    }

    #[cfg(test)]
    pub(super) fn registration_for_test(&self) -> Option<&ReceiverRegistration> {
        self.registration.as_ref()
    }

    pub(super) fn has_registration(&self) -> bool {
        self.registration.is_some()
    }

    pub(super) fn has_pending_offer(&self) -> bool {
        matches!(self.offer_state, OfferState::Pending(_))
    }

    pub(super) fn is_available_for_new_offer(&self) -> bool {
        matches!(self.offer_state, OfferState::Idle)
    }

    pub(super) fn advertising_active(&self) -> bool {
        self.advertising.is_some()
    }

    pub(super) fn clear_advertising(&mut self) {
        self.advertising.take();
    }

    pub(super) async fn close_endpoint(&self) {
        self.endpoint.close().await;
    }

    /// Tear down the inbound-ALPN router so no new connections are accepted.
    /// Idempotent — safe to call multiple times.  We `take()` the router so
    /// a follow-up shutdown call doesn't double-shutdown.
    pub(super) async fn shutdown_router(&mut self) {
        if let Some(router) = self.router.take() {
            if let Err(err) = router.shutdown().await {
                tracing::warn!(
                    target: "drift_app::receiver::runtime",
                    %err,
                    "router shutdown returned an error; ignoring"
                );
            }
        }
    }

    pub(super) async fn handle_setup(
        &mut self,
        server_url: Option<String>,
        pairing_tx: &watch::Sender<PairingCodeState>,
        event_tx: &broadcast::Sender<ReceiverEvent>,
    ) -> AppResult<ReceiverRegistration> {
        self.server_url = Some(resolve_server_url(server_url.as_deref()));
        let was_active = self.advertising_active();
        let result = self.ensure_registered_with_current_server().await;
        match &result {
            Ok(registration) => {
                let _ = pairing_tx.send(PairingCodeState::Active(registration.clone()));
                let _ = event_tx.send(ReceiverEvent::SetupCompleted(registration.clone()));
            }
            Err(_) => {
                self.reconcile_advertising().await;
                let _ = pairing_tx.send(PairingCodeState::Unavailable);
            }
        }
        self.publish_discoverability_change_if_needed(was_active, event_tx);
        result
    }

    pub(super) async fn handle_ensure_registered(
        &mut self,
        server_url: Option<String>,
        pairing_tx: &watch::Sender<PairingCodeState>,
        event_tx: &broadcast::Sender<ReceiverEvent>,
    ) -> AppResult<ReceiverRegistration> {
        let was_active = self.advertising_active();
        let result = self.ensure_registered(server_url).await;
        match &result {
            Ok(registration) => {
                let _ = pairing_tx.send(PairingCodeState::Active(registration.clone()));
                let _ = event_tx.send(ReceiverEvent::RegistrationUpdated(registration.clone()));
            }
            Err(_) => {
                self.reconcile_advertising().await;
                let _ = pairing_tx.send(PairingCodeState::Unavailable);
            }
        }
        self.publish_discoverability_change_if_needed(was_active, event_tx);
        result
    }

    pub(super) async fn ensure_registered(
        &mut self,
        server_url: Option<String>,
    ) -> AppResult<ReceiverRegistration> {
        self.server_url = Some(resolve_server_url(server_url.as_deref()));
        self.ensure_registered_with_current_server().await
    }

    async fn ensure_registered_with_current_server(&mut self) -> AppResult<ReceiverRegistration> {
        let resolved_url = self
            .server_url
            .clone()
            .ok_or(AppError::ReceiverSetupIncomplete)?;
        let ticket = make_ticket(&self.endpoint)
            .await
            .map_err(|e| AppError::Internal {
                message: e.to_string(),
            })?;
        let registration = RendezvousClient::new(resolved_url)
            .register_peer(ticket)
            .await
            .map_err(|e| AppError::Internal {
                message: e.to_string(),
            })?;
        let registration = ReceiverRegistration {
            code: registration.code,
            expires_at: registration.expires_at,
        };
        self.registration = Some(registration.clone());
        self.reconcile_advertising().await;
        Ok(registration)
    }

    pub(super) async fn refresh_registration_after_offer(
        &mut self,
        pairing_tx: &watch::Sender<PairingCodeState>,
        event_tx: &broadcast::Sender<ReceiverEvent>,
    ) -> AppResult<Option<ReceiverRegistration>> {
        let Some(_) = self.server_url else {
            return Ok(None);
        };
        let was_active = self.advertising_active();
        let registration = self.ensure_registered_with_current_server().await?;
        let _ = pairing_tx.send(PairingCodeState::Active(registration.clone()));
        let _ = event_tx.send(ReceiverEvent::RegistrationUpdated(registration.clone()));
        self.publish_discoverability_change_if_needed(was_active, event_tx);
        Ok(Some(registration))
    }

    pub(super) async fn maintain_registration(
        &mut self,
        pairing_tx: &watch::Sender<PairingCodeState>,
        event_tx: &broadcast::Sender<ReceiverEvent>,
    ) -> AppResult<()> {
        let Some(server_url) = self.server_url.clone() else {
            return Ok(());
        };

        let Some(existing) = self.registration.clone() else {
            let registration = self.ensure_registered(Some(server_url)).await?;
            let _ = pairing_tx.send(PairingCodeState::Active(registration.clone()));
            let _ = event_tx.send(ReceiverEvent::RegistrationUpdated(registration));
            return Ok(());
        };

        if registration_needs_refresh(&existing) {
            let was_active = self.advertising_active();
            let registration = self.ensure_registered(Some(server_url)).await?;
            let _ = pairing_tx.send(PairingCodeState::Active(registration.clone()));
            let _ = event_tx.send(ReceiverEvent::RegistrationUpdated(registration));
            self.publish_discoverability_change_if_needed(was_active, event_tx);
            return Ok(());
        }

        // Poll the rendezvous server for the current claim status of our
        // pairing code.  Two outcomes that matter to us:
        //
        // 1. `Some(_)` — server still has an Open session under our code.
        //    Nothing to do; the code is still usable for a new sender.
        // 2. `None` — server returned 404, meaning the session was either
        //    claimed (sender pulled the ticket) or purged (TTL).  Either
        //    way the visible code is dead; we mint a new one immediately
        //    so the user always sees a usable code, regardless of whether
        //    the previous sender ever successfully connected.  This is
        //    the "no grace period" policy: if a sender claims and then
        //    fails to dial us, the previously-claimed code is gone and
        //    we'd rather show a fresh one than leave the user staring at
        //    a code that can never be claimed again until TTL.
        //
        // If the rotation itself fails (network blip, server 5xx, …) we
        // emit `Stale(existing)` so the UI can prompt the user to tap
        // Refresh manually — keeping the user informed without retrying
        // aggressively on a flaky network.
        let pair_status = RendezvousClient::new(server_url.clone())
            .pair_status(&existing.code)
            .await;

        match pair_status {
            Ok(Some(_)) => Ok(()),
            Ok(None) => {
                let was_active = self.advertising_active();
                match self.ensure_registered_with_current_server().await {
                    Ok(registration) => {
                        let _ = pairing_tx.send(PairingCodeState::Active(registration.clone()));
                        let _ = event_tx.send(ReceiverEvent::RegistrationUpdated(registration));
                        self.publish_discoverability_change_if_needed(was_active, event_tx);
                        Ok(())
                    }
                    Err(err) => {
                        tracing::warn!(
                            target: "drift_app::receiver::runtime",
                            %err,
                            code = %existing.code,
                            "pair status returned 404 but auto-rotation failed; \
                             marking pairing code as Stale and waiting for the \
                             user to tap Refresh"
                        );
                        let _ = pairing_tx.send(PairingCodeState::Stale(existing.clone()));
                        Ok(())
                    }
                }
            }
            Err(err) => {
                // Network/transport error talking to the rendezvous server.
                // Don't downgrade visible state to Stale on a single failure —
                // that would flicker the UI on flaky connections.  Just log
                // and try again on the next tick.
                tracing::debug!(
                    target: "drift_app::receiver::runtime",
                    %err,
                    "pair_status request failed; will retry on next maintenance tick"
                );
                Ok(())
            }
        }
    }

    pub(super) async fn set_discoverable(&mut self, enabled: bool) -> AppResult<()> {
        self.discoverable_requested = enabled;
        self.reconcile_advertising().await;
        Ok(())
    }

    pub(super) fn respond_to_offer(&mut self, decision: OfferDecision) -> AppResult<()> {
        let OfferState::Pending(pending_offer) =
            std::mem::replace(&mut self.offer_state, OfferState::Idle)
        else {
            return Err(AppError::NoPendingOffer);
        };
        let run = pending_offer.run;

        let offer_id = run.offer_id;
        let resolution = if matches!(decision, OfferDecision::Accept) {
            self.offer_state = OfferState::Receiving {
                offer_id,
                cancel_tx: run.cancel_tx.clone(),
            };
            OfferResolution::Accept
        } else {
            OfferResolution::Decline
        };
        run.decision_tx
            .send(resolution)
            .map_err(|_| AppError::OfferNoLongerActive)?;
        Ok(())
    }

    pub(super) fn handle_offer_prepared(&mut self, run: ReceiverRun) -> bool {
        if !matches!(self.offer_state, OfferState::Idle) {
            let state_label = match &self.offer_state {
                OfferState::Idle => "idle",
                OfferState::Pending(_) => "pending",
                OfferState::Receiving { .. } => "receiving",
            };
            tracing::warn!(
                target: "drift_app::receiver::runtime",
                offer_id = run.offer_id,
                state = state_label,
                "auto-declining new offer because runtime is not idle"
            );
            let _ = run.decision_tx.send(OfferResolution::Decline);
            return false;
        }

        tracing::info!(
            target: "drift_app::receiver::runtime",
            offer_id = run.offer_id,
            "accepted offer into Pending state, broadcasting OfferUpdated"
        );
        self.offer_state = OfferState::Pending(PendingOfferState { run });
        true
    }

    pub(super) fn handle_offer_progress(&mut self, offer_id: u64) -> bool {
        match &mut self.offer_state {
            OfferState::Pending(pending) if pending.run.offer_id == offer_id => {
                self.offer_state = OfferState::Receiving {
                    offer_id,
                    cancel_tx: pending.run.cancel_tx.clone(),
                };
                true
            }
            OfferState::Receiving {
                offer_id: active_offer_id,
                ..
            } if *active_offer_id == offer_id => true,
            _ => false,
        }
    }

    pub(super) fn handle_offer_finished(&mut self, offer_id: u64) -> bool {
        if offer_id == 0 {
            self.offer_state = OfferState::Idle;
            return true;
        }

        match &mut self.offer_state {
            OfferState::Pending(pending) if pending.run.offer_id == offer_id => {
                self.offer_state = OfferState::Idle;
                true
            }
            OfferState::Receiving {
                offer_id: active_offer_id,
                ..
            } if *active_offer_id == offer_id => {
                self.offer_state = OfferState::Idle;
                true
            }
            _ => false,
        }
    }

    pub(super) fn cancel_active_transfer(&mut self) -> AppResult<()> {
        match &self.offer_state {
            OfferState::Receiving { cancel_tx, .. } => {
                let _ = cancel_tx.send(true);
                Ok(())
            }
            _ => Err(AppError::NoActiveTransfer),
        }
    }

    fn cancel_pending_offer(&mut self, offer_id: u64) -> bool {
        match std::mem::replace(&mut self.offer_state, OfferState::Idle) {
            OfferState::Pending(pending) if pending.run.offer_id == offer_id => {
                let _ = pending.run.decision_tx.send(OfferResolution::Cancel);
                true
            }
            other => {
                self.offer_state = other;
                false
            }
        }
    }

    async fn reconcile_advertising(&mut self) {
        if !should_advertise(self.discoverable_requested, self.registration.is_some()) {
            self.clear_advertising();
            return;
        }

        // Already advertising — keep the existing advertisement alive instead of
        // tearing it down and re-creating it (which causes a gap in mDNS coverage
        // and triggers repeated `lan_advertisement.starting` log entries).
        if self.advertising.is_some() {
            return;
        }
        let ticket = match make_ticket(&self.endpoint).await {
            Ok(ticket) => ticket,
            Err(error) => {
                warn!(
                    device = %self.config.device_name,
                    error = %error,
                    error_chain = %format!("{error:#}"),
                    "receiver.lan_advertising_unavailable"
                );
                return;
            }
        };

        let device_type = match parse_device_type(&self.config.device_type) {
            Ok(device_type) => device_type,
            Err(error) => {
                warn!(
                    device = %self.config.device_name,
                    error = %error,
                    "receiver.lan_advertising_invalid_device_type"
                );
                return;
            }
        };

        match LanReceiveAdvertisement::start(&ticket, &self.config.device_name, device_type) {
            Ok(Some(advertising)) => {
                self.advertising = Some(advertising);
            }
            Ok(None) => {
                warn!(
                    device = %self.config.device_name,
                    "receiver.lan_advertising_unavailable_no_ipv4_route"
                );
            }
            Err(error) => {
                warn!(
                    device = %self.config.device_name,
                    error = %error,
                    error_chain = %format!("{error:#}"),
                    "receiver.lan_advertising_unavailable"
                );
            }
        }
    }

    pub(super) fn publish_discoverability_change_if_needed(
        &self,
        was_active: bool,
        event_tx: &broadcast::Sender<ReceiverEvent>,
    ) {
        let is_active = self.advertising_active();
        if was_active != is_active || !self.discoverable_requested {
            let _ = event_tx.send(ReceiverEvent::DiscoverabilityChanged {
                requested: self.discoverable_requested,
                active: is_active,
            });
        }
    }
}

pub(super) fn registration_needs_refresh(registration: &ReceiverRegistration) -> bool {
    let Ok(expires_at) = OffsetDateTime::parse(&registration.expires_at, &Rfc3339) else {
        return true;
    };
    OffsetDateTime::now_utc() >= expires_at
}

pub(super) fn should_advertise(discoverable_requested: bool, _has_registration: bool) -> bool {
    discoverable_requested
}
