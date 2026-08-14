//! Environment switches that exist only to make performance experiments
//! runnable. Never set in a shipped build.

use iroh::RelayMode;

/// Opt-in switch that binds the endpoint with no relay at all.
///
/// Benchmark use only — an endpoint bound this way is reachable **only** over
/// a direct path, so a transfer that cannot hole-punch will not fall back, it
/// will fail. Both peers of an experiment run must set it: the relay paths a
/// connection uses come from the addresses each side advertises, so leaving it
/// off at either end leaves relay addresses on offer.
const BENCH_NO_RELAY_ENV: &str = "WISP_BENCH_NO_RELAY";

/// Returns the relay mode to bind with: [`RelayMode::Disabled`] when
/// [`BENCH_NO_RELAY_ENV`] is set, [`RelayMode::Default`] otherwise.
///
/// This is the second half of the multipath experiment that
/// `WISP_BENCH_SINGLE_PATH` (in `wisp_core::blobs::receive`) could not carry on
/// its own. Telemetry showed 3-8 paths carrying payload at once and a relay
/// path taking 25.7% of a transfer the receiver reported as direct throughout,
/// which raises a question the current numbers cannot answer: does that
/// concurrency add throughput, or do paths with differing RTT reorder enough
/// that BAO verification stalls behind the slowest one?
///
/// Narrowing the dial does not answer it, because iroh tracks addresses per
/// *remote* rather than per dial and deliberately reopens relay paths once a
/// direct path comes up. Neither does transport config: iroh 0.97 floors both
/// `max_concurrent_multipath_paths` and `set_max_remote_nat_traversal_addresses`,
/// warning and keeping the default for any value below the floor. Removing the
/// relay at bind time is what is left — it takes the addresses out of play at
/// the source instead of asking iroh not to reopen them.
///
/// Read as an A/B, the arms differ in relay availability, not only in path
/// count, so a throughput win here is evidence that relay participation costs
/// this workload rather than proof that path count alone does.
///
/// Run on loopback (release, 256 MiB, five alternating pairs) it found no
/// reliable difference — p50 median 49.6 MiB/s with the relay against 73.6
/// without, on ranges that overlap almost entirely, with the last two pairs
/// splitting the win. The relay carried ~16-20 KB per transfer, all before hole
/// punching completed, which is what iroh's `TransportBias::backup()` default
/// predicts. Loopback paths differ by microseconds, so that rules out a large
/// effect on a fast local link and nothing more; the switch is kept for the
/// phone-to-desktop Wi-Fi A/B, where setting it on the receiver alone makes the
/// whole connection relay-free.
pub(crate) fn relay_mode() -> RelayMode {
    relay_mode_for(no_relay())
}

/// Whether the no-relay benchmark switch is set.
///
/// Callers other than [`relay_mode`] need this because removing the relay also
/// removes `Endpoint::online()`: it resolves on a relay handshake, so anything
/// that waits for it hangs forever on a no-relay endpoint. Rendezvous
/// registration is the one that matters here — it has to publish a
/// direct-addresses-only ticket instead of waiting to come online.
pub(crate) fn no_relay() -> bool {
    std::env::var_os(BENCH_NO_RELAY_ENV).is_some()
}

/// The decision itself, split from reading the environment so both arms stay
/// testable without mutating process-wide state.
fn relay_mode_for(no_relay: bool) -> RelayMode {
    if no_relay {
        tracing::warn!(
            target: "wisp_app::bench",
            "{BENCH_NO_RELAY_ENV} set: binding with no relay, direct paths only"
        );
        RelayMode::Disabled
    } else {
        RelayMode::Default
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The switch is opt-in: with it unset, a bind must keep shipping
    /// behaviour, relay included.
    #[test]
    fn unset_switch_keeps_the_relay() {
        assert!(
            matches!(relay_mode_for(false), RelayMode::Default),
            "default bind must keep RelayMode::Default"
        );
    }

    /// And with it set, the relay has to be gone — a mode that merely
    /// deprioritises the relay would leave relay addresses on offer, which is
    /// the whole thing the experiment needs removed.
    #[test]
    fn set_switch_removes_the_relay_entirely() {
        assert!(
            matches!(relay_mode_for(true), RelayMode::Disabled),
            "benchmark bind must leave no relay address to advertise"
        );
    }
}
