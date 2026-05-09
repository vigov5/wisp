use std::time::Duration;

use iroh::endpoint::QuicTransportConfig;

/// Tuned QUIC transport config for Android-friendly keepalive behaviour.
///
/// iroh caps the QUIC-level path idle/keepalive at 6.5s / 5s respectively
/// (anything larger is logged as a warning and ignored). Higher-level
/// resilience — surviving Doze pauses, NAT churn — is handled by iroh's
/// path migration + relay fallback, not these knobs. We pick values just
/// under the cap so the QUIC layer keeps NAT mappings warm during active
/// transfers without tripping iroh's clamp.
///
/// - `default_path_max_idle_timeout = 6_000ms` — peer must respond within 6s
///   or QUIC tears the path; iroh then re-establishes via its own logic.
/// - `default_path_keep_alive_interval = 4_500ms` — sub-5s ping keeps the
///   common NAT 30-60s binding window alive while transfers are in flight.
pub(crate) fn build_transport_config() -> QuicTransportConfig {
    QuicTransportConfig::builder()
        .default_path_max_idle_timeout(Duration::from_millis(6_000))
        .default_path_keep_alive_interval(Duration::from_millis(4_500))
        .build()
}

#[cfg(test)]
mod tests {
    use super::build_transport_config;

    #[test]
    fn build_transport_config_runs_without_panic() {
        // Smoke test: the builder accepts the chosen Durations and produces a
        // value. We can't easily assert internal state because iroh doesn't
        // expose getters on QuicTransportConfig — but a compile-and-run check
        // is enough to guard against API drift on iroh upgrades.
        let _ = build_transport_config();
    }
}
