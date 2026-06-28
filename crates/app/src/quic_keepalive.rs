use std::sync::Arc;
use std::time::Duration;

use iroh::endpoint::{QuicTransportConfig, VarInt};
use noq_proto::congestion::BbrConfig;

/// Per-stream flow-control receive window advertised by this endpoint.
///
/// A drift transfer pulls a whole collection over **one** QUIC stream, so
/// throughput is hard-capped at `stream_receive_window / RTT`. quinn's default
/// is only 1.25 MB ("tuned for a 100 Mbps × 100 ms link"), which throttles the
/// relay path to ~12.5 MB/s regardless of real bandwidth. We raise it so the
/// window stops being the ceiling on high-RTT paths:
///
/// - **Desktop: 16 MiB** — lifts the relay cap to ~160 MB/s @100 ms RTT.
/// - **Mobile (Android): 8 MiB** — lifts it to ~80 MB/s @100 ms; ~8 MB RAM per
///   active transfer (one stream), safe on modern phones.
///
/// Tiered by build target so the heavier window only costs RAM where there's
/// headroom for it, with no call-site signature churn.
#[cfg(target_os = "android")]
const STREAM_RECEIVE_WINDOW_BYTES: u32 = 8 * 1024 * 1024;

#[cfg(not(target_os = "android"))]
const STREAM_RECEIVE_WINDOW_BYTES: u32 = 16 * 1024 * 1024;

/// Tuned QUIC transport config for drift: keepalive (Android-friendly) plus the
/// throughput knobs that lift the single-stream ceiling on high-latency paths.
///
/// **Keepalive** — iroh caps the QUIC-level path idle/keepalive at 6.5s / 5s
/// respectively (anything larger is logged as a warning and ignored). Higher-
/// level resilience — surviving Doze pauses, NAT churn — is handled by iroh's
/// path migration + relay fallback, not these knobs. We pick values just under
/// the cap so the QUIC layer keeps NAT mappings warm during active transfers
/// without tripping iroh's clamp.
///
/// - `default_path_max_idle_timeout = 6_000ms` — peer must respond within 6s
///   or QUIC tears the path; iroh then re-establishes via its own logic.
/// - `default_path_keep_alive_interval = 4_500ms` — sub-5s ping keeps the
///   common NAT 30-60s binding window alive while transfers are in flight.
///
/// **Throughput**
/// - `stream_receive_window` — see [`STREAM_RECEIVE_WINDOW_BYTES`]; the single
///   biggest win on the relay path.
/// - `send_window` — raised to `8 ×` the stream window (mirroring quinn's
///   default send/stream ratio) so the serving side can keep the larger receive
///   window full and isn't the new bottleneck.
/// - `congestion_controller_factory = BBR` — materially better than the default
///   CUBIC on lossy/variable Wi-Fi and on high-RTT relay paths.
///
/// `initial_mtu` (1200) and MTU discovery are left at iroh's defaults — safe on
/// every path; `receive_window` (connection-level) stays at iroh's
/// `VarInt::MAX` default, so it never limits.
pub(crate) fn build_transport_config() -> QuicTransportConfig {
    // `from_u32` is infallible for our values (both well under VarInt::MAX), and
    // `send_window` is a plain u64 — 8× the stream window.
    let stream_receive_window = VarInt::from_u32(STREAM_RECEIVE_WINDOW_BYTES);
    let send_window = 8u64 * u64::from(STREAM_RECEIVE_WINDOW_BYTES);

    QuicTransportConfig::builder()
        .default_path_max_idle_timeout(Duration::from_millis(6_000))
        .default_path_keep_alive_interval(Duration::from_millis(4_500))
        .stream_receive_window(stream_receive_window)
        .send_window(send_window)
        .congestion_controller_factory(Arc::new(BbrConfig::default()))
        .build()
}

#[cfg(test)]
mod tests {
    use super::build_transport_config;

    #[test]
    fn build_transport_config_runs_without_panic() {
        // Smoke test: the builder accepts the chosen Durations, windows, and the
        // BBR congestion-controller factory, and produces a value. We can't
        // assert internal state because iroh doesn't expose getters on
        // QuicTransportConfig — but a compile-and-run check guards against API
        // drift on iroh/noq-proto upgrades (e.g. the BBR factory type moving).
        let _ = build_transport_config();
    }
}
