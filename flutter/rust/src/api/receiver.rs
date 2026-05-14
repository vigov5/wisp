use std::path::PathBuf;
use std::sync::{Arc, LazyLock, Mutex};

use drift_app::{
    ConflictPolicy, ConnectionPath as AppConnectionPath, OfferDecision, PairingCodeState,
    QrPairingInfo as AppQrPairingInfo, ReceiverConfig, ReceiverEvent as AppReceiverEvent,
    ReceiverOfferEvent as AppReceiverOfferEvent, ReceiverOfferFile as AppReceiverOfferFile,
    ReceiverOfferPhase as AppReceiverOfferPhase, ReceiverRegistration as AppReceiverRegistration,
    ReceiverService, identity as app_identity,
};
use drift_core::transfer::{TransferPhase, TransferPlan, TransferPlanFile, TransferSnapshot};
use tokio::sync::Mutex as AsyncMutex;
use tokio::task::JoinHandle;

use super::transfer::{
    TransferPhaseData, TransferPlanData, TransferPlanFileData, TransferSnapshotData,
};
use super::RUNTIME;
use crate::api::error::{internal_user_facing_error, map_optional_user_facing_error};
use crate::frb_generated::StreamSink;

static RECEIVER_STATE: LazyLock<Mutex<Option<BridgeReceiverState>>> =
    LazyLock::new(|| Mutex::new(None));
static RECEIVER_SERVICE_LOCK: LazyLock<AsyncMutex<()>> = LazyLock::new(|| AsyncMutex::new(()));
const ENABLE_DEMO_HELLO_PROTOCOL: bool = false;

#[derive(Clone, Debug, PartialEq, Eq)]
struct BridgeReceiverConfig {
    device_name: String,
    device_type: String,
    download_root: PathBuf,
    server_url: Option<String>,
    conflict_policy: ConflictPolicy,
}

struct BridgeReceiverState {
    config: BridgeReceiverConfig,
    service: Arc<ReceiverService>,
    updates_task: Option<JoinHandle<()>>,
    pairing_task: Option<JoinHandle<()>>,
}

#[derive(Debug, Clone)]
pub struct ReceiverRegistration {
    pub code: String,
    pub expires_at: String,
}

#[derive(Debug, Clone)]
pub struct ReceiverPairingState {
    pub code: Option<String>,
    pub expires_at: Option<String>,
    /// `true` when the rendezvous server has told us the code is no longer
    /// claimable (likely a sender has already consumed it) but background
    /// re-registration is currently failing.  Dart UI should keep the code
    /// visible but prompt the user to tap Refresh.
    pub stale: bool,
}

#[derive(Clone, Debug)]
pub enum ReceiverTransferPhase {
    Connecting,
    OfferReady,
    Receiving,
    Completed,
    Cancelled,
    Failed,
    Declined,
}

#[derive(Clone, Debug)]
pub struct ReceiverTransferFile {
    pub path: String,
    pub size: u64,
}

#[derive(Clone, Debug)]
pub struct ReceiverConnectionPath {
    pub kind: String,
    pub relay_url: Option<String>,
    pub direct_addr: Option<String>,
}

#[derive(Clone, Debug)]
pub struct ReceiverTransferEvent {
    pub phase: ReceiverTransferPhase,
    pub sender_name: String,
    pub sender_device_type: String,
    pub destination_label: String,
    pub save_root_label: String,
    pub status_message: String,
    pub item_count: u64,
    pub total_size_bytes: u64,
    pub bytes_received: u64,
    pub plan: Option<TransferPlanData>,
    pub snapshot: Option<TransferSnapshotData>,
    pub total_size_label: String,
    pub files: Vec<ReceiverTransferFile>,
    pub connection_path: Option<ReceiverConnectionPath>,
    pub sender_endpoint_id: Option<String>,
    pub sender_ticket: Option<String>,
    pub error: Option<crate::api::error::UserFacingErrorData>,
}

pub fn register_receiver(
    server_url: Option<String>,
    device_name: String,
) -> Result<ReceiverRegistration, crate::api::error::UserFacingErrorData> {
    ensure_receiver_registration(server_url, device_name)
}

pub fn ensure_receiver_registration(
    server_url: Option<String>,
    device_name: String,
) -> Result<ReceiverRegistration, crate::api::error::UserFacingErrorData> {
    RUNTIME.block_on(async move {
        // Prefer the live service so a UI-driven refresh (Refresh button)
        // rotates the visible code without shutting down the endpoint or
        // tearing down the pairing/offer relay tasks.  Building a synthetic
        // config here would have a different `device_type`/`download_root`
        // than `watch_receiver_pairing` set up, causing `ensure_receiver_service`
        // to treat it as a new identity and rebuild from scratch.
        if let Some(service) = current_service() {
            return service
                .ensure_registered(server_url)
                .await
                .map(map_registration)
                .map_err(|e| {
                    internal_user_facing_error("Receiver registration failed", e.to_string())
                });
        }

        // Cold-start fallback: the bridge state hasn't been populated yet
        // (no `watch_receiver_pairing` call).  Spin one up so the very first
        // explicit registration request still works.  device_type / download_root
        // are placeholders here — the subsequent `watch_receiver_pairing` call
        // will provide the real values and reuse the same service.
        let service = ensure_receiver_service(BridgeReceiverConfig {
            device_name,
            device_type: "laptop".to_owned(),
            download_root: PathBuf::from("."),
            server_url: server_url.clone(),
            conflict_policy: ConflictPolicy::Rename,
        })
        .await
        .map_err(|e| internal_user_facing_error("Receiver unavailable", e))?;

        service
            .ensure_registered(server_url)
            .await
            .map(map_registration)
            .map_err(|e| internal_user_facing_error("Receiver registration failed", e.to_string()))
    })
}

pub fn current_receiver_registration() -> Option<ReceiverRegistration> {
    current_service()
        .and_then(|service| pairing_registration(&service.pairing_code()))
        .map(map_registration)
}

#[derive(Clone, Debug)]
pub struct QrPairingInfoData {
    pub ticket: String,
    pub lan_ips: Vec<String>,
}

/// Returns a ticket plus the LAN-routable IPs of the local receiver.
/// The receiver service must already be running.  Designed for the QR
/// pairing screen — the ticket is built from currently-known addresses
/// without waiting on relay handshake, so it works on offline-LAN.
pub fn current_qr_pairing_info()
-> Result<QrPairingInfoData, crate::api::error::UserFacingErrorData> {
    let Some(service) = current_service() else {
        return Err(internal_user_facing_error(
            "Receiver unavailable",
            "The receiver is not running.",
        ));
    };
    service
        .qr_pairing_info()
        .map(map_qr_pairing_info)
        .map_err(|err| internal_user_facing_error("Couldn't build QR ticket", err.to_string()))
}

fn map_qr_pairing_info(info: AppQrPairingInfo) -> QrPairingInfoData {
    QrPairingInfoData {
        ticket: info.ticket,
        lan_ips: info.lan_ips,
    }
}

pub fn watch_receiver_pairing(
    server_url: Option<String>,
    download_root: String,
    device_name: String,
    device_type: String,
    updates: StreamSink<ReceiverPairingState>,
) -> Result<(), crate::api::error::UserFacingErrorData> {
    RUNTIME.block_on(async move {
        let config = BridgeReceiverConfig {
            device_name,
            device_type,
            download_root: PathBuf::from(download_root),
            server_url: server_url.clone(),
            conflict_policy: ConflictPolicy::Rename,
        };
        let service = ensure_receiver_service(config.clone())
            .await
            .map_err(|e| internal_user_facing_error("Receiver unavailable", e))?;
        if let Some(server_url) = config.server_url.clone() {
            if let Err(error) = service.ensure_registered(Some(server_url)).await {
                println!(
                    "[bridge] receiver pairing registration unavailable: {}",
                    error
                );
            }
        }
        service
            .set_discoverable(true)
            .await
            .map_err(|e| internal_user_facing_error("Receiver unavailable", e.to_string()))?;

        replace_pairing_task(config, service, updates);
        Ok(())
    })
}

pub fn set_receiver_discoverable(
    enabled: bool,
) -> Result<(), crate::api::error::UserFacingErrorData> {
    set_discoverable(enabled)
}

pub fn start_receiver_transfer_listener(
    server_url: Option<String>,
    download_root: String,
    device_name: String,
    device_type: String,
    updates: StreamSink<ReceiverTransferEvent>,
) -> Result<(), crate::api::error::UserFacingErrorData> {
    if ENABLE_DEMO_HELLO_PROTOCOL {
        std::env::set_var("DRIFT_DEMO_HELLO", "1");
        println!("[bridge/receive] demo hello protocol enabled");
    }

    RUNTIME.block_on(async move {
        let config = BridgeReceiverConfig {
            device_name,
            device_type,
            download_root: PathBuf::from(download_root),
            server_url: server_url.clone(),
            conflict_policy: ConflictPolicy::Rename,
        };
        let service = ensure_receiver_service(config.clone())
            .await
            .map_err(|e| internal_user_facing_error("Receiver unavailable", e))?;
        if let Some(server_url) = config.server_url.clone() {
            if let Err(error) = service.ensure_registered(Some(server_url)).await {
                println!(
                    "[bridge] receiver transfer listener registration unavailable: {}",
                    error
                );
            }
        }
        service
            .set_discoverable(true)
            .await
            .map_err(|e| internal_user_facing_error("Receiver unavailable", e.to_string()))?;

        replace_updates_task(config, service, updates);
        Ok(())
    })
}

pub fn respond_to_receiver_offer(
    accept: bool,
) -> Result<(), crate::api::error::UserFacingErrorData> {
    RUNTIME.block_on(async move {
        let Some(service) = current_service() else {
            return Err(internal_user_facing_error(
                "Receiver unavailable",
                "The receiver is not running.",
            ));
        };
        service
            .respond_to_offer(if accept {
                OfferDecision::Accept
            } else {
                OfferDecision::Decline
            })
            .await
            .map_err(|e| internal_user_facing_error("Receiver unavailable", e.to_string()))
    })
}

pub fn cancel_receiver_transfer() -> Result<(), crate::api::error::UserFacingErrorData> {
    RUNTIME.block_on(async move {
        let Some(service) = current_service() else {
            return Err(internal_user_facing_error(
                "Receiver unavailable",
                "The receiver is not running.",
            ));
        };
        service
            .cancel_transfer()
            .await
            .map_err(|e| internal_user_facing_error("Receiver unavailable", e.to_string()))
    })
}

pub(crate) async fn scan_nearby_with_receiver(
    timeout_secs: u64,
) -> Result<Vec<crate::api::lan::NearbyReceiverInfo>, crate::api::error::UserFacingErrorData> {
    println!(
        "[bridge] scanning nearby receivers (timeout={}s)",
        timeout_secs
    );
    let service = match current_service() {
        Some(service) => service,
        None => {
            println!("[bridge] starting temporary receiver service for scan");
            let temp = ReceiverService::start(ReceiverConfig {
                device_name: String::new(),
                device_type: "laptop".to_owned(),
                download_root: PathBuf::from("."),
                conflict_policy: ConflictPolicy::Rename,
                secret_key: app_identity::current_secret_key(),
            })
            .await
            .map_err(|e| internal_user_facing_error("Receiver unavailable", e.to_string()))?;
            let receivers = temp
                .scan_nearby(timeout_secs)
                .await
                .map_err(|e| internal_user_facing_error("Receiver unavailable", e.to_string()))?;
            println!("[bridge] scan found {} receivers", receivers.len());
            for r in &receivers {
                println!(
                    "[bridge]   - found receiver: name='{}' label='{}' code='{}'",
                    r.fullname, r.label, r.code
                );
            }
            let _ = temp.shutdown().await;
            return Ok(receivers
                .into_iter()
                .map(crate::api::lan::map_nearby_receiver)
                .collect());
        }
    };

    let receivers = service
        .scan_nearby(timeout_secs)
        .await
        .map_err(|e| internal_user_facing_error("Receiver unavailable", e.to_string()))?;

    println!("[bridge] scan found {} receivers", receivers.len());
    for r in &receivers {
        println!(
            "[bridge]   - found receiver: name='{}' label='{}' code='{}'",
            r.fullname, r.label, r.code
        );
    }

    Ok(receivers
        .into_iter()
        .map(crate::api::lan::map_nearby_receiver)
        .collect())
}

async fn ensure_receiver_service(
    config: BridgeReceiverConfig,
) -> Result<Arc<ReceiverService>, String> {
    let _lock = RECEIVER_SERVICE_LOCK.lock().await;

    if let Some(service) = existing_service_for_config(&config) {
        return Ok(service);
    }

    println!(
        "[bridge] creating new receiver service: device_name='{}' device_type='{}'",
        config.device_name, config.device_type
    );

    let old_state = {
        let mut guard = RECEIVER_STATE
            .lock()
            .map_err(|_| "receiver bridge mutex poisoned".to_owned())?;
        guard.take()
    };

    if let Some(old_state) = old_state {
        if let Some(task) = old_state.updates_task {
            task.abort();
        }
        if let Some(task) = old_state.pairing_task {
            task.abort();
        }
        let _ = old_state.service.shutdown().await;
    }

    let service = Arc::new(
        ReceiverService::start(ReceiverConfig {
            device_name: config.device_name.clone(),
            device_type: config.device_type.clone(),
            download_root: config.download_root.clone(),
            conflict_policy: config.conflict_policy,
            secret_key: app_identity::current_secret_key(),
        })
        .await
        .map_err(|e| format!("{e:#}"))?,
    );

    println!("[bridge] receiver service started");

    let mut guard = RECEIVER_STATE
        .lock()
        .map_err(|_| "receiver bridge mutex poisoned".to_owned())?;
    *guard = Some(BridgeReceiverState {
        config,
        service: service.clone(),
        updates_task: None,
        pairing_task: None,
    });
    Ok(service)
}

fn replace_updates_task(
    config: BridgeReceiverConfig,
    service: Arc<ReceiverService>,
    updates: StreamSink<ReceiverTransferEvent>,
) {
    let mut event_rx = service.subscribe_events();
    let task = RUNTIME.spawn(async move {
        let mut last_event: Option<ReceiverTransferEvent> = None;
        loop {
            match event_rx.recv().await {
                Ok(AppReceiverEvent::OfferUpdated(event)) => {
                    let mapped = map_event(event);
                    if matches!(
                        mapped.phase,
                        ReceiverTransferPhase::Completed
                            | ReceiverTransferPhase::Cancelled
                            | ReceiverTransferPhase::Failed
                            | ReceiverTransferPhase::Declined
                    ) {
                        last_event = None;
                    } else {
                        last_event = Some(mapped.clone());
                    }
                    let _ = updates.add(mapped);
                }
                Ok(AppReceiverEvent::ConnectionPathChanged {
                    offer_id: _,
                    connection_path,
                }) => {
                    if let Some(cached) = last_event.as_mut() {
                        cached.connection_path = Some(map_connection_path(connection_path));
                        let _ = updates.add(cached.clone());
                    }
                }
                Ok(AppReceiverEvent::Shutdown) => break,
                Ok(AppReceiverEvent::RegistrationUpdated(_))
                | Ok(AppReceiverEvent::SetupCompleted(_))
                | Ok(AppReceiverEvent::DiscoverabilityChanged { .. }) => {}
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
            }
        }
    });

    if let Ok(mut guard) = RECEIVER_STATE.lock() {
        if let Some(state) = guard.as_mut() {
            if state.config == config && Arc::ptr_eq(&state.service, &service) {
                if let Some(old_task) = state.updates_task.replace(task) {
                    old_task.abort();
                }
            }
        }
    }
}

fn replace_pairing_task(
    config: BridgeReceiverConfig,
    service: Arc<ReceiverService>,
    updates: StreamSink<ReceiverPairingState>,
) {
    let mut pairing_rx = service.subscribe_pairing_code();
    let task = RUNTIME.spawn(async move {
        let _ = updates.add(map_pairing_state(&pairing_rx.borrow().clone()));
        loop {
            if pairing_rx.changed().await.is_err() {
                break;
            }
            let _ = updates.add(map_pairing_state(&pairing_rx.borrow().clone()));
        }
    });

    if let Ok(mut guard) = RECEIVER_STATE.lock() {
        if let Some(state) = guard.as_mut() {
            if state.config == config && Arc::ptr_eq(&state.service, &service) {
                if let Some(old_task) = state.pairing_task.replace(task) {
                    old_task.abort();
                }
            }
        }
    }
}

fn existing_service_for_config(config: &BridgeReceiverConfig) -> Option<Arc<ReceiverService>> {
    let guard = RECEIVER_STATE.lock().ok()?;
    let state = guard.as_ref()?;
    (state.config == *config).then(|| state.service.clone())
}

fn current_service() -> Option<Arc<ReceiverService>> {
    RECEIVER_STATE
        .lock()
        .ok()
        .and_then(|guard| guard.as_ref().map(|state| state.service.clone()))
}

/// Used by the sender bridge to share the long-lived receiver endpoint
/// instead of binding a second iroh instance with the same secret key
/// (which would race with the receiver for the relay slot).
pub(crate) fn current_service_endpoint() -> Option<iroh::Endpoint> {
    current_service().map(|service| service.endpoint())
}

fn set_discoverable(enabled: bool) -> Result<(), crate::api::error::UserFacingErrorData> {
    println!("[bridge] setting discoverable: {}", enabled);
    RUNTIME.block_on(async move {
        let Some(service) = current_service() else {
            println!("[bridge] WARNING: set_discoverable called but no service running");
            return Ok(());
        };
        service
            .set_discoverable(enabled)
            .await
            .map_err(|e| internal_user_facing_error("Receiver unavailable", e.to_string()))
    })
}

fn pairing_registration(state: &PairingCodeState) -> Option<AppReceiverRegistration> {
    match state {
        PairingCodeState::Unavailable => None,
        PairingCodeState::Active(registration)
        | PairingCodeState::Stale(registration) => Some(registration.clone()),
    }
}

fn map_registration(value: AppReceiverRegistration) -> ReceiverRegistration {
    ReceiverRegistration {
        code: value.code,
        expires_at: value.expires_at,
    }
}

fn map_pairing_state(state: &PairingCodeState) -> ReceiverPairingState {
    match state {
        PairingCodeState::Unavailable => ReceiverPairingState {
            code: None,
            expires_at: None,
            stale: false,
        },
        PairingCodeState::Active(registration) => ReceiverPairingState {
            code: Some(registration.code.clone()),
            expires_at: Some(registration.expires_at.clone()),
            stale: false,
        },
        PairingCodeState::Stale(registration) => ReceiverPairingState {
            code: Some(registration.code.clone()),
            expires_at: Some(registration.expires_at.clone()),
            stale: true,
        },
    }
}

fn map_event(event: AppReceiverOfferEvent) -> ReceiverTransferEvent {
    ReceiverTransferEvent {
        phase: match event.phase {
            AppReceiverOfferPhase::Connecting => ReceiverTransferPhase::Connecting,
            AppReceiverOfferPhase::OfferReady => ReceiverTransferPhase::OfferReady,
            AppReceiverOfferPhase::Receiving => ReceiverTransferPhase::Receiving,
            AppReceiverOfferPhase::Completed => ReceiverTransferPhase::Completed,
            AppReceiverOfferPhase::Cancelled => ReceiverTransferPhase::Cancelled,
            AppReceiverOfferPhase::Failed => ReceiverTransferPhase::Failed,
            AppReceiverOfferPhase::Declined => ReceiverTransferPhase::Declined,
        },
        sender_name: event.sender_name,
        sender_device_type: event.sender_device_type,
        destination_label: event.destination_label,
        save_root_label: event.save_root_label,
        status_message: event.status_message,
        item_count: event.item_count,
        total_size_bytes: event.total_size_bytes,
        bytes_received: event.bytes_received,
        plan: event.plan.map(map_plan),
        snapshot: event.snapshot.map(map_snapshot),
        total_size_label: event.total_size_label,
        files: event.files.into_iter().map(map_file_row).collect(),
        connection_path: event.connection_path.map(map_connection_path),
        sender_endpoint_id: event.sender_endpoint_id,
        sender_ticket: event.sender_ticket,
        error: map_optional_user_facing_error(event.error),
    }
}

fn map_connection_path(path: AppConnectionPath) -> ReceiverConnectionPath {
    ReceiverConnectionPath {
        kind: path.label().to_owned(),
        relay_url: path.relay_url,
        direct_addr: path.direct_addr,
    }
}

fn map_plan(plan: TransferPlan) -> TransferPlanData {
    TransferPlanData {
        session_id: plan.session_id,
        total_files: plan.total_files,
        total_bytes: plan.total_bytes,
        files: plan.files.into_iter().map(map_plan_file).collect(),
    }
}

fn map_plan_file(file: TransferPlanFile) -> TransferPlanFileData {
    TransferPlanFileData {
        id: file.id,
        path: file.path,
        size: file.size,
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

fn map_file_row(row: AppReceiverOfferFile) -> ReceiverTransferFile {
    ReceiverTransferFile {
        path: row.path,
        size: row.size,
    }
}
