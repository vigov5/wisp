# Superseded draft: relay striping on iroh 0.97 (historical record — do not file)

This was a draft upstream report for `n0-computer/iroh`, measured against iroh
0.97.0 with iroh-blobs 0.99, noq 0.17, noq-proto 0.16. **Its diagnosis is wrong
and it must not be filed.** It is kept only because it holds the measurements
taken before the 1.0 upgrade, which `docs/transfer-performance-plan.md` cites;
read it as a record of what 0.97 did, not as an account of why.

> **DO NOT FILE AS WRITTEN — superseded.** This draft's central claim, that the
> relay bias cannot be configured down, is wrong: relay is already registered as
> `TransportBias::backup()` at 0.97, so the striping happened *despite* the path
> being marked `PathStatus::Backup`. The actual defect is one layer lower, in
> `noq-proto 0.16`'s packet scheduler, and `noq-proto 1.1.1` has largely fixed it
> (`may_send_data`). After upgrading to iroh 1.0.3 the relay share fell to 0% in
> three of five runs on this same rig. What is left worth reporting is only the
> residue — 4.3% and 8.5% of wire bytes over relay in the other two runs — and
> that needs to be re-diagnosed against 1.0 before it is written up. See
> "Resolved by upgrading to iroh 1.0 — mostly" in
> `docs/transfer-performance-plan.md`.

## Summary

On a phone-to-desktop Wi-Fi transfer, a relay path carries **6-20% of all UDP
bytes for the whole duration of a bulk transfer**, while a direct path is open,
validated and selected. The share peaks *mid-transfer*, not during connection
setup, so this is not hole-punching latency.

The documented contract is the opposite: `TransportBiasMap::default()` registers
`AddrKind::Relay` as `TransportBias::backup()` (`src/socket/transports.rs:743`),
and the builder docs for `transport_bias` say relay "is a backup transport (only
used when no primary transport is available)".

Binding the receiving endpoint with `RelayMode::Disabled` removes the relay
share entirely and **doubles p10 throughput** (8.3 → 16.8 MiB/s median across
four A/B pairs) while barely moving the median (18.6 → 21.0 MiB/s). The cost is
in the tail, not the average.

## Environment

| | |
| --- | --- |
| iroh | 0.97.0 (noq 0.17.0, noq-proto 0.16.0) |
| Endpoints | `presets::N0`, `RelayMode::Default`, default transport bias |
| Sender | Android 14, Pixel, Wi-Fi 5 GHz |
| Receiver | Windows 11, same 192.168.1.0/24 subnet, Wi-Fi |
| Payload | one 273 MB file over iroh-blobs, single QUIC stream |
| Build | release |

Both peers are on the same LAN and hole punching succeeds: the receiver reports
a direct selected path within the first sample window and never migrates away
from it.

## Measurement

Per-path `PathStats.udp_tx.bytes + udp_rx.bytes`, sampled ~1 Hz, deduplicated by
`PathId` (a path reachable at several transport addresses appears once per
address in `PathInfoList` with identical counters, so summing the list as-is
double counts). A path is classified "relay" only when every address it is
reachable at is a relay address, which makes the relay share a lower bound.

Cross-check: the sum over all paths matches `ConnectionStats.udp_rx.bytes +
udp_tx.bytes` to within 0.01%, and the relay share is exactly 0 in the
`RelayMode::Disabled` arm.

### A/B, four alternating pairs

`no relay` = receiving endpoint bound with `RelayMode::Disabled` (enough on its
own: the dialer can only open relay paths to addresses the receiver advertises).

| metric | relay enabled | relay disabled |
| --- | --- | --- |
| p50 throughput, median | 18.6 MiB/s | 21.0 MiB/s |
| **p10 throughput, median** | **8.3 MiB/s** | **16.8 MiB/s** |
| p10/p50, median | 45% | 79% |
| coefficient of variation, median | 0.33 | 0.16 |
| relay share of wire bytes, median | 16.1% | 0.0% |
| paths opened | 4-6 | 3-4 |
| stall episodes | 1 | 0 |

Per run:

| run | relay | p50 MiB/s | p10 MiB/s | p10/p50 | CV | elapsed |
| --- | --- | --- | --- | --- | --- | --- |
| A1 | on | 20.4 | 6.5 | 32% | 0.42 | 16.1 s |
| A2 | on | 19.8 | 9.1 | 46% | 0.33 | 15.1 s |
| A3 | on | 17.4 | 7.5 | 43% | 0.34 | 16.9 s |
| A4 | on | 15.3 | 9.6 | 63% | 0.19 | 19.1 s |
| B1 | off | 23.8 | 19.1 | 80% | 0.16 | 11.4 s |
| B2 | off | 19.5 | 14.5 | 74% | 0.16 | 14.7 s |
| B3 | off | 17.6 | 13.7 | 78% | 0.24 | 14.7 s |
| B4 | off | 22.6 | 20.7 | 92% | 0.06 | 12.4 s |

Every pair points the same way on p10.

### It is not connection setup

Each relay-enabled run split into deciles of its sample sequence, showing the
relay share of UDP bytes in each:

```
run A1 (relay total 50.0 MB)      run A3 (relay total 60.4 MB)
  d0   0.0%                         d0   2.6%
  d1  44.6%                         d1  36.5%
  d2  67.9%  <- peak                d2  45.7%  <- peak
  d3  59.4%                         d3  43.8%
  d4  21.4%                         d4  31.8%
  d5  10.9%                         d5  15.8%
  d6   4.6%                         d6  23.8%
  d7   6.4%                         d7  11.2%
  d8   5.4%                         d8   6.6%
  d9   6.3%                         d9   7.3%

run A2 (relay total 19.0 MB)      run A4 (relay total 46.2 MB)
  d0   0.0%                         d0   6.6%
  d1   0.1%                         d1  30.2%
  d2   0.1%                         d2  43.0%  <- peak
  d3   1.3%                         d3  16.7%
  d4   4.0%                         d4  11.3%
  d5   9.1%                         d5  11.1%
  d6  13.6%                         d6  11.4%
  d7  14.2%                         d7  10.4%
  d8  11.7%                         d8  11.3%
  d9  19.2%  <- rises to the end    d9   6.8%
```

Three runs peak mid-transfer and none reaches zero; A2 climbs monotonically and
ends at its maximum. In decile 2 of A1 the relay carried 12.6 MB against the
direct path's 6.0 MB — more than half the traffic in that window, over the
relay, with a direct path selected.

For contrast, the same A/B on loopback shows the relay carrying ~16-20 KB per
transfer and no throughput difference at all. The behaviour needs a link where
the relay is within an order of magnitude of the direct path to appear.

## Why this looks like a defect rather than a tuning choice

`noq-proto` does implement backup semantics. In `connection/mod.rs`:

```rust
let have_available_path = self.paths.iter().any(|(id, path)| {
    path.data.validated
        && path.data.local_status() == PathStatus::Available
        && self.remote_cids.contains_key(id)
});
// ...
let path_exclusive_only =
    have_available_path && self.path_data(path_id).local_status() == PathStatus::Backup;
```

and iroh does apply the status: `remote_state.rs` calls
`path.set_status(bias.transport_type.to_path_status())`, which maps
`TransportType::Backup` to `PathStatus::Backup`.

So a relay path should only carry path-exclusive frames while a direct path is
available. It does not. Candidate explanations, none of which can be
distinguished from outside the crate:

1. **`remote_cids` exhaustion.** `have_available_path` additionally requires
   `self.remote_cids.contains_key(id)` for the direct path. If the peer has not
   issued (or has retired) connection IDs for that path, the direct path stops
   counting as available and every backup path becomes eligible for stream data
   — while still reporting itself selected to the application.
2. **Status not applied on the sending side.** The measurements above are taken
   at the receiver; the sender is the Android peer. If `set_status` failed or
   raced there, its scheduler would treat the relay path as ordinary.
3. **Status applied late.** The relay path exists before hole punching
   completes; if the direct path's transition to `Available` does not re-evaluate
   in-flight scheduling, the relay keeps its share.

Hypothesis 1 best matches the shape: the share is bursty and peaks
mid-transfer rather than decaying from the start.

## Impact

The relay path's RTT is an order of magnitude above the direct path's. Blocks
arriving over it are late, and any receiver that verifies a contiguous prefix
(iroh-blobs' BAO verification does) cannot advance past the gap, so the delay
converts directly into an application-visible stall rather than into merely
reordered delivery. That is what the p10 collapse measures: 0/4 relay-enabled
runs meet our stability criterion (p10 >= 70% of median, no stalls), against 4/4
with the relay disabled.

## What we could not work around

- `max_concurrent_multipath_paths` and `set_max_remote_nat_traversal_addresses`
  reject any value below their recommended floors (13 and 12), warning and
  keeping the default (`src/endpoint/quic.rs:467`, `:530`), so the path count
  can be raised but never lowered.
- Narrowing the dial to a single direct address does not help: addresses are
  tracked per remote rather than per dial, and `remote_state.rs` deliberately
  reopens held relay addresses once a direct path comes up ("we may have raced
  this with a relay address") and then triggers hole punching for more.
- `transport_bias` cannot lower the relay below its default, because
  `TransportBias::backup()` is `pub(crate)`; an external caller can only
  construct `TransportBias::primary().with_rtt_disadvantage(..)`, which *raises*
  the relay from Backup to Primary.
- `RelayMode::Disabled` works but is not shippable — it removes the fallback that
  makes non-LAN transfers possible at all.

## Questions

1. Is a relay path carrying a sustained share of stream data while a direct path
   is selected expected at 0.97, or a defect?
2. Is there a supported way to keep the relay strictly as a failover path for a
   bulk transfer, without giving up relay reachability?
3. Would you consider exposing the QUIC `PathStatus` on `PathInfo`? Today
   `PathInfo` gives `id`, `remote_addr`, `is_selected`, `is_closed` and `stats`,
   so an application can see that a relay path is moving bytes but cannot see
   whether the stack still considers it a backup — which is exactly the
   distinction needed to diagnose this from outside.

## Reproducing

1. Two peers on the same LAN, one Android one desktop, default `presets::N0`
   endpoints.
2. Transfer a few hundred MB with iroh-blobs over a single stream.
3. Sample `ConnectionInfo::paths()` at ~1 Hz; for each `PathInfo`, record
   `id()`, `remote_addr()` and `stats().udp_tx.bytes + udp_rx.bytes`.
   Deduplicate by `PathId` before summing.
4. Compare the byte share of relay-addressed paths against wall-clock progress.
5. Repeat with the receiving endpoint bound at `RelayMode::Disabled`.
