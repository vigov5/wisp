use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use std::time::{Duration, Instant};

use futures_lite::StreamExt;
use iroh::{
    TransportAddr,
    endpoint::{Connection, PathId},
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
pub fn benchmark_run_id(session_id: &str) -> Option<u64> {
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
    /// Every progress item the blob layer produced, counted before any
    /// coalescing. The rate this implies is what P0.1's 10 Hz cap avoids
    /// forwarding, so B2 cannot attribute the coalescer without it.
    events: AtomicU64,
}

#[derive(Debug, Clone, Copy)]
struct PublishedProgressSnapshot {
    bytes: u64,
    first_at: Instant,
    latest_at: Instant,
    events: u64,
}

impl PublishedProgress {
    fn new(started_at: Instant) -> Self {
        Self {
            started_at,
            bytes: AtomicU64::new(0),
            first_elapsed_nanos: AtomicU64::new(UNSET_PROGRESS_TIME_NANOS),
            latest_elapsed_nanos: AtomicU64::new(UNSET_PROGRESS_TIME_NANOS),
            events: AtomicU64::new(0),
        }
    }

    /// The blob download loop is the single writer. Publish timestamps before
    /// the byte position so the sampler's acquire load observes a coherent
    /// progress event without adding a lock or await to the hot path.
    fn publish(&self, now: Instant, bytes_received: u64) {
        // Counted before the monotonicity check: a repeated or stale offset is
        // still an item the download loop had to handle.
        self.events.fetch_add(1, Ordering::Relaxed);
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
            events: self.events.load(Ordering::Relaxed),
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
        connection: Connection,
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
    connection: Connection,
    transport_profile: BlobTransportProfile,
    benchmark_run_id: Option<u64>,
    progress: Arc<PublishedProgress>,
    mut stop_rx: oneshot::Receiver<TransferEnd>,
) {
    let mut recorder = TelemetryRecorder::new(now, transport_profile, benchmark_run_id);
    recorder.previous_path = NetworkSnapshot::capture(&connection).counters();
    recorder.previous_connection = ConnectionCounters::capture(&connection);
    recorder.emit_config();
    recorder.emit_sample(now, &connection, false);
    let mut interval = tokio::time::interval(TELEMETRY_SAMPLE_INTERVAL);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    // `interval` ticks immediately by default. Reset it so the first sample is
    // a real 250 ms interval while the stop signal remains immediately usable.
    interval.reset();
    let mut path_events = connection.path_events();
    let mut path_watch_connected = true;

    // `PathEventStream` wraps a tokio `BroadcastStream`, whose `next()` is
    // cancel-safe, so the select loop cannot consume and lose a path event when
    // another branch wins. A consumer that falls behind gets a single
    // `PathEvent::Lagged` instead of a closed stream, so lagging costs one
    // sample rather than the whole feed.
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
            path_event = path_events.next(), if path_watch_connected => {
                match path_event {
                    Some(_) => {
                        let now = Instant::now();
                        recorder.observe_progress(progress.snapshot(now));
                        recorder.emit_sample(now, &connection, false);
                    }
                    None => path_watch_connected = false,
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
        connection: Connection,
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
    connection: Connection,
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
    let mut path_events = connection.path_events();
    let mut path_watch_connected = true;

    // See the cancel-safety note in the receiver sampler above. This stream has
    // the same `BroadcastStream` contract.
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
            path_event = path_events.next(), if path_watch_connected => {
                match path_event {
                    Some(_) => recorder.emit_sample(Instant::now(), &connection, false),
                    None => path_watch_connected = false,
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
    path_aggregate: PathAggregate,
    all_paths_udp_tx_bytes_total: u64,
    all_paths_lost_packets_total: u64,
    previous_connection: ConnectionCounters,
    udp_tx_bytes_total: u64,
    lost_packets_total: u64,
    lost_bytes_total: u64,
    congestion_events_total: u64,
    path_counter_discontinuity_count: u64,
    connection_totals: ConnectionTotals,
}

impl ProviderTelemetryRecorder {
    fn new(
        now: Instant,
        transport_profile: BlobTransportProfile,
        benchmark_run_id: Option<u64>,
        connection: &Connection,
    ) -> Self {
        Self {
            transfer_id: NEXT_TRANSFER_ID.fetch_add(1, Ordering::Relaxed),
            benchmark_run_id,
            transport_profile,
            started_at: now,
            last_sample_at: now,
            previous_path: NetworkSnapshot::capture(connection).counters(),
            path_aggregate: PathAggregate::default(),
            all_paths_udp_tx_bytes_total: 0,
            all_paths_lost_packets_total: 0,
            previous_connection: ConnectionCounters::capture(connection),
            udp_tx_bytes_total: 0,
            lost_packets_total: 0,
            lost_bytes_total: 0,
            congestion_events_total: 0,
            path_counter_discontinuity_count: 0,
            connection_totals: ConnectionTotals::default(),
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

    fn emit_sample(&mut self, now: Instant, connection: &Connection, terminal_sample: bool) {
        let interval = now.saturating_duration_since(self.last_sample_at);
        let network = NetworkSnapshot::capture(connection);
        let delta = network.delta_from(self.previous_path);
        let connection_counters = ConnectionCounters::capture(connection);
        let connection_delta = connection_counters.delta_from(self.previous_connection);
        let all_paths = self
            .path_aggregate
            .observe(&PathObservation::capture_all(connection));
        if let Some(counters) = network.counters() {
            self.previous_path = Some(counters);
        }
        if connection_counters.available {
            self.previous_connection = connection_counters;
        }
        self.connection_totals.accumulate(connection_delta);
        self.all_paths_udp_tx_bytes_total = self
            .all_paths_udp_tx_bytes_total
            .saturating_add(all_paths.udp_tx_bytes);
        self.all_paths_lost_packets_total = self
            .all_paths_lost_packets_total
            .saturating_add(all_paths.lost_packets);
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
            path_count = network.path_count,
            path_counter_discontinuity = delta.path_counter_discontinuity,
            rtt_us = duration_micros(network.rtt),
            cwnd_bytes = network.cwnd,
            udp_tx_bytes_delta = delta.udp_tx_bytes,
            udp_rx_bytes_delta = delta.udp_rx_bytes,
            lost_packets_delta = delta.lost_packets,
            lost_bytes_delta = delta.lost_bytes,
            congestion_events_delta = delta.congestion_events,
            spurious_congestion_events_delta = delta.spurious_congestion_events,
            current_mtu = network.current_mtu,
            plpmtud_probe_loss_delta = delta.lost_plpmtud_probes,
            // Connection-scoped counters. Unlike the path-scoped ones above,
            // these survive path selection gaps and migrations, so they are the
            // byte figures a report should trust.
            connection_stats_available = connection_delta.available,
            connection_udp_tx_bytes_delta = connection_delta.udp_tx_bytes,
            connection_udp_rx_bytes_delta = connection_delta.udp_rx_bytes,
            stream_data_blocked_tx_delta = connection_delta.stream_data_blocked_tx,
            stream_data_blocked_rx_delta = connection_delta.stream_data_blocked_rx,
            data_blocked_tx_delta = connection_delta.data_blocked_tx,
            // Summed over every path, so loss is a real count rather than the
            // share that happened to land on the selected path.
            active_path_count = all_paths.active_path_count,
            all_paths_udp_tx_bytes_delta = all_paths.udp_tx_bytes,
            all_paths_udp_rx_bytes_delta = all_paths.udp_rx_bytes,
            all_paths_lost_packets_delta = all_paths.lost_packets,
            all_paths_lost_bytes_delta = all_paths.lost_bytes,
            all_paths_congestion_events_delta = all_paths.congestion_events,
            all_paths_spurious_congestion_events_delta = all_paths.spurious_congestion_events,
            direct_path_udp_bytes_delta = all_paths.direct_udp_bytes,
            relay_path_udp_bytes_delta = all_paths.relay_udp_bytes,
            aoa_path_udp_bytes_delta = all_paths.aoa_udp_bytes,
            "blob provider telemetry sample"
        );
    }

    fn finish(&self, now: Instant, connection: &Connection, outcome: TransferEnd) {
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
            // Path-scoped totals. Kept under the historical name so older logs
            // stay comparable, but they are a lower bound: read them against
            // `connection_udp_tx_bytes_total` to see the share of the payload
            // the selected path actually observed.
            udp_tx_bytes_total = self.udp_tx_bytes_total,
            lost_packets_total = self.lost_packets_total,
            lost_bytes_total = self.lost_bytes_total,
            congestion_events_total = self.congestion_events_total,
            path_counter_discontinuity_count = self.path_counter_discontinuity_count,
            connection_udp_tx_bytes_total = self.connection_totals.udp_tx_bytes,
            connection_udp_rx_bytes_total = self.connection_totals.udp_rx_bytes,
            connection_samples_without_stats = self.connection_totals.samples_without_stats,
            stream_data_blocked_tx_total = self.connection_totals.stream_data_blocked_tx,
            stream_data_blocked_rx_total = self.connection_totals.stream_data_blocked_rx,
            data_blocked_tx_total = self.connection_totals.data_blocked_tx,
            all_paths_udp_tx_bytes_total = self.all_paths_udp_tx_bytes_total,
            all_paths_lost_packets_total = self.all_paths_lost_packets_total,
            path = network.path_kind,
            path_stats_available = network.stats_available,
            path_count = network.path_count,
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
    path_aggregate: PathAggregate,
    all_paths_udp_rx_bytes_total: u64,
    all_paths_lost_packets_total: u64,
    previous_connection: ConnectionCounters,
    connection_totals: ConnectionTotals,
    path_udp_rx_bytes_total: u64,
    application_path: Option<&'static str>,
    /// Raw progress items seen at the previous sample, so each window can
    /// report how many the blob layer produced.
    previous_progress_events: u64,
    progress_events_total: u64,
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
            path_aggregate: PathAggregate::default(),
            all_paths_udp_rx_bytes_total: 0,
            all_paths_lost_packets_total: 0,
            previous_connection: ConnectionCounters::default(),
            connection_totals: ConnectionTotals::default(),
            path_udp_rx_bytes_total: 0,
            application_path: None,
            previous_progress_events: 0,
            progress_events_total: 0,
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
        self.progress_events_total = progress.events;
    }

    /// Raw progress items since the previous sample, and remember the new base.
    fn take_progress_events_delta(&mut self) -> u64 {
        let delta = self
            .progress_events_total
            .saturating_sub(self.previous_progress_events);
        self.previous_progress_events = self.progress_events_total;
        delta
    }

    fn emit_sample(&mut self, now: Instant, connection: &Connection, terminal_sample: bool) {
        let app = self.state.sample(now);
        let network = NetworkSnapshot::capture(connection);
        let delta = network.delta_from(self.previous_path);
        let connection_counters = ConnectionCounters::capture(connection);
        let connection_delta = connection_counters.delta_from(self.previous_connection);
        let all_paths = self
            .path_aggregate
            .observe(&PathObservation::capture_all(connection));
        let progress_events = self.take_progress_events_delta();
        if let Some(counters) = network.counters() {
            self.previous_path = Some(counters);
        }
        if connection_counters.available {
            self.previous_connection = connection_counters;
        }
        self.connection_totals.accumulate(connection_delta);
        self.all_paths_udp_rx_bytes_total = self
            .all_paths_udp_rx_bytes_total
            .saturating_add(all_paths.udp_rx_bytes);
        self.all_paths_lost_packets_total = self
            .all_paths_lost_packets_total
            .saturating_add(all_paths.lost_packets);
        self.path_udp_rx_bytes_total = self
            .path_udp_rx_bytes_total
            .saturating_add(delta.udp_rx_bytes);
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
            path_count = network.path_count,
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
            local_spurious_congestion_events_delta = delta.spurious_congestion_events,
            current_mtu = network.current_mtu,
            local_plpmtud_probe_loss_delta = delta.lost_plpmtud_probes,
            // Connection-scoped counters, unaffected by path selection gaps and
            // migrations. `stream_data_blocked_rx` is the sender telling us it
            // had data ready and our advertised stream window stopped it — the
            // one unambiguous receive-window-bound signal available here.
            connection_stats_available = connection_delta.available,
            connection_udp_rx_bytes_delta = connection_delta.udp_rx_bytes,
            connection_udp_tx_bytes_delta = connection_delta.udp_tx_bytes,
            stream_data_blocked_rx_delta = connection_delta.stream_data_blocked_rx,
            stream_data_blocked_tx_delta = connection_delta.stream_data_blocked_tx,
            // Summed over every path. `active_path_count` above one means the
            // payload really is arriving over several paths at once, which is
            // the reason the selected-path figures fall short.
            active_path_count = all_paths.active_path_count,
            all_paths_udp_rx_bytes_delta = all_paths.udp_rx_bytes,
            all_paths_udp_tx_bytes_delta = all_paths.udp_tx_bytes,
            all_paths_lost_packets_delta = all_paths.lost_packets,
            all_paths_lost_bytes_delta = all_paths.lost_bytes,
            all_paths_congestion_events_delta = all_paths.congestion_events,
            all_paths_spurious_congestion_events_delta = all_paths.spurious_congestion_events,
            direct_path_udp_bytes_delta = all_paths.direct_udp_bytes,
            relay_path_udp_bytes_delta = all_paths.relay_udp_bytes,
            aoa_path_udp_bytes_delta = all_paths.aoa_udp_bytes,
            progress_events_delta = progress_events,
            "blob transfer telemetry sample"
        );
    }

    fn finish(&mut self, now: Instant, connection: &Connection, outcome: TransferEnd) {
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
            // Path-scoped receive total is a lower bound; compare it against the
            // connection-scoped total to see how much of the payload the
            // selected path accounted for.
            path_udp_rx_bytes_total = self.path_udp_rx_bytes_total,
            connection_udp_rx_bytes_total = self.connection_totals.udp_rx_bytes,
            connection_udp_tx_bytes_total = self.connection_totals.udp_tx_bytes,
            connection_samples_without_stats = self.connection_totals.samples_without_stats,
            stream_data_blocked_rx_total = self.connection_totals.stream_data_blocked_rx,
            stream_data_blocked_tx_total = self.connection_totals.stream_data_blocked_tx,
            all_paths_udp_rx_bytes_total = self.all_paths_udp_rx_bytes_total,
            all_paths_lost_packets_total = self.all_paths_lost_packets_total,
            progress_events_total = self.progress_events_total,
            path = network.path_kind,
            path_stats_available = network.stats_available,
            path_count = network.path_count,
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
    /// Number of paths the connection currently holds. Anything above one means
    /// the selected path's counters describe a subset of the connection's
    /// traffic, which is the ordinary reason `path_counter_coverage` sits below
    /// 1.0 even with a stable selected path and no migration.
    path_count: u64,
    path_id: Option<PathId>,
    rtt: Duration,
    cwnd: u64,
    udp_rx_bytes: u64,
    udp_tx_bytes: u64,
    lost_packets: u64,
    lost_bytes: u64,
    congestion_events: u64,
    /// Losses later withdrawn because an ACK covered the packet.
    ///
    /// Separates "the path really dropped this" from "we gave up on it too
    /// early and backed off for nothing", which is the difference between a
    /// lossy link and a loss-detection artefact.
    spurious_congestion_events: u64,
    current_mtu: u16,
    lost_plpmtud_probes: u64,
}

/// Absolute counters for one path, read at a single instant.
#[derive(Debug, Clone, Copy)]
struct PathObservation {
    path_id: PathId,
    path_kind: &'static str,
    udp_tx_bytes: u64,
    udp_rx_bytes: u64,
    lost_packets: u64,
    lost_bytes: u64,
    congestion_events: u64,
    spurious_congestion_events: u64,
}

impl PathObservation {
    /// One entry per **path**, not per advertised address.
    ///
    /// Kept as a defensive fold. Under iroh 0.97 a single QUIC path appeared in
    /// the list once per transport address it was reachable at, every entry
    /// carrying the same underlying counters, so summing the list as-is counted
    /// that path's traffic once per address. iroh 1.0 keys its path list by
    /// `PathId` and replaces rather than appends, so duplicates should no longer
    /// reach us — but `PathList` does not guarantee uniqueness in its type, and
    /// the failure is silent inflation rather than a panic. The 0.97 behaviour
    /// measured 1.70x the connection total on a Wi-Fi
    /// transfer. Deduplicating by `PathId` keeps each path counted once.
    fn capture_all(connection: &Connection) -> Vec<Self> {
        let mut by_path: HashMap<PathId, Self> = HashMap::new();
        for path in connection.paths().iter() {
            let stats = path.stats();
            let kind = classify_path(path.remote_addr());
            by_path
                .entry(path.id())
                .and_modify(|existing| {
                    // Attribute a path reachable both directly and over the
                    // relay to the direct address: that is the route its
                    // traffic takes while the direct path is usable, and
                    // over-reporting relay bytes would misdirect the
                    // relay-vs-direct decision in D1.
                    if existing.path_kind == "relay" && kind != "relay" {
                        existing.path_kind = kind;
                    }
                })
                .or_insert(Self {
                    path_id: path.id(),
                    path_kind: kind,
                    udp_tx_bytes: stats.udp_tx.bytes,
                    udp_rx_bytes: stats.udp_rx.bytes,
                    lost_packets: stats.lost_packets,
                    lost_bytes: stats.lost_bytes,
                    congestion_events: stats.congestion_events,
                    spurious_congestion_events: stats.spurious_congestion_events,
                });
        }
        by_path.into_values().collect()
    }
}

/// Per-path counters summed over **every** path of the connection.
///
/// The selected path alone accounts for a fraction of a multipath connection —
/// measured at 0-21% on real transfers — so selected-path loss and byte figures
/// are lower bounds. Summing every path restores figures that can be compared
/// against the connection-scoped totals, and makes loss counts usable rather
/// than merely indicative.
///
/// Counters are tracked per `PathId` because a path that appears mid-transfer
/// starts its own series at zero: summing raw totals across a changing path set
/// would report a new path's whole history as one interval's traffic, and would
/// go backwards when a path leaves the list.
///
/// Entries are kept for paths that drop out of the current list. A path can
/// leave and return — measured on a Wi-Fi transfer that cycled through nine
/// paths — and forgetting its counters would credit its whole history to the
/// interval it reappeared in. That inflated the aggregate 7.4% above the
/// connection total before this was fixed. `PathId`s are never reused within a
/// connection, so the map is bounded by the paths that connection ever opened.
#[derive(Debug, Default)]
struct PathAggregate {
    previous: HashMap<PathId, PathObservation>,
}

impl PathAggregate {
    fn observe(&mut self, paths: &[PathObservation]) -> PathAggregateDelta {
        let mut delta = PathAggregateDelta {
            observed_path_count: u64::try_from(paths.len()).unwrap_or(u64::MAX),
            ..PathAggregateDelta::default()
        };
        for path in paths {
            let previous = self.previous.get(&path.path_id);
            let udp_tx = sub_from_optional(path.udp_tx_bytes, previous.map(|p| p.udp_tx_bytes));
            let udp_rx = sub_from_optional(path.udp_rx_bytes, previous.map(|p| p.udp_rx_bytes));
            if udp_tx > 0 || udp_rx > 0 {
                delta.active_path_count = delta.active_path_count.saturating_add(1);
            }
            delta.udp_tx_bytes = delta.udp_tx_bytes.saturating_add(udp_tx);
            delta.udp_rx_bytes = delta.udp_rx_bytes.saturating_add(udp_rx);
            // Attribute wire bytes to the kind of path that carried them. The
            // selected path alone cannot answer "how much went over the relay"
            // once several paths are moving payload at the same time.
            let by_kind = match path.path_kind {
                "relay" => &mut delta.relay_udp_bytes,
                "aoa" => &mut delta.aoa_udp_bytes,
                "direct" => &mut delta.direct_udp_bytes,
                _ => &mut delta.other_udp_bytes,
            };
            *by_kind = by_kind.saturating_add(udp_tx.saturating_add(udp_rx));
            delta.lost_packets = delta.lost_packets.saturating_add(sub_from_optional(
                path.lost_packets,
                previous.map(|p| p.lost_packets),
            ));
            delta.lost_bytes = delta.lost_bytes.saturating_add(sub_from_optional(
                path.lost_bytes,
                previous.map(|p| p.lost_bytes),
            ));
            delta.congestion_events = delta.congestion_events.saturating_add(sub_from_optional(
                path.congestion_events,
                previous.map(|p| p.congestion_events),
            ));
            delta.spurious_congestion_events =
                delta
                    .spurious_congestion_events
                    .saturating_add(sub_from_optional(
                        path.spurious_congestion_events,
                        previous.map(|p| p.spurious_congestion_events),
                    ));
        }
        self.previous
            .extend(paths.iter().map(|path| (path.path_id, *path)));
        delta
    }
}

/// A path seen for the first time has been counting from zero since it opened,
/// so its current value *is* the delta.
fn sub_from_optional(current: u64, previous: Option<u64>) -> u64 {
    current.saturating_sub(previous.unwrap_or(0))
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct PathAggregateDelta {
    observed_path_count: u64,
    active_path_count: u64,
    udp_tx_bytes: u64,
    udp_rx_bytes: u64,
    lost_packets: u64,
    lost_bytes: u64,
    congestion_events: u64,
    spurious_congestion_events: u64,
    direct_udp_bytes: u64,
    relay_udp_bytes: u64,
    aoa_udp_bytes: u64,
    other_udp_bytes: u64,
}

impl NetworkSnapshot {
    fn capture(connection: &Connection) -> Self {
        // Read the path list once: it yields the selected path, how many paths
        // exist, and the per-path counters that `selected_path()` alone misses.
        let paths = connection.paths();
        let path_count = u64::try_from(paths.len()).unwrap_or(u64::MAX);
        let Some(path) = paths.iter().find(|path| path.is_selected()) else {
            return Self::unavailable("unknown", path_count);
        };
        let path_kind = classify_path(path.remote_addr());
        let stats = path.stats();
        Self {
            path_kind,
            stats_available: true,
            path_count,
            path_id: Some(path.id()),
            rtt: stats.rtt,
            cwnd: stats.cwnd,
            udp_rx_bytes: stats.udp_rx.bytes,
            udp_tx_bytes: stats.udp_tx.bytes,
            lost_packets: stats.lost_packets,
            lost_bytes: stats.lost_bytes,
            congestion_events: stats.congestion_events,
            spurious_congestion_events: stats.spurious_congestion_events,
            current_mtu: stats.current_mtu,
            lost_plpmtud_probes: stats.lost_plpmtud_probes,
        }
    }

    fn unavailable(path_kind: &'static str, path_count: u64) -> Self {
        Self {
            path_kind,
            stats_available: false,
            path_count,
            path_id: None,
            rtt: Duration::ZERO,
            cwnd: 0,
            udp_rx_bytes: 0,
            udp_tx_bytes: 0,
            lost_packets: 0,
            lost_bytes: 0,
            congestion_events: 0,
            spurious_congestion_events: 0,
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
            spurious_congestion_events: self.spurious_congestion_events,
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
            spurious_congestion_events: current
                .spurious_congestion_events
                .saturating_sub(previous.spurious_congestion_events),
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
    spurious_congestion_events: u64,
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
    spurious_congestion_events: u64,
    lost_plpmtud_probes: u64,
}

/// Connection-scoped QUIC counters, aggregated by the QUIC stack over every
/// path of the connection.
///
/// [`PathStats`] describes a single path, and the selected path can be absent:
/// no entry in [`Connection::paths`] reports `is_selected` while iroh is between
/// selections. Both properties silently removed payload from the totals: bytes
/// carried while no path was selected were never counted at all, and every
/// migration voided a full sampling interval as a discontinuity. That is what
/// made provider totals read `udp_tx_bytes_total = 0` or a few kilobytes against
/// a multi-hundred-megabyte payload.
///
/// [`Connection::stats`] has neither problem. It is monotonic for the whole
/// connection and indifferent to path selection, so it carries the authoritative
/// byte counters — and doubles as the yardstick that says how much of the
/// payload the per-path counters managed to observe.
///
/// [`PathStats`]: iroh::endpoint::PathStats
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct ConnectionCounters {
    available: bool,
    udp_tx_bytes: u64,
    udp_rx_bytes: u64,
    /// `STREAM_DATA_BLOCKED` frames sent: this endpoint had stream data ready
    /// and was held back by the peer's `MAX_STREAM_DATA`. On the provider this
    /// is direct proof of a receive-window-bound transfer — evidence the
    /// `bdp_window_ratio` heuristic can only hint at.
    stream_data_blocked_tx: u64,
    /// `STREAM_DATA_BLOCKED` frames received: the peer was held back by the
    /// stream window this endpoint advertises. This is the receiver-side view
    /// of the same event.
    stream_data_blocked_rx: u64,
    /// `DATA_BLOCKED` frames sent: this endpoint was held back by the peer's
    /// connection-level window rather than the per-stream one.
    data_blocked_tx: u64,
}

impl ConnectionCounters {
    fn capture(connection: &Connection) -> Self {
        let stats = connection.stats();
        Self {
            available: true,
            udp_tx_bytes: stats.udp_tx.bytes,
            udp_rx_bytes: stats.udp_rx.bytes,
            stream_data_blocked_tx: stats.frame_tx.stream_data_blocked,
            stream_data_blocked_rx: stats.frame_rx.stream_data_blocked,
            data_blocked_tx: stats.frame_tx.data_blocked,
        }
    }

    /// Counters are monotonic for the lifetime of the connection, so a delta is
    /// only meaningful when both ends of the interval were readable. A dropped
    /// connection reports zeros rather than a spurious jump.
    fn delta_from(self, previous: Self) -> ConnectionCountersDelta {
        if !self.available || !previous.available {
            return ConnectionCountersDelta::default();
        }
        ConnectionCountersDelta {
            available: true,
            udp_tx_bytes: self.udp_tx_bytes.saturating_sub(previous.udp_tx_bytes),
            udp_rx_bytes: self.udp_rx_bytes.saturating_sub(previous.udp_rx_bytes),
            stream_data_blocked_tx: self
                .stream_data_blocked_tx
                .saturating_sub(previous.stream_data_blocked_tx),
            stream_data_blocked_rx: self
                .stream_data_blocked_rx
                .saturating_sub(previous.stream_data_blocked_rx),
            data_blocked_tx: self
                .data_blocked_tx
                .saturating_sub(previous.data_blocked_tx),
        }
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct ConnectionCountersDelta {
    available: bool,
    udp_tx_bytes: u64,
    udp_rx_bytes: u64,
    stream_data_blocked_tx: u64,
    stream_data_blocked_rx: u64,
    data_blocked_tx: u64,
}

/// Running connection-scoped totals plus the path-scoped totals measured over
/// the same interval, so a report can state how much of the connection's
/// traffic the per-path counters actually accounted for.
#[derive(Debug, Default, Clone, Copy)]
struct ConnectionTotals {
    udp_tx_bytes: u64,
    udp_rx_bytes: u64,
    stream_data_blocked_tx: u64,
    stream_data_blocked_rx: u64,
    data_blocked_tx: u64,
    samples_without_stats: u64,
}

impl ConnectionTotals {
    fn accumulate(&mut self, delta: ConnectionCountersDelta) {
        if !delta.available {
            self.samples_without_stats = self.samples_without_stats.saturating_add(1);
            return;
        }
        self.udp_tx_bytes = self.udp_tx_bytes.saturating_add(delta.udp_tx_bytes);
        self.udp_rx_bytes = self.udp_rx_bytes.saturating_add(delta.udp_rx_bytes);
        self.stream_data_blocked_tx = self
            .stream_data_blocked_tx
            .saturating_add(delta.stream_data_blocked_tx);
        self.stream_data_blocked_rx = self
            .stream_data_blocked_rx
            .saturating_add(delta.stream_data_blocked_rx);
        self.data_blocked_tx = self.data_blocked_tx.saturating_add(delta.data_blocked_tx);
    }
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

    fn path_observation(id: u8, udp_tx: u64, lost: u64) -> PathObservation {
        PathObservation {
            path_id: PathId::ZERO.saturating_add(id),
            path_kind: "direct",
            udp_tx_bytes: udp_tx,
            udp_rx_bytes: 0,
            lost_packets: lost,
            lost_bytes: lost * 1_200,
            congestion_events: 0,
            spurious_congestion_events: 0,
        }
    }

    #[test]
    fn path_aggregate_sums_every_path_and_counts_the_active_ones() {
        let mut aggregate = PathAggregate::default();

        let first =
            aggregate.observe(&[path_observation(0, 1_000, 1), path_observation(1, 400, 0)]);
        // A path seen for the first time has counted from zero since it opened,
        // so its whole value belongs to this interval.
        assert_eq!(first.udp_tx_bytes, 1_400);
        assert_eq!(first.lost_packets, 1);
        assert_eq!(first.observed_path_count, 2);
        assert_eq!(first.active_path_count, 2);

        let second =
            aggregate.observe(&[path_observation(0, 3_000, 1), path_observation(1, 400, 0)]);
        assert_eq!(second.udp_tx_bytes, 2_000);
        assert_eq!(second.lost_packets, 0);
        // Path 1 sent nothing this interval, so it is not counted as active.
        assert_eq!(second.active_path_count, 1);
    }

    /// Spurious congestion events are what separate "the link dropped it" from
    /// "we declared it lost too early and backed off for nothing". They have to
    /// aggregate across paths like every other counter, and must not be
    /// confused with `congestion_events` — the relay investigation turns on
    /// telling those two apart.
    #[test]
    fn path_aggregate_sums_spurious_congestion_separately() {
        let mut aggregate = PathAggregate::default();
        let with_events = |id: u8, congestion: u64, spurious: u64| PathObservation {
            congestion_events: congestion,
            spurious_congestion_events: spurious,
            ..path_observation(id, 1_000, 0)
        };

        let first = aggregate.observe(&[with_events(0, 3, 1), with_events(1, 1, 4)]);
        assert_eq!(first.congestion_events, 4);
        assert_eq!(first.spurious_congestion_events, 5);

        // Only path 0 gains events; the delta must reflect that, not the totals.
        let second = aggregate.observe(&[with_events(0, 5, 6), with_events(1, 1, 4)]);
        assert_eq!(second.congestion_events, 2);
        assert_eq!(second.spurious_congestion_events, 5);
    }

    #[test]
    fn path_aggregate_attributes_bytes_to_the_path_kind_that_carried_them() {
        let mut aggregate = PathAggregate::default();
        let relay = PathObservation {
            path_kind: "relay",
            ..path_observation(1, 900, 0)
        };

        let delta = aggregate.observe(&[path_observation(0, 100, 0), relay]);

        // Both paths were moving payload, so a relay ratio taken from the
        // selected path alone would have reported none of this relay traffic.
        assert_eq!(delta.direct_udp_bytes, 100);
        assert_eq!(delta.relay_udp_bytes, 900);
        assert_eq!(delta.aoa_udp_bytes, 0);
        assert_eq!(delta.udp_tx_bytes, 1_000);
    }

    #[test]
    fn path_aggregate_handles_paths_joining_and_leaving() {
        let mut aggregate = PathAggregate::default();
        aggregate.observe(&[path_observation(0, 1_000, 0)]);

        // A new path joins mid-transfer: its counter starts at zero, so adding
        // its current value is the correct delta rather than a spurious jump.
        let joined =
            aggregate.observe(&[path_observation(0, 1_500, 0), path_observation(2, 700, 0)]);
        assert_eq!(joined.udp_tx_bytes, 1_200);
        assert_eq!(joined.observed_path_count, 2);

        // A path leaving must not subtract its history from the running total.
        let left = aggregate.observe(&[path_observation(0, 1_900, 0)]);
        assert_eq!(left.udp_tx_bytes, 400);
        assert_eq!(left.observed_path_count, 1);

        // And when it returns, only the traffic since it was last seen counts.
        // Forgetting a departed path would credit its whole history here.
        let returned =
            aggregate.observe(&[path_observation(0, 1_900, 0), path_observation(2, 1_000, 0)]);
        assert_eq!(returned.udp_tx_bytes, 300);
    }

    #[test]
    fn connection_counters_survive_the_gaps_that_void_path_counters() {
        let first = ConnectionCounters {
            available: true,
            udp_tx_bytes: 1_000,
            udp_rx_bytes: 200,
            stream_data_blocked_tx: 1,
            stream_data_blocked_rx: 0,
            data_blocked_tx: 0,
        };
        let second = ConnectionCounters {
            udp_tx_bytes: 5_000,
            udp_rx_bytes: 260,
            stream_data_blocked_tx: 4,
            ..first
        };

        let delta = second.delta_from(first);
        assert!(delta.available);
        assert_eq!(delta.udp_tx_bytes, 4_000);
        assert_eq!(delta.udp_rx_bytes, 60);
        assert_eq!(delta.stream_data_blocked_tx, 3);

        // A path migration voids the path-scoped delta but must not void this
        // one: the connection counter is the same monotonic series throughout.
        let mut totals = ConnectionTotals::default();
        totals.accumulate(delta);
        let third = ConnectionCounters {
            udp_tx_bytes: 9_000,
            ..second
        };
        totals.accumulate(third.delta_from(second));
        assert_eq!(totals.udp_tx_bytes, 8_000);
        assert_eq!(totals.samples_without_stats, 0);
    }

    /// The event count exists to size what the 10 Hz coalescer drops, so it has
    /// to count items the blob layer produced — including the repeated and
    /// stale offsets that never move the byte position, since the download loop
    /// still had to handle each one.
    #[test]
    fn progress_events_count_every_item_not_every_byte_advance() {
        let start = Instant::now();
        let progress = PublishedProgress::new(start);

        progress.publish(start, 4096);
        progress.publish(start + Duration::from_millis(1), 4096); // repeat
        progress.publish(start + Duration::from_millis(2), 2048); // stale
        progress.publish(start + Duration::from_millis(3), 8192);

        let snapshot = progress.snapshot(start + Duration::from_millis(4));
        assert_eq!(
            snapshot.events, 4,
            "every published item counts, not just the ones that advanced bytes"
        );
        assert_eq!(
            snapshot.bytes, 8192,
            "the byte position still only moves forward"
        );
    }

    #[test]
    fn dropped_connection_reports_no_counters_rather_than_a_jump() {
        let live = ConnectionCounters {
            available: true,
            udp_tx_bytes: 4_000,
            ..ConnectionCounters::default()
        };
        let dropped = ConnectionCounters::default();

        assert_eq!(
            dropped.delta_from(live),
            ConnectionCountersDelta::default(),
            "a dropped connection must not report its counters as zeroed"
        );
        assert_eq!(
            live.delta_from(dropped),
            ConnectionCountersDelta::default(),
            "the first readable sample must not count the whole connection"
        );

        let mut totals = ConnectionTotals::default();
        totals.accumulate(dropped.delta_from(live));
        assert_eq!(totals.udp_tx_bytes, 0);
        assert_eq!(totals.samples_without_stats, 1);
    }

    #[test]
    fn path_counter_delta_flags_path_migration() {
        let base = NetworkSnapshot {
            path_kind: "direct",
            stats_available: true,
            path_count: 1,
            path_id: Some(PathId::ZERO),
            rtt: Duration::from_millis(2),
            cwnd: 1_000,
            udp_rx_bytes: 10_000,
            udp_tx_bytes: 1_000,
            lost_packets: 2,
            lost_bytes: 2_400,
            congestion_events: 1,
            spurious_congestion_events: 1,
            current_mtu: 1_200,
            lost_plpmtud_probes: 1,
        };
        let next = NetworkSnapshot {
            udp_rx_bytes: 12_500,
            udp_tx_bytes: 1_400,
            lost_packets: 3,
            lost_bytes: 3_600,
            congestion_events: 2,
            spurious_congestion_events: 4,
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
                spurious_congestion_events: 3,
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
