use std::net::SocketAddr;
use std::time::{Duration, Instant};

use iroh::{
    TransportAddr,
    endpoint::{ConnectionInfo, PathId},
};
use tracing::debug;

use crate::lan::in_usb_tunnel_subnet;

pub(super) const TELEMETRY_SAMPLE_INTERVAL: Duration = Duration::from_millis(250);
const STALL_THRESHOLD: Duration = Duration::from_millis(500);
const TELEMETRY_TARGET: &str = "wisp_transfer_telemetry";

pub(super) fn is_enabled() -> bool {
    tracing::enabled!(target: TELEMETRY_TARGET, tracing::Level::DEBUG)
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
    state: TransferTelemetryState,
    previous_path: Option<PathCounters>,
}

impl BlobTransferTelemetry {
    pub(super) fn new(now: Instant) -> Self {
        Self {
            state: TransferTelemetryState::new(now),
            previous_path: None,
        }
    }

    /// Record raw blob progress. This stays allocation-free because it runs for
    /// every BAO progress item, before application progress is coalesced.
    pub(super) fn observe_progress(&mut self, now: Instant, bytes_received: u64) {
        self.state.observe_progress(now, bytes_received);
    }

    pub(super) fn emit_sample(&mut self, now: Instant, connection: &ConnectionInfo) {
        let app = self.state.sample(now);
        let network = NetworkSnapshot::capture(connection);
        let delta = network.delta_from(self.previous_path);
        self.previous_path = network.counters();

        debug!(
            target: TELEMETRY_TARGET,
            event = "blob_sample",
            sample_ms = duration_millis(app.interval),
            elapsed_ms = duration_millis(app.elapsed),
            bytes_total = app.bytes_total,
            bytes_delta = app.bytes_delta,
            app_bytes_per_sec = app.bytes_per_sec,
            stalled_for_ms = duration_millis(app.stalled_for),
            path = network.path_kind,
            path_stats_available = network.stats_available,
            rtt_us = duration_micros(network.rtt),
            cwnd_bytes = network.cwnd,
            udp_rx_bytes_delta = delta.udp_rx_bytes,
            lost_packets_delta = delta.lost_packets,
            lost_bytes_delta = delta.lost_bytes,
            congestion_events_delta = delta.congestion_events,
            current_mtu = network.current_mtu,
            plpmtud_probe_loss_delta = delta.lost_plpmtud_probes,
            "blob transfer telemetry sample"
        );
    }

    pub(super) fn finish(
        &mut self,
        now: Instant,
        connection: &ConnectionInfo,
        outcome: TransferEnd,
    ) {
        let summary = self.state.finish(now);
        let network = NetworkSnapshot::capture(connection);
        debug!(
            target: TELEMETRY_TARGET,
            event = "blob_summary",
            outcome = outcome.label(),
            elapsed_ms = duration_millis(summary.elapsed),
            bytes_total = summary.bytes_total,
            average_bytes_per_sec = summary.average_bytes_per_sec,
            stall_count = summary.stall_count,
            stall_total_ms = duration_millis(summary.stall_total),
            longest_stall_ms = duration_millis(summary.longest_stall),
            path = network.path_kind,
            path_stats_available = network.stats_available,
            rtt_us = duration_micros(network.rtt),
            cwnd_bytes = network.cwnd,
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
    last_progress_at: Instant,
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
            last_progress_at: now,
            active_stall: false,
            stall_count: 0,
            stall_total: Duration::ZERO,
            longest_stall: Duration::ZERO,
        }
    }

    fn observe_progress(&mut self, now: Instant, bytes_received: u64) {
        if bytes_received <= self.latest_bytes {
            return;
        }
        self.close_active_stall(now);
        self.latest_bytes = bytes_received;
        self.last_progress_at = now;
    }

    fn sample(&mut self, now: Instant) -> ApplicationSample {
        self.detect_stall(now);
        let interval = now.saturating_duration_since(self.last_sample_at);
        let bytes_delta = self.latest_bytes.saturating_sub(self.last_sample_bytes);
        let sample = ApplicationSample {
            interval,
            elapsed: now.saturating_duration_since(self.started_at),
            bytes_total: self.latest_bytes,
            bytes_delta,
            bytes_per_sec: bytes_per_second(bytes_delta, interval),
            stalled_for: if self.active_stall {
                now.saturating_duration_since(self.last_progress_at)
            } else {
                Duration::ZERO
            },
        };
        self.last_sample_at = now;
        self.last_sample_bytes = self.latest_bytes;
        sample
    }

    fn finish(&mut self, now: Instant) -> TransferSummary {
        self.detect_stall(now);
        self.close_active_stall(now);
        let elapsed = now.saturating_duration_since(self.started_at);
        TransferSummary {
            elapsed,
            bytes_total: self.latest_bytes,
            average_bytes_per_sec: bytes_per_second(self.latest_bytes, elapsed),
            stall_count: self.stall_count,
            stall_total: self.stall_total,
            longest_stall: self.longest_stall,
        }
    }

    fn detect_stall(&mut self, now: Instant) {
        if !self.active_stall
            && now.saturating_duration_since(self.last_progress_at) >= STALL_THRESHOLD
        {
            self.active_stall = true;
            self.stall_count = self.stall_count.saturating_add(1);
        }
    }

    fn close_active_stall(&mut self, now: Instant) {
        if !self.active_stall {
            return;
        }
        let duration = now.saturating_duration_since(self.last_progress_at);
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
    stalled_for: Duration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TransferSummary {
    elapsed: Duration,
    bytes_total: u64,
    average_bytes_per_sec: u64,
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
        let Some(previous) = previous.filter(|value| value.path_id == current.path_id) else {
            return PathCountersDelta::default();
        };
        PathCountersDelta {
            udp_rx_bytes: current.udp_rx_bytes.saturating_sub(previous.udp_rx_bytes),
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
    lost_packets: u64,
    lost_bytes: u64,
    congestion_events: u64,
    lost_plpmtud_probes: u64,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct PathCountersDelta {
    udp_rx_bytes: u64,
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
        state.observe_progress(start + Duration::from_millis(100), 100);

        let first = state.sample(start + Duration::from_millis(250));
        assert_eq!(first.bytes_delta, 100);
        assert_eq!(first.bytes_per_sec, 400);
        assert_eq!(first.stalled_for, Duration::ZERO);

        state.observe_progress(start + Duration::from_millis(300), 250);
        let second = state.sample(start + Duration::from_millis(500));
        assert_eq!(second.bytes_delta, 150);
        assert_eq!(second.bytes_per_sec, 600);
    }

    #[test]
    fn stalls_are_counted_once_and_closed_when_progress_resumes() {
        let start = Instant::now();
        let mut state = TransferTelemetryState::new(start);
        state.observe_progress(start + Duration::from_millis(100), 100);

        let sample = state.sample(start + Duration::from_millis(600));
        assert_eq!(sample.stalled_for, Duration::from_millis(500));
        assert_eq!(state.stall_count, 1);

        let sample = state.sample(start + Duration::from_millis(850));
        assert_eq!(sample.stalled_for, Duration::from_millis(750));
        assert_eq!(state.stall_count, 1);

        state.observe_progress(start + Duration::from_millis(900), 200);
        let summary = state.finish(start + Duration::from_millis(1_000));
        assert_eq!(summary.stall_count, 1);
        assert_eq!(summary.stall_total, Duration::from_millis(800));
        assert_eq!(summary.longest_stall, Duration::from_millis(800));
    }

    #[test]
    fn path_counter_delta_resets_after_path_migration() {
        let base = NetworkSnapshot {
            path_kind: "direct",
            stats_available: true,
            path_id: Some(PathId::ZERO),
            rtt: Duration::from_millis(2),
            cwnd: 1_000,
            udp_rx_bytes: 10_000,
            lost_packets: 2,
            lost_bytes: 2_400,
            congestion_events: 1,
            current_mtu: 1_200,
            lost_plpmtud_probes: 1,
        };
        let next = NetworkSnapshot {
            udp_rx_bytes: 12_500,
            lost_packets: 3,
            lost_bytes: 3_600,
            congestion_events: 2,
            lost_plpmtud_probes: 2,
            ..base
        };
        assert_eq!(
            next.delta_from(base.counters()),
            PathCountersDelta {
                udp_rx_bytes: 2_500,
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
            PathCountersDelta::default()
        );
    }
}
