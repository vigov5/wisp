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
/// On loopback it found no reliable difference — the relay never becomes
/// competitive there, carrying ~16-20 KB per run. On Wi-Fi it answers the
/// question: phone to desktop, 273 MB, four alternating pairs, relay share 16.1%
/// against 0%, p10 median 8.3 MiB/s against 16.8, and 0/4 runs passing the
/// stability criteria against 4/4. The median barely moves; the tail doubles.
///
/// All of the above was measured at iroh 0.97. The upgrade to iroh 1.0.3 /
/// noq-proto 1.1.1 largely closes the gap on its own: with the relay still
/// enabled, five runs on the same rig gave a p10 median of 18.3 MiB/s and 3/5
/// passing — at or above what 0.97 could only reach by switching the relay off.
/// The switch is now a diagnostic rather than a lever: two of those five runs
/// still carried 4.3% and 8.5% of wire bytes over a relay, and those are exactly
/// the two that failed. Use it to confirm relay participation is the cause when
/// a run's tail regresses. See "Resolved by upgrading to iroh 1.0 — mostly" in
/// `docs/transfer-performance-plan.md`.
///
/// Setting it on the receiver alone is enough — a dialer can only use the relay
/// addresses the receiver advertises.
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
