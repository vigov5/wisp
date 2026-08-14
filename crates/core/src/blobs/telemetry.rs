use std::net::SocketAddr;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use std::time::{Duration, Instant};

use iroh::{
    TransportAddr, Watcher,
    endpoint::{ConnectionInfo, PathId},
};
use tokio::{sync::oneshot, task::JoinHandle};
use tracing::debug;

use super::receive::BlobTransportProfile;
use crate::lan::in_usb_tunnel_subnet;

pub(super) const TELEMETRY_SAMPLE_INTERVAL: Duration = Duration::from_millis(250);
const STALL_THRESHOLD: Duration = Duration::from_millis(500);
const TELEMETRY_TARGET: &str = "wisp_transfer_telemetry";
const BUILD_PROFILE: &str = if cfg!(debug_assertions) {
    "debug"
} else {
    "release"
};
static NEXT_TRANSFER_ID: AtomicU64 = AtomicU64::new(1);
const UNSET_PROGRESS_TIME_NANOS: u64 = u64::MAX;
const RUN_ID_DERIVATION_CONTEXT: &str = "wisp telemetry correlation token v1";

pub(super) fn is_enabled() -> bool {
    tracing::enabled!(target: TELEMETRY_TARGET, tracing::Level::DEBUG)
}

/// Sender-generated session IDs are 16 lowercase hexadecimal characters.
///
/// Deriving a domain-separated token lets both endpoints correlate opt-in
/// telemetry without logging either the raw ID or a trivially reversible
/// base-conversion of it. This is pseudonymization, not anonymization.
pub(crate) fn benchmark_run_id(session_id: &str) -> Option<u64> {
    let valid = session_id.len() == 16
        && session_id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
    valid.then(|| {
        let digest = blake3::derive_key(RUN_ID_DERIVATION_CONTEXT, session_id.as_bytes());
        u64::from_le_bytes(
            digest[..8]
                .try_into()
                .expect("BLAKE3 output has eight bytes"),
        )
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TelemetryRole {
    Sender,
    Receiver,
}

impl TelemetryRole {
    fn label(self) -> &'static str {
        match self {
            Self::Sender => "sender",
            Self::Receiver => "receiver",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TransferPhase {
    PrepareTotal,
    WalkMetadata,
    ImportHash,
    CollectionStore,
    Dial,
    ControlHandshake,
    DecisionWait,
    BlobSetup,
    FetchStore,
    Export,
    FinalAck,
}

impl TransferPhase {
    fn label(self) -> &'static str {
        match self {
            Self::PrepareTotal => "prepare_total",
            Self::WalkMetadata => "walk_metadata",
            Self::ImportHash => "import_hash",
            Self::CollectionStore => "collection_store",
            Self::Dial => "dial",
            Self::ControlHandshake => "control_handshake",
            Self::DecisionWait => "decision_wait",
            Self::BlobSetup => "blob_setup",
            Self::FetchStore => "fetch_store",
            Self::Export => "export",
            Self::FinalAck => "final_ack",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PhaseOutcome {
    Complete,
    Failed,
    Cancelled,
    Skipped,
}

impl PhaseOutcome {
    fn label(self) -> &'static str {
        match self {
            Self::Complete => "complete",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Skipped => "skipped",
        }
    }
}

pub(crate) fn emit_phase(
    role: TelemetryRole,
    phase: TransferPhase,
    run_id: Option<u64>,
    elapsed: Duration,
    outcome: PhaseOutcome,
    bytes_total: u64,
    file_count: usize,
) {
    debug!(
        target: TELEMETRY_TARGET,
        event = "blob_phase",
        role = role.label(),
        benchmark_run_id_available = run_id.is_some(),
        benchmark_run_id = run_id.unwrap_or(0),
        phase = phase.label(),
        outcome = outcome.label(),
        elapsed_ms = duration_millis(elapsed),
        bytes_total,
        file_count = u64::try_from(file_count).unwrap_or(u64::MAX),
        "blob transfer phase timing"
    );
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TransferEnd {
    Complete,
    Failed,
}

impl TransferEnd {
    fn label(self) -> &'static str {
        match self {
            Self::Complete => "complete",
            Self::Failed => "failed",
        }
    }
}

#[derive(Debug)]
pub(super) struct BlobTransferTelemetry {
    progress: Arc<PublishedProgress>,
    stop_tx: Option<oneshot::Sender<TransferEnd>>,
    sampler_task: Option<JoinHandle<()>>,
}

#[derive(Debug)]
struct PublishedProgress {
    started_at: Instant,
    bytes: AtomicU64,
    first_elapsed_nanos: AtomicU64,
    latest_elapsed_nanos: AtomicU64,
}

#[derive(Debug, Clone, Copy)]
struct PublishedProgressSnapshot {
    bytes: u64,
    first_at: Instant,
    latest_at: Instant,
}

impl PublishedProgress {
    fn new(started_at: Instant) -> Self {
        Self {
            started_at,
            bytes: AtomicU64::new(0),
            first_elapsed_nanos: AtomicU64::new(UNSET_PROGRESS_TIME_NANOS),
            latest_elapsed_nanos: AtomicU64::new(UNSET_PROGRESS_TIME_NANOS),
        }
    }

    /// The blob download loop is the single writer. Publish timestamps before
    /// the byte position so the sampler's acquire load observes a coherent
    /// progress event without adding a lock or await to the hot path.
    fn publish(&self, now: Instant, bytes_received: u64) {
        if bytes_received <= self.bytes.load(Ordering::Relaxed) {
            return;
        }
        let elapsed_nanos = duration_nanos(now.saturating_duration_since(self.started_at));
        let _ = self.first_elapsed_nanos.compare_exchange(
            UNSET_PROGRESS_TIME_NANOS,
            elapsed_nanos,
            Ordering::Relaxed,
            Ordering::Relaxed,
        );
        self.latest_elapsed_nanos
            .store(elapsed_nanos, Ordering::Relaxed);
        self.bytes.store(bytes_received, Ordering::Release);
    }

    fn snapshot(&self, fallback_now: Instant) -> PublishedProgressSnapshot {
        let bytes = self.bytes.load(Ordering::Acquire);
        let first_at = self.instant_from_elapsed(
            self.first_elapsed_nanos.load(Ordering::Relaxed),
            fallback_now,
        );
        let latest_at = self.instant_from_elapsed(
            self.latest_elapsed_nanos.load(Ordering::Relaxed),
            fallback_now,
        );
        PublishedProgressSnapshot {
            bytes,
            first_at,
            latest_at: latest_at.max(first_at),
        }
    }

    fn instant_from_elapsed(&self, elapsed_nanos: u64, fallback: Instant) -> Instant {
        if elapsed_nanos == UNSET_PROGRESS_TIME_NANOS {
            return fallback;
        }
        self.started_at
            .checked_add(Duration::from_nanos(elapsed_nanos))
            .unwrap_or(fallback)
    }
}

impl BlobTransferTelemetry {
    pub(super) fn start(
        started_at: Instant,
        connection: ConnectionInfo,
        transport_profile: BlobTransportProfile,
        benchmark_run_id: Option<u64>,
    ) -> Self {
        let progress = Arc::new(PublishedProgress::new(started_at));
        let sampler_progress = Arc::clone(&progress);
        let (stop_tx, stop_rx) = oneshot::channel();
        let sampler_task = tokio::spawn(run_sampler(
            started_at,
            connection,
            transport_profile,
            benchmark_run_id,
            sampler_progress,
            stop_rx,
        ));
        Self {
            progress,
            stop_tx: Some(stop_tx),
            sampler_task: Some(sampler_task),
        }
    }

    /// Publish only the latest monotonic byte position. The sampler owns all
    /// timing and network-stat work, so the raw download loop never waits for
    /// telemetry and never cancels `stream.next()` on a timer tick.
    pub(super) fn observe_progress(&self, now: Instant, bytes_received: u64) {
        self.progress.publish(now, bytes_received);
    }

    pub(super) async fn finish(&mut self, outcome: TransferEnd) {
        if let Some(stop_tx) = self.stop_tx.take() {
            let _ = stop_tx.send(outcome);
        }
        if let Some(sampler_task) = self.sampler_task.take()
            && let Err(error) = sampler_task.await
        {
            debug!(?error, "blob telemetry sampler task failed");
        }
    }
}

async fn run_sampler(
    now: Instant,
    connection: ConnectionInfo,
    transport_profile: BlobTransportProfile,
    benchmark_run_id: Option<u64>,
    progress: Arc<PublishedProgress>,
    mut stop_rx: oneshot::Receiver<TransferEnd>,
) {
    let mut recorder = TelemetryRecorder::new(now, transport_profile, benchmark_run_id);
    recorder.previous_path = NetworkSnapshot::capture(&connection).counters();
    recorder.emit_config();
    recorder.emit_sample(now, &connection, false);
    let mut interval = tokio::time::interval(TELEMETRY_SAMPLE_INTERVAL);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    // `interval` ticks immediately by default. Reset it so the first sample is
    // a real 250 ms interval while the stop signal remains immediately usable.
    interval.reset();
    let mut path_watcher = connection.paths();
    let mut path_watch_connected = true;

    // n0-watcher 0.6.1 documents `Watcher::updated()` as cancel-safe, so the
    // select loop cannot consume and lose a path update when another branch wins.
    loop {
        tokio::select! {
            biased;
            outcome = &mut stop_rx => {
                let now = Instant::now();
                recorder.observe_progress(progress.snapshot(now));
                recorder.emit_sample(now, &connection, true);
                recorder.finish(
                    now,
                    &connection,
                    outcome.unwrap_or(TransferEnd::Failed),
                );
                break;
            }
            _ = interval.tick() => {
                let now = Instant::now();
                recorder.observe_progress(progress.snapshot(now));
                recorder.emit_sample(now, &connection, false);
            }
            path_update = path_watcher.updated(), if path_watch_connected => {
                match path_update {
                    Ok(_) => {
                        let now = Instant::now();
                        recorder.observe_progress(progress.snapshot(now));
                        recorder.emit_sample(now, &connection, false);
                    }
                    Err(_) => path_watch_connected = false,
                }
            }
        }
    }
}

#[derive(Debug)]
pub(super) struct BlobProviderTelemetry {
    stop_tx: Option<oneshot::Sender<TransferEnd>>,
    sampler_task: Option<JoinHandle<()>>,
}

impl BlobProviderTelemetry {
    pub(super) fn start(
        now: Instant,
        connection: ConnectionInfo,
        transport_profile: BlobTransportProfile,
        benchmark_run_id: Option<u64>,
    ) -> Self {
        let (stop_tx, stop_rx) = oneshot::channel();
        let sampler_task = tokio::spawn(run_provider_sampler(
            now,
            connection,
            transport_profile,
            benchmark_run_id,
            stop_rx,
        ));
        Self {
            stop_tx: Some(stop_tx),
            sampler_task: Some(sampler_task),
        }
    }

    pub(super) async fn finish(&mut self, outcome: TransferEnd) {
        if let Some(stop_tx) = self.stop_tx.take() {
            let _ = stop_tx.send(outcome);
        }
        if let Some(sampler_task) = self.sampler_task.take()
            && let Err(error) = sampler_task.await
        {
            debug!(?error, "blob provider telemetry sampler task failed");
        }
    }
}

async fn run_provider_sampler(
    now: Instant,
    connection: ConnectionInfo,
    transport_profile: BlobTransportProfile,
    benchmark_run_id: Option<u64>,
    mut stop_rx: oneshot::Receiver<TransferEnd>,
) {
    let mut recorder =
        ProviderTelemetryRecorder::new(now, transport_profile, benchmark_run_id, &connection);
    recorder.emit_config();
    recorder.emit_sample(now, &connection, false);
    let mut interval = tokio::time::interval(TELEMETRY_SAMPLE_INTERVAL);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    interval.reset();
    let mut path_watcher = connection.paths();
    let mut path_watch_connected = true;

    // See the cancel-safety note in the receiver sampler above. This watcher
    // has the same n0-watcher 0.6.1 contract.
    loop {
        tokio::select! {
            biased;
            outcome = &mut stop_rx => {
                let now = Instant::now();
                recorder.emit_sample(now, &connection, true);
                recorder.finish(
                    now,
                    &connection,
                    outcome.unwrap_or(TransferEnd::Failed),
                );
                break;
            }
            _ = interval.tick() => {
                recorder.emit_sample(Instant::now(), &connection, false);
            }
            path_update = path_watcher.updated(), if path_watch_connected => {
                match path_update {
                    Ok(_) => recorder.emit_sample(Instant::now(), &connection, false),
                    Err(_) => path_watch_connected = false,
                }
            }
        }
    }
}

#[derive(Debug)]
struct ProviderTelemetryRecorder {
    transfer_id: u64,
    benchmark_run_id: Option<u64>,
    transport_profile: BlobTransportProfile,
    started_at: Instant,
    last_sample_at: Instant,
    previous_path: Option<PathCounters>,
    udp_tx_bytes_total: u64,
    lost_packets_total: u64,
    lost_bytes_total: u64,
    congestion_events_total: u64,
    path_counter_discontinuity_count: u64,
}

impl ProviderTelemetryRecorder {
    fn new(
        now: Instant,
        transport_profile: BlobTransportProfile,
        benchmark_run_id: Option<u64>,
        connection: &ConnectionInfo,
    ) -> Self {
        Self {
            transfer_id: NEXT_TRANSFER_ID.fetch_add(1, Ordering::Relaxed),
            benchmark_run_id,
            transport_profile,
            started_at: now,
            last_sample_at: now,
            previous_path: NetworkSnapshot::capture(connection).counters(),
            udp_tx_bytes_total: 0,
            lost_packets_total: 0,
            lost_bytes_total: 0,
            congestion_events_total: 0,
            path_counter_discontinuity_count: 0,
        }
    }

    fn emit_config(&self) {
        debug!(
            target: TELEMETRY_TARGET,
            event = "blob_config",
            role = "provider",
            transfer_id = self.transfer_id,
            benchmark_run_id_available = self.benchmark_run_id.is_some(),
            benchmark_run_id = self.benchmark_run_id.unwrap_or(0),
            config_known = self.transport_profile.known,
            config_source = self.transport_profile.config_source,
            local_stream_receive_window_bytes = self
                .transport_profile
                .stream_receive_window_bytes,
            local_connection_receive_window_bytes = self
                .transport_profile
                .connection_receive_window_bytes,
            send_window_bytes = self.transport_profile.send_window_bytes,
            congestion_controller = self.transport_profile.congestion_controller,
            build_profile = BUILD_PROFILE,
            sample_interval_ms = duration_millis(TELEMETRY_SAMPLE_INTERVAL),
            stall_threshold_ms = duration_millis(STALL_THRESHOLD),
            "blob provider telemetry configuration"
        );
    }

    fn emit_sample(&mut self, now: Instant, connection: &ConnectionInfo, terminal_sample: bool) {
        let interval = now.saturating_duration_since(self.last_sample_at);
        let network = NetworkSnapshot::capture(connection);
        let delta = network.delta_from(self.previous_path);
        if let Some(counters) = network.counters() {
            self.previous_path = Some(counters);
        }
        self.last_sample_at = now;
        self.udp_tx_bytes_total = self.udp_tx_bytes_total.saturating_add(delta.udp_tx_bytes);
        self.lost_packets_total = self.lost_packets_total.saturating_add(delta.lost_packets);
        self.lost_bytes_total = self.lost_bytes_total.saturating_add(delta.lost_bytes);
        self.congestion_events_total = self
            .congestion_events_total
            .saturating_add(delta.congestion_events);
        self.path_counter_discontinuity_count = self
            .path_counter_discontinuity_count
            .saturating_add(u64::from(delta.path_counter_discontinuity));

        debug!(
            target: TELEMETRY_TARGET,
            event = "blob_sample",
            role = "provider",
            transfer_id = self.transfer_id,
            benchmark_run_id_available = self.benchmark_run_id.is_some(),
            benchmark_run_id = self.benchmark_run_id.unwrap_or(0),
            sample_ms = duration_millis(interval),
            elapsed_ms = duration_millis(now.saturating_duration_since(self.started_at)),
            terminal_sample,
            path = network.path_kind,
            path_stats_available = network.stats_available,
            path_counter_discontinuity = delta.path_counter_discontinuity,
            rtt_us = duration_micros(network.rtt),
            cwnd_bytes = network.cwnd,
            udp_tx_bytes_delta = delta.udp_tx_bytes,
            udp_rx_bytes_delta = delta.udp_rx_bytes,
            lost_packets_delta = delta.lost_packets,
            lost_bytes_delta = delta.lost_bytes,
            congestion_events_delta = delta.congestion_events,
            current_mtu = network.current_mtu,
            plpmtud_probe_loss_delta = delta.lost_plpmtud_probes,
            "blob provider telemetry sample"
        );
    }

    fn finish(&self, now: Instant, connection: &ConnectionInfo, outcome: TransferEnd) {
        let network = NetworkSnapshot::capture(connection);
        debug!(
            target: TELEMETRY_TARGET,
            event = "blob_summary",
            role = "provider",
            transfer_id = self.transfer_id,
            benchmark_run_id_available = self.benchmark_run_id.is_some(),
            benchmark_run_id = self.benchmark_run_id.unwrap_or(0),
            outcome = outcome.label(),
            elapsed_ms = duration_millis(now.saturating_duration_since(self.started_at)),
            udp_tx_bytes_total = self.udp_tx_bytes_total,
            lost_packets_total = self.lost_packets_total,
            lost_bytes_total = self.lost_bytes_total,
            congestion_events_total = self.congestion_events_total,
            path_counter_discontinuity_count = self.path_counter_discontinuity_count,
            path = network.path_kind,
            path_stats_available = network.stats_available,
            rtt_us = duration_micros(network.rtt),
            cwnd_bytes = network.cwnd,
            current_mtu = network.current_mtu,
            "blob provider telemetry summary"
        );
    }
}

#[derive(Debug)]
struct TelemetryRecorder {
    transfer_id: u64,
    benchmark_run_id: Option<u64>,
    transport_profile: BlobTransportProfile,
    state: TransferTelemetryState,
    previous_path: Option<PathCounters>,
    application_path: Option<&'static str>,
}

impl TelemetryRecorder {
    fn new(
        now: Instant,
        transport_profile: BlobTransportProfile,
        benchmark_run_id: Option<u64>,
    ) -> Self {
        Self {
            transfer_id: NEXT_TRANSFER_ID.fetch_add(1, Ordering::Relaxed),
            benchmark_run_id,
            transport_profile,
            state: TransferTelemetryState::new(now),
            previous_path: None,
            application_path: None,
        }
    }

    fn emit_config(&self) {
        debug!(
            target: TELEMETRY_TARGET,
            event = "blob_config",
            role = "receiver",
            transfer_id = self.transfer_id,
            benchmark_run_id_available = self.benchmark_run_id.is_some(),
            benchmark_run_id = self.benchmark_run_id.unwrap_or(0),
            config_known = self.transport_profile.known,
            config_source = self.transport_profile.config_source,
            stream_receive_window_bytes = self.transport_profile.stream_receive_window_bytes,
            connection_receive_window_bytes = self
                .transport_profile
                .connection_receive_window_bytes,
            send_window_bytes = self.transport_profile.send_window_bytes,
            congestion_controller = self.transport_profile.congestion_controller,
            build_profile = BUILD_PROFILE,
            sample_interval_ms = duration_millis(TELEMETRY_SAMPLE_INTERVAL),
            stall_threshold_ms = duration_millis(STALL_THRESHOLD),
            "blob transfer telemetry configuration"
        );
    }

    fn observe_progress(&mut self, progress: PublishedProgressSnapshot) {
        self.state
            .observe_progress(progress.first_at, progress.latest_at, progress.bytes);
    }

    fn emit_sample(&mut self, now: Instant, connection: &ConnectionInfo, terminal_sample: bool) {
        let app = self.state.sample(now);
        let network = NetworkSnapshot::capture(connection);
        let delta = network.delta_from(self.previous_path);
        if let Some(counters) = network.counters() {
            self.previous_path = Some(counters);
        }
        // When the selected path changes, the bytes accumulated since the
        // preceding sample mostly belong to the old path. Attribute this one
        // boundary delta to that old path, then switch future deltas to the new
        // path. Path-watcher-triggered samples keep the uncertainty below one
        // sampler interval.
        let application_path = match (self.application_path, network.path_kind) {
            (Some(previous), current) if current != "unknown" && previous != current => previous,
            (_, current) => current,
        };
        if network.path_kind != "unknown" {
            self.application_path = Some(network.path_kind);
        }

        debug!(
            target: TELEMETRY_TARGET,
            event = "blob_sample",
            role = "receiver",
            transfer_id = self.transfer_id,
            benchmark_run_id_available = self.benchmark_run_id.is_some(),
            benchmark_run_id = self.benchmark_run_id.unwrap_or(0),
            sample_ms = duration_millis(app.interval),
            elapsed_ms = duration_millis(app.elapsed),
            bytes_total = app.bytes_total,
            bytes_delta = app.bytes_delta,
            app_bytes_per_sec = app.bytes_per_sec,
            warmup = app.warmup,
            first_byte_seen = app.first_byte_seen,
            time_to_first_byte_ms = duration_millis(app.time_to_first_byte),
            stalled_for_ms = duration_millis(app.stalled_for),
            terminal_sample,
            path = network.path_kind,
            application_path,
            path_stats_available = network.stats_available,
            path_counter_discontinuity = delta.path_counter_discontinuity,
            rtt_us = duration_micros(network.rtt),
            // These congestion counters describe packets sent by this local
            // endpoint. On a blob receiver that is mostly ACK/control traffic,
            // so keep the `local_` prefix to avoid presenting it as the
            // provider's payload congestion state.
            local_cwnd_bytes = network.cwnd,
            udp_rx_bytes_delta = delta.udp_rx_bytes,
            udp_tx_bytes_delta = delta.udp_tx_bytes,
            local_lost_packets_delta = delta.lost_packets,
            local_lost_bytes_delta = delta.lost_bytes,
            local_congestion_events_delta = delta.congestion_events,
            current_mtu = network.current_mtu,
            local_plpmtud_probe_loss_delta = delta.lost_plpmtud_probes,
            "blob transfer telemetry sample"
        );
    }

    fn finish(&mut self, now: Instant, connection: &ConnectionInfo, outcome: TransferEnd) {
        let summary = self.state.finish(now, outcome);
        let network = NetworkSnapshot::capture(connection);
        debug!(
            target: TELEMETRY_TARGET,
            event = "blob_summary",
            role = "receiver",
            transfer_id = self.transfer_id,
            benchmark_run_id_available = self.benchmark_run_id.is_some(),
            benchmark_run_id = self.benchmark_run_id.unwrap_or(0),
            outcome = outcome.label(),
            elapsed_ms = duration_millis(summary.elapsed),
            bytes_total = summary.bytes_total,
            average_bytes_per_sec = summary.average_bytes_per_sec,
            first_byte_seen = summary.first_byte_seen,
            time_to_first_byte_ms = duration_millis(summary.time_to_first_byte),
            warmup_ms = duration_millis(summary.warmup),
            finalization_pause_ms = duration_millis(summary.finalization_pause),
            stall_count = summary.stall_count,
            stall_total_ms = duration_millis(summary.stall_total),
            longest_stall_ms = duration_millis(summary.longest_stall),
            path = network.path_kind,
            path_stats_available = network.stats_available,
            rtt_us = duration_micros(network.rtt),
            local_cwnd_bytes = network.cwnd,
            current_mtu = network.current_mtu,
            "blob transfer telemetry summary"
        );
    }
}

#[derive(Debug)]
struct TransferTelemetryState {
    started_at: Instant,
    last_sample_at: Instant,
    last_sample_bytes: u64,
    latest_bytes: u64,
    first_progress_at: Option<Instant>,
    last_progress_at: Option<Instant>,
    active_stall: bool,
    stall_count: u64,
    stall_total: Duration,
    longest_stall: Duration,
}

impl TransferTelemetryState {
    fn new(now: Instant) -> Self {
        Self {
            started_at: now,
            last_sample_at: now,
            last_sample_bytes: 0,
            latest_bytes: 0,
            first_progress_at: None,
            last_progress_at: None,
            active_stall: false,
            stall_count: 0,
            stall_total: Duration::ZERO,
            longest_stall: Duration::ZERO,
        }
    }

    fn observe_progress(
        &mut self,
        first_progress_at: Instant,
        latest_progress_at: Instant,
        bytes_received: u64,
    ) {
        if bytes_received <= self.latest_bytes {
            return;
        }
        self.close_active_stall(latest_progress_at);
        self.latest_bytes = bytes_received;
        self.first_progress_at.get_or_insert(first_progress_at);
        self.last_progress_at = Some(latest_progress_at);
    }

    fn sample(&mut self, now: Instant) -> ApplicationSample {
        self.detect_stall(now);
        let interval = now.saturating_duration_since(self.last_sample_at);
        let bytes_delta = self.latest_bytes.saturating_sub(self.last_sample_bytes);
        // The first sample carrying bytes includes connection/fetch warm-up.
        // Keep it in the raw log, but mark it so stability analysis can start
        // from the following complete interval.
        let warmup = self.last_sample_bytes == 0;
        let time_to_first_byte = self
            .first_progress_at
            .map(|first| first.saturating_duration_since(self.started_at))
            .unwrap_or(Duration::ZERO);
        let sample = ApplicationSample {
            interval,
            elapsed: now.saturating_duration_since(self.started_at),
            bytes_total: self.latest_bytes,
            bytes_delta,
            bytes_per_sec: bytes_per_second(bytes_delta, interval),
            warmup,
            first_byte_seen: self.first_progress_at.is_some(),
            time_to_first_byte,
            stalled_for: if self.active_stall {
                self.last_progress_at
                    .map(|last| now.saturating_duration_since(last))
                    .unwrap_or(Duration::ZERO)
            } else {
                Duration::ZERO
            },
        };
        self.last_sample_at = now;
        self.last_sample_bytes = self.latest_bytes;
        sample
    }

    fn finish(&mut self, now: Instant, outcome: TransferEnd) -> TransferSummary {
        let elapsed = now.saturating_duration_since(self.started_at);
        let time_to_first_byte = self
            .first_progress_at
            .map(|first| first.saturating_duration_since(self.started_at))
            .unwrap_or(Duration::ZERO);
        let terminal_pause = self
            .last_progress_at
            .map(|last| now.saturating_duration_since(last))
            .unwrap_or(Duration::ZERO);

        let finalization_pause = match outcome {
            TransferEnd::Complete => {
                // A stall still active at successful completion is the
                // finalization tail, not a proven mid-transfer stall.
                if self.active_stall {
                    self.active_stall = false;
                    self.stall_count = self.stall_count.saturating_sub(1);
                }
                terminal_pause
            }
            TransferEnd::Failed => {
                // On failure, an active stall can be the failure itself. Keep
                // it in the stall metrics instead of relabeling it finalization.
                self.close_active_stall(now);
                Duration::ZERO
            }
        };
        TransferSummary {
            elapsed,
            bytes_total: self.latest_bytes,
            average_bytes_per_sec: bytes_per_second(self.latest_bytes, elapsed),
            first_byte_seen: self.first_progress_at.is_some(),
            time_to_first_byte,
            warmup: if self.first_progress_at.is_some() {
                time_to_first_byte
            } else {
                elapsed
            },
            finalization_pause,
            stall_count: self.stall_count,
            stall_total: self.stall_total,
            longest_stall: self.longest_stall,
        }
    }

    fn detect_stall(&mut self, now: Instant) {
        let Some(last_progress_at) = self.last_progress_at else {
            return;
        };
        if !self.active_stall && now.saturating_duration_since(last_progress_at) >= STALL_THRESHOLD
        {
            self.active_stall = true;
            self.stall_count = self.stall_count.saturating_add(1);
        }
    }

    fn close_active_stall(&mut self, now: Instant) {
        if !self.active_stall {
            return;
        }
        let Some(last_progress_at) = self.last_progress_at else {
            self.active_stall = false;
            return;
        };
        let duration = now.saturating_duration_since(last_progress_at);
        self.stall_total = self.stall_total.saturating_add(duration);
        self.longest_stall = self.longest_stall.max(duration);
        self.active_stall = false;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ApplicationSample {
    interval: Duration,
    elapsed: Duration,
    bytes_total: u64,
    bytes_delta: u64,
    bytes_per_sec: u64,
    warmup: bool,
    first_byte_seen: bool,
    time_to_first_byte: Duration,
    stalled_for: Duration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TransferSummary {
    elapsed: Duration,
    bytes_total: u64,
    average_bytes_per_sec: u64,
    first_byte_seen: bool,
    time_to_first_byte: Duration,
    warmup: Duration,
    finalization_pause: Duration,
    stall_count: u64,
    stall_total: Duration,
    longest_stall: Duration,
}

#[derive(Debug, Clone, Copy)]
struct NetworkSnapshot {
    path_kind: &'static str,
    stats_available: bool,
    path_id: Option<PathId>,
    rtt: Duration,
    cwnd: u64,
    udp_rx_bytes: u64,
    udp_tx_bytes: u64,
    lost_packets: u64,
    lost_bytes: u64,
    congestion_events: u64,
    current_mtu: u16,
    lost_plpmtud_probes: u64,
}

impl NetworkSnapshot {
    fn capture(connection: &ConnectionInfo) -> Self {
        let Some(path) = connection.selected_path() else {
            return Self::unavailable("unknown");
        };
        let path_kind = classify_path(path.remote_addr());
        let Some(stats) = path.stats() else {
            return Self::unavailable(path_kind);
        };
        Self {
            path_kind,
            stats_available: true,
            path_id: Some(path.id()),
            rtt: stats.rtt,
            cwnd: stats.cwnd,
            udp_rx_bytes: stats.udp_rx.bytes,
            udp_tx_bytes: stats.udp_tx.bytes,
            lost_packets: stats.lost_packets,
            lost_bytes: stats.lost_bytes,
            congestion_events: stats.congestion_events,
            current_mtu: stats.current_mtu,
            lost_plpmtud_probes: stats.lost_plpmtud_probes,
        }
    }

    fn unavailable(path_kind: &'static str) -> Self {
        Self {
            path_kind,
            stats_available: false,
            path_id: None,
            rtt: Duration::ZERO,
            cwnd: 0,
            udp_rx_bytes: 0,
            udp_tx_bytes: 0,
            lost_packets: 0,
            lost_bytes: 0,
            congestion_events: 0,
            current_mtu: 0,
            lost_plpmtud_probes: 0,
        }
    }

    fn counters(self) -> Option<PathCounters> {
        Some(PathCounters {
            path_id: self.path_id?,
            udp_rx_bytes: self.udp_rx_bytes,
            udp_tx_bytes: self.udp_tx_bytes,
            lost_packets: self.lost_packets,
            lost_bytes: self.lost_bytes,
            congestion_events: self.congestion_events,
            lost_plpmtud_probes: self.lost_plpmtud_probes,
        })
    }

    fn delta_from(self, previous: Option<PathCounters>) -> PathCountersDelta {
        let Some(current) = self.counters() else {
            return PathCountersDelta::default();
        };
        let Some(previous) = previous else {
            return PathCountersDelta::default();
        };
        if previous.path_id != current.path_id {
            return PathCountersDelta {
                path_counter_discontinuity: true,
                ..PathCountersDelta::default()
            };
        }
        PathCountersDelta {
            path_counter_discontinuity: false,
            udp_rx_bytes: current.udp_rx_bytes.saturating_sub(previous.udp_rx_bytes),
            udp_tx_bytes: current.udp_tx_bytes.saturating_sub(previous.udp_tx_bytes),
            lost_packets: current.lost_packets.saturating_sub(previous.lost_packets),
            lost_bytes: current.lost_bytes.saturating_sub(previous.lost_bytes),
            congestion_events: current
                .congestion_events
                .saturating_sub(previous.congestion_events),
            lost_plpmtud_probes: current
                .lost_plpmtud_probes
                .saturating_sub(previous.lost_plpmtud_probes),
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct PathCounters {
    path_id: PathId,
    udp_rx_bytes: u64,
    udp_tx_bytes: u64,
    lost_packets: u64,
    lost_bytes: u64,
    congestion_events: u64,
    lost_plpmtud_probes: u64,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct PathCountersDelta {
    path_counter_discontinuity: bool,
    udp_rx_bytes: u64,
    udp_tx_bytes: u64,
    lost_packets: u64,
    lost_bytes: u64,
    congestion_events: u64,
    lost_plpmtud_probes: u64,
}

fn classify_path(addr: &TransportAddr) -> &'static str {
    match addr {
        TransportAddr::Ip(SocketAddr::V4(addr)) if in_usb_tunnel_subnet(*addr.ip()) => "aoa",
        TransportAddr::Ip(_) => "direct",
        TransportAddr::Relay(_) => "relay",
        TransportAddr::Custom(_) => "custom",
        _ => "unknown",
    }
}

fn bytes_per_second(bytes: u64, elapsed: Duration) -> u64 {
    let nanos = elapsed.as_nanos();
    if bytes == 0 || nanos == 0 {
        return 0;
    }
    ((u128::from(bytes) * 1_000_000_000) / nanos).min(u128::from(u64::MAX)) as u64
}

fn duration_nanos(duration: Duration) -> u64 {
    duration.as_nanos().min(u128::from(u64::MAX)) as u64
}

fn duration_millis(duration: Duration) -> u64 {
    duration.as_millis().min(u128::from(u64::MAX)) as u64
}

fn duration_micros(duration: Duration) -> u64 {
    duration.as_micros().min(u128::from(u64::MAX)) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn application_samples_use_fixed_interval_deltas() {
        let start = Instant::now();
        let mut state = TransferTelemetryState::new(start);
        let first_progress = start + Duration::from_millis(100);
        state.observe_progress(first_progress, first_progress, 100);

        let first = state.sample(start + Duration::from_millis(250));
        assert_eq!(first.bytes_delta, 100);
        assert_eq!(first.bytes_per_sec, 400);
        assert_eq!(first.stalled_for, Duration::ZERO);

        let second_progress = start + Duration::from_millis(300);
        state.observe_progress(first_progress, second_progress, 250);
        let second = state.sample(start + Duration::from_millis(500));
        assert_eq!(second.bytes_delta, 150);
        assert_eq!(second.bytes_per_sec, 600);
    }

    #[test]
    fn blob_telemetry_assigns_distinct_process_local_transfer_ids() {
        let now = Instant::now();
        let first = TelemetryRecorder::new(now, BlobTransportProfile::default(), None);
        let second = TelemetryRecorder::new(now, BlobTransportProfile::default(), None);

        assert_ne!(first.transfer_id, second.transfer_id);
    }

    #[test]
    fn benchmark_run_id_derives_a_stable_non_reversible_token() {
        let session_id = "0123456789abcdef";
        let token = benchmark_run_id(session_id).expect("valid sender session ID");
        assert_eq!(benchmark_run_id(session_id), Some(token));
        assert_ne!(token, 0x0123_4567_89ab_cdef);
        assert_ne!(
            benchmark_run_id("fedcba9876543210"),
            benchmark_run_id(session_id)
        );
        assert_eq!(benchmark_run_id("session-1"), None);
        assert_eq!(benchmark_run_id("0123456789abcde"), None);
        assert_eq!(benchmark_run_id("0123456789abcdeg"), None);
        assert_eq!(benchmark_run_id("0123456789ABCDEf"), None);
    }

    #[test]
    fn warmup_before_first_byte_is_not_counted_as_a_stall() {
        let start = Instant::now();
        let mut state = TransferTelemetryState::new(start);

        let sample = state.sample(start + Duration::from_secs(2));
        assert!(sample.warmup);
        assert!(!sample.first_byte_seen);
        assert_eq!(sample.stalled_for, Duration::ZERO);
        assert_eq!(state.stall_count, 0);

        let summary = state.finish(start + Duration::from_secs(3), TransferEnd::Complete);
        assert!(!summary.first_byte_seen);
        assert_eq!(summary.warmup, Duration::from_secs(3));
        assert_eq!(summary.stall_count, 0);
    }

    #[test]
    fn first_byte_timing_is_reported_separately_from_stalls() {
        let start = Instant::now();
        let mut state = TransferTelemetryState::new(start);
        let first_progress = start + Duration::from_millis(750);
        state.observe_progress(first_progress, first_progress, 100);

        let first = state.sample(start + Duration::from_secs(1));
        assert!(first.warmup);
        assert!(first.first_byte_seen);
        assert_eq!(first.time_to_first_byte, Duration::from_millis(750));
        assert_eq!(state.stall_count, 0);
    }

    #[test]
    fn published_progress_preserves_hot_loop_timestamps_between_samples() {
        let start = Instant::now();
        let progress = PublishedProgress::new(start);
        progress.publish(start + Duration::from_millis(25), 100);
        progress.publish(start + Duration::from_millis(225), 250);

        let snapshot = progress.snapshot(start + Duration::from_millis(250));
        assert_eq!(snapshot.bytes, 250);
        assert_eq!(snapshot.first_at, start + Duration::from_millis(25));
        assert_eq!(snapshot.latest_at, start + Duration::from_millis(225));
    }

    #[test]
    fn terminal_pause_is_excluded_from_mid_transfer_stalls() {
        let start = Instant::now();
        let mut state = TransferTelemetryState::new(start);
        let first_progress = start + Duration::from_millis(100);
        state.observe_progress(first_progress, first_progress, 100);

        let sample = state.sample(start + Duration::from_millis(600));
        assert_eq!(sample.stalled_for, Duration::from_millis(500));
        assert_eq!(state.stall_count, 1);

        let summary = state.finish(start + Duration::from_millis(900), TransferEnd::Complete);
        assert_eq!(summary.finalization_pause, Duration::from_millis(800));
        assert_eq!(summary.stall_count, 0);
        assert_eq!(summary.stall_total, Duration::ZERO);
    }

    #[test]
    fn failed_terminal_preserves_the_active_failure_stall() {
        let start = Instant::now();
        let mut state = TransferTelemetryState::new(start);
        let first_progress = start + Duration::from_millis(100);
        state.observe_progress(first_progress, first_progress, 100);

        let sample = state.sample(start + Duration::from_millis(600));
        assert_eq!(sample.stalled_for, Duration::from_millis(500));

        let summary = state.finish(start + Duration::from_millis(900), TransferEnd::Failed);
        assert_eq!(summary.finalization_pause, Duration::ZERO);
        assert_eq!(summary.stall_count, 1);
        assert_eq!(summary.stall_total, Duration::from_millis(800));
        assert_eq!(summary.longest_stall, Duration::from_millis(800));
    }

    #[test]
    fn stalls_are_counted_once_and_closed_when_progress_resumes() {
        let start = Instant::now();
        let mut state = TransferTelemetryState::new(start);
        let first_progress = start + Duration::from_millis(100);
        state.observe_progress(first_progress, first_progress, 100);

        let sample = state.sample(start + Duration::from_millis(600));
        assert_eq!(sample.stalled_for, Duration::from_millis(500));
        assert_eq!(state.stall_count, 1);

        let sample = state.sample(start + Duration::from_millis(850));
        assert_eq!(sample.stalled_for, Duration::from_millis(750));
        assert_eq!(state.stall_count, 1);

        let resumed_progress = start + Duration::from_millis(900);
        state.observe_progress(first_progress, resumed_progress, 200);
        let summary = state.finish(start + Duration::from_millis(1_000), TransferEnd::Complete);
        assert_eq!(summary.stall_count, 1);
        assert_eq!(summary.stall_total, Duration::from_millis(800));
        assert_eq!(summary.longest_stall, Duration::from_millis(800));
    }

    #[test]
    fn path_counter_delta_flags_path_migration() {
        let base = NetworkSnapshot {
            path_kind: "direct",
            stats_available: true,
            path_id: Some(PathId::ZERO),
            rtt: Duration::from_millis(2),
            cwnd: 1_000,
            udp_rx_bytes: 10_000,
            udp_tx_bytes: 1_000,
            lost_packets: 2,
            lost_bytes: 2_400,
            congestion_events: 1,
            current_mtu: 1_200,
            lost_plpmtud_probes: 1,
        };
        let next = NetworkSnapshot {
            udp_rx_bytes: 12_500,
            udp_tx_bytes: 1_400,
            lost_packets: 3,
            lost_bytes: 3_600,
            congestion_events: 2,
            lost_plpmtud_probes: 2,
            ..base
        };
        assert_eq!(
            next.delta_from(base.counters()),
            PathCountersDelta {
                path_counter_discontinuity: false,
                udp_rx_bytes: 2_500,
                udp_tx_bytes: 400,
                lost_packets: 1,
                lost_bytes: 1_200,
                congestion_events: 1,
                lost_plpmtud_probes: 1,
            }
        );

        let migrated = NetworkSnapshot {
            path_id: Some(PathId::ZERO.saturating_add(1_u8)),
            ..next
        };
        assert_eq!(
            migrated.delta_from(next.counters()),
            PathCountersDelta {
                path_counter_discontinuity: true,
                ..PathCountersDelta::default()
            }
        );
    }
}
