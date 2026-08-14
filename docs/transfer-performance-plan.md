# File transfer performance plan

Updated: 2026-08-14
Revised against: rounds 1-3 of `docs/transfer-performance-plan-review.md`

## 1. Goals and how to read status

The goal is to use most of each link's real capacity, hold the rate steady, and cut
the time a user waits between choosing a file and the file being ready at the far
end. Resume, BLAKE3 verification, path safety and memory stability are not traded
away for throughput.

The document uses three distinct states:

- **Implemented:** code is merged or present in the working tree.
- **Functionally verified:** it builds and tests pass; no proof it is faster.
- **Performance verified:** there is a baseline, a reproducible A/B, and numbers
  that meet the threshold.

The changes at commit `8a33818` are only **implemented and functionally
verified**. They are optimization hypotheses that have never been A/B'd; P0 is not
called "complete" until Phase B can attribute an effect to each change.

No further QUIC window, congestion-control or AOA tuning happens before Gate 1 and
Gate 2 at the end of this document are met.

## 2. Current architecture and constraints

- The sender imports files into `iroh_blobs::FsStore` with
  `ImportMode::TryReference`, builds a collection and a blob ticket.
- A transfer is a **pull**: the receiver opens an `iroh-blobs` ALPN connection,
  calls `store.remote().fetch(connection, ticket).stream()`, and bulk data flows
  from sender to receiver.
- One HashSeq and its children are currently handled sequentially over a single
  bidirectional QUIC stream. Many-small-files workloads must be measured
  separately because latency and round trips can matter more than bandwidth.
- The receiver downloads into `.wisp/transfers/<hash>/store`, then exports with
  `ExportMode::TryReference` where the filesystem supports it.
- Control and progress use a separate stream from the file payload.
- The app shares one `iroh::Endpoint` so that multiple endpoints with the same
  identity do not contend for a relay slot.
- The progress channel is still an `mpsc::unbounded_channel`. Backlog risk is
  reduced by coalescing at the source to at most 10 Hz; this plan does not claim
  it has been replaced with a bounded channel.

Pinned versions:

- `iroh 0.97.0`
- `iroh-blobs 0.99.0`
- `noq 0.17.0`
- `noq-proto 0.16.0`
- `tokio 1.50.0`

`noq`/`noq-proto` are the QUIC stack beneath `iroh`; upgrading `iroh` can change
congestion control, multipath, path stats and benchmark results even when Wisp's
own code does not change.

## 3. Current hypotheses, not conclusions

| Hypothesis | Evidence so far | Deciding measurement |
|---|---|---|
| Writing `record.json` per progress item bottlenecks the receiver | Before P0 this could reach thousands of writes/s, with I/O on the async path | Microbench the record write, plus A/B legacy vs checkpoint-only |
| A progress/event storm creates scheduler and UI pressure | BAO progress can be emitted per small block | A/B coalescer-only, measuring CPU, queue lag and app throughput |
| `opt-level = "z"` slows the transfer runtime on mobile | No z-vs-3 data; the existing number only compares opt 0 vs 3 | Bench BLAKE3, AEAD and end-to-end z-vs-3 on device |
| AOA allocation/copying causes GC pauses | Allocations exist on the hot path but there is no profile | CPU/GC trace, USB throughput, A/B buffer reuse |
| Failing to reach a direct path is the biggest throughput loss in the field | Direct and relay differ greatly in capacity; telemetry already sees the path | `time_to_direct_ms`, `wire_relay_bytes_ratio`, direct-success rate |
| The connection spreads payload across several paths at once, causing reordering/HOL and sending part of it over relay even when direct is available | 3-8 paths measured active simultaneously; the selected path carried only 42.6% of bytes; 25.7% of bytes went over a relay path in a direct run | A/B with paths limited, comparing CV, delivery gaps and `wire_relay_bytes_ratio` |
| The window limits relay throughput | Only the theoretical `window / RTT` calculation | BDP/window ratio, raw relay baseline, A/B with a larger window |
| Sequential HashSeq limits many-small-files or a single stream | Current architecture and upstream issue #4286 | files/s, RTT sweep, concurrency/stream A/B |

## 4. P0: implemented but not performance verified

### P0.1 Coalesce blob progress

- Caps updates from the blob layer into the transfer/application layers at 10 Hz.
- Flushes the final byte count before any terminal state.
- Unit tests cover the rate limit and final flush.
- How much this change alone gains is unknown.

### P0.2 Throttle record checkpoints and move them off the Tokio worker

- Checkpoint at most once per second or per 64 MiB.
- Serialize and write via `spawn_blocking`.
- Important state checkpoints are still written immediately.
- Sits downstream of the coalescer, so it must be A/B'd separately to avoid
  double-crediting.

### P0.3 Write records atomically

- Compact JSON, a randomly named temp file created with `create_new`, then renamed
  within the same directory.
- This is primarily a correctness and safety change. Measure its overhead
  separately, but do not drop atomic replacement to make a benchmark look better.

### P0.4 Release dependencies at `opt-level = 3`

- The bridge crate still optimizes for size; transfer and crypto dependencies are
  configured at `opt-level = 3`.
- Do not claim BLAKE3 as the cause until there is z-vs-3 data on aarch64; BLAKE3's
  NEON path may be built through `cc`, and the real win may sit in AEAD or in Rust
  code.

## 5. Phase A — Make telemetry trustworthy

This phase blocks every tuning decision that follows.

The baseline telemetry at commit `afc652f` was functionally verified but is not
treated as measurement-valid until A1-A4 are complete.

**Update 2026-08-14 — commit `7036d47`:** A1-A4 and the analyzer part of A6 are
implemented, with format/test/analyze all passing locally. Receiver and provider
share one benchmark correlation token; the provider has correct sender-side
cwnd/loss/UDP TX; window and config are logged; warm-up and finalization are
separated from mid-transfer stalls; an EOF without `Done` is a failure; the sampler
task does not cancel the download future; a path change forces a sample and the
analyzer uses weighted 1-second windows. Commit `bb1bb93` adds A5 phase timing for
the core sender and receiver plus analyzer schema v3. Commit `ced0e5e` measures
Android SAF URI → app-cache and background save app-cache → SAF/MediaStore,
correlated by the same token, with mobile telemetry off by default. Commit
`1fe1c25` wires the same opt-in flag into Rust provider telemetry on mobile,
commit `3b6aff0` emits that target as typed JSON without span context, and commit
`dca51e3` classifies stalls as transport-active delivery gap, transport-idle or
unknown without loosening the stability gate. Commit `bcbde90` replaced the
session base-conversion with a BLAKE3 pseudonymous token, publishes first and
latest progress timestamps from the hot loop, and keeps the active stall on
failure. Commit `f978003` joins provider loss by token and timeline to separate
loss recovery, computes RTT inflation and BDP on the same 1-second window, and
fixes the coverage denominator. Commit `26b4f7c` renames the provider receive
window to `local_*`, adds `config_source`, stops treating a value copied from
noq-proto as measured or configured, and flags path-counter discontinuities so
under-counted totals are no longer read as complete. The analyzer is now at schema
v5 and still reads older schemas. A recheck before the device smoke found the Dart
phase emitter still using the base-converted session hex while the core had moved
to BLAKE3; commit `b5820c5` puts the core-generated token into `TransferPlanData`,
so mobile phases and Rust events can no longer diverge on the correlation
algorithm.

**Update — transport counter provenance has a root cause and a fix (schema v6):**
network counters previously read `PathStats` only through
`ConnectionInfo::selected_path()`. In iroh 0.97 `selected_path()` is
`paths().find(|p| p.is_selected())`, and `is_selected` is set only while the
selected-path watcher holds an address matching a live path. Bytes carried while
no path was selected were therefore never counted, and every migration lost a
whole sampling interval. That is the mechanism behind `udp_tx_bytes_total=0` and
`4,261 bytes` against a payload of hundreds of MiB.

The fix adds **connection**-scoped counters from `ConnectionInfo::stats()`
(`udp_tx`/`udp_rx` plus frame counters). These are monotonic for the whole
connection and independent of path selection, making them the trustworthy byte
figures; the ratio of path counters to connection counters becomes
`path_counter_coverage`, which is exactly the provenance check Gate 1 was missing.

A 64 MiB CLI-to-CLI smoke (loopback, direct, `0` discontinuities, no migration)
shows the problem is wider than the device report suggested:

| Counter | Provider | Receiver |
|---|---:|---:|
| Path-scoped | 57,591,003 | 57,590,345 |
| Connection-scoped | 68,938,270 | 68,932,812 |
| `path_counter_coverage` | 83.5% | 83.5% |

The payload was 67,108,864 bytes. The connection counters correspond to about 2.7%
overhead, correct for QUIC over UDP. The path counters come out **below the
payload itself**, so they cannot be a correct UDP byte count. In other words the
path counters fall roughly 16.5% short even in the cleanest case, not only during
a migration or when `path=unknown`. Every path-scoped loss and cwnd figure in this
document must be read as a lower bound tied to that run's coverage.

**The mechanism of the shortfall is identified.** Samples now log `path_count`, and
a repeated smoke showed the connection holding up to **4 paths** at once
(`path_count` taking values 1, 3, 4 at both ends) at 85.0% coverage. `noq-proto`
increments `stats.udp_tx` and `path_stats[path_id].udp_tx` at the same sites with
the same values, so the two series do not contradict each other — the path counter
simply describes **one** path among several in use. The selected path's
`PathStats` is therefore not the denominator for "how much was sent or received",
and this is not an edge case: it holds for every multipath connection, including a
stable direct LAN.

Reports consequently distinguish two diagnoses with the same symptom:
`path_count > 1` means the selected path carries only part of the traffic;
`path_count == 1` with low coverage means some samples had no path selected.

**A real-device smoke (desktop → Pixel 4 release, Wi-Fi 192.168.1.x, 128 MiB
direct) reproduces Gate 1's worst case exactly, and shows the fix handles it:**

| | Provider (desktop) | Receiver (Android) |
|---|---:|---:|
| Reported `path` | `unknown` | direct |
| `path_count` | 6 | 6 |
| Path-scoped total | **0** | 6,278,860 |
| Connection-scoped total | **137,971,390** | 137,969,068 |
| Coverage | **0.0%** | 4.6% |

Payload 134,217,728 bytes. The provider is precisely the previously reported
symptom — `path=unknown` and a path counter of 0 — so under schema v5 this run
would have had no usable network figures. Under schema v6 the connection counters
record 137.97 MB at both ends (2,322 bytes apart, 2.8% overhead against the
payload), averaging 11.93 MB/s, which agrees with the receiver's 11.7 MiB/s app
median. The receiver reported `outcome=complete`, so BLAKE3 verification passed.
`stream_data_blocked` was 0 at both ends, so this run was **not** window-bound —
measured, not assumed.

Conclusion: byte accounting no longer depends on whether iroh managed to select a
path. Path-scoped numbers are unchanged but always travel with their coverage, so
they cannot be misread as the whole of the traffic.

**The reverse direction (Android release provider → desktop receiver, 128 MiB
direct) shows the quantitative cost of the old bug.** The file was the payload
received in the previous direction, and the desktop → Android → desktop round-trip
hash matched byte for byte.

| | Android provider | Desktop receiver |
|---|---:|---:|
| Reported `path` | `unknown` | direct |
| `path_count` | 6 | 6 |
| Path-scoped total | 16,298,505 | — |
| Connection-scoped total | 146,552,381 | 138,288,810 |
| Coverage | 11.1% | 21.0% |
| Average throughput | **1.96 MiB/s** via path | — |
| | **17.66 MiB/s** via connection | |

The same run and the same counters, read two ways, differ by a factor of **9**.
The path-scoped number (1.96 MiB/s) is what the old telemetry would report, and it
would have sent the whole performance investigation chasing a phenomenon that does
not exist. The connection-scoped number (17.66 MiB/s) agrees with the receiver's
16.4 MiB/s app median.

The sender/receiver discrepancy at connection level also carries physical meaning:
the sender sent 146.55 MB for a 134.22 MB payload (9.2% overhead) while the
receiver got 138.29 MB (3.0% overhead). The roughly 8.2 MB difference is lost and
retransmitted data — consistent with the loss and congestion events the provider
recorded, plus one path discontinuity. All of that was previously invisible.
`stream_data_blocked` was 0 at both ends, so this run was not window-bound either.

The fix also logs `stream_data_blocked` (tx on the provider, rx on the receiver).
The frame means "the sender had data ready and the receiver's `MAX_STREAM_DATA`
held it back", so it answers the window-bound question directly instead of
inferring from `bdp_window_ratio`. In the smoke above both agreed: `0` blocked
frames and `bdp_window_ratio_p90 = 0.0033`.

Gate 1 is **closed for desktop and Android**. Schema v6 emission, correlation
token, phase timing, provenance counters and the discontinuity flag are all
verified on release builds in both directions. iOS is tracked as a separate item
and no longer blocks Gate 1; see §11.

**Real smokes on 2026-08-14:**

- CLI debug ↔ CLI debug over rendezvous/loopback transferred 128 MiB with matching
  SHA-256; the analyzer joined receiver, provider and 13 phases at schema v3 with
  `0` malformed lines. This confirms the schema only, not performance, because it
  is a debug build over loopback.
- Desktop CLI debug → Pixel 4 Android release transferred 64 MiB direct with
  matching SHA-256; the provider recorded 60 samples over 14.481 s, 2 packet
  loss/congestion events early on, and mobile `background_save=171 ms`; the
  analyzer joined 9 phases by pseudonymous run ID with `0` malformed. The UI
  reported about 4.4 MB/s and provider UDP TX averaged about 4.2 MiB/s. Not usable
  as a baseline: the sender was a debug build, the file synthetic, one run only,
  and no iperf3 in the same session.
- Android release sender → desktop CLI debug receiver transferred 64 MiB direct
  twice with matching SHA-256. `saf_read_copy` was only 71-106 ms while
  `fetch_store` took 46.854-59.955 s, so SAF/cache is not the bottleneck for the
  current large-file workload. The run with complete provider JSON recorded 190
  samples over 46.417 s, UDP TX averaging about 1.4 MiB/s, 4,813 packets /
  6,988,224 bytes lost, 71 congestion events, a final RTT of about 3.057 s, CUBIC,
  MTU 1452, a provider-local receive window of 8 MiB and a send window of 64 MiB —
  that local receive window does not govern the bulk being sent. The receiver saw
  p10 0.0 MiB/s, median 1.2 MiB/s, CV 1.62 and 3 stalls; the analyzer joined 14
  phases with `0` malformed and `--fail-on-unstable` failed as expected. In each
  gap the app bytes stood still while the receiver kept taking roughly 4.6-12.1 MB
  of UDP. Cross-referencing the provider and receiver timelines (terminals only
  35 ms apart) showed the first gap, about 7.7 s long, had no sender
  loss/congestion, while gaps 2 and 3 coincided with 2,877 and 1,532 lost packets
  and 37 and 28 congestion events respectively. Re-analysis with `f978003` /
  schema v4 correctly reclassified the latter two as
  `transport_active_loss_recovery`, leaving only the first gap as an unexplained
  delivery gap. Schema v5 re-reading the same log still gives `0` malformed, two
  loss-recovery episodes, one delivery gap, a 35 ms receiver/provider terminal
  skew and coverage 1.0.
- A schema v5 smoke, Android release sender → desktop CLI debug receiver,
  transferred 85,540,428 bytes direct in 152.7 s with a matching output SHA-256.
  The Android provider emitted `config_source=configured`, correct local window
  naming, `path_counter_discontinuity=false` on samples and a count of `0` in the
  summary. Dart reported `saf_read_copy=126 ms`, and the Rust provider and desktop
  receiver shared one `benchmark_run_id` token; the analyzer joined 14 phases and
  614 provider samples with `0` malformed. This run had 5 loss-recovery stalls, 8
  delivery gaps, 5,513 lost packets, 104 congestion events, p10 `0`, p50 about
  0.3 MiB/s and 80.1 s of stall. The final QUIC RTT reached about 7.5 s on the
  provider and 8.1 s on the receiver, so it is only usable for schema and
  correlation acceptance, not as a baseline or tuning input.
- After moving the Pixel to `TienNA 5G`, two fresh 256 MiB random smokes both
  completed direct with a matching SHA-256 of `0994D6F7...DC84BA9`. Desktop →
  Android took 12.903 s, averaging 19.84 MiB/s, p10/p50 17.7/21.1 MiB/s, CV 0.187,
  p10/p50 at 84.2% and no stalls. Android → desktop took 12.252 s, averaging
  20.89 MiB/s, p10/p50 15.5/21.6 MiB/s, CV 0.232, p10/p50 at 71.6% and no stalls.
  `saf_read_copy=250 ms` and Android `background_save=471 ms`; the control
  handshake took only 24-37 ms instead of the 30-second timeout seen on the old
  topology. This is strong evidence that the earlier Wi-Fi 4 / cross-band batch was
  dominated by topology, but it is still not an absolute baseline: no iperf3
  bracket, no host BSSID, and not a release build at both ends.
- The receiver core on Android release emitted complete schema v5: real config,
  samples and summary, correlation token, Rust and Dart phases, `0` malformed.
  However the two 256 MiB runs revealed that provider telemetry was not yet
  measurement-valid for bulk: the Android provider reported `path=unknown` and
  `udp_tx_bytes_total=0`, and the desktop provider counted only 4,261 bytes against
  a 256 MiB payload. The receiver's selected-path counters also under-accounted the
  payload, and the desktop → Android run had two counter discontinuities. App
  throughput, phases and stalls from those runs remain usable, while provider
  loss/cwnd/UDP, loss attribution and network totals had to be treated as
  unavailable or lower bounds until either the sampler was shown to be holding the
  right payload connection or the analyzer downgraded validity by coverage.
- A follow-up produced a valid raw bracket for the desktop → Android direction: a
  `.NET TcpClient` with a 1 MiB buffer sending 256 MiB into Android
  `toybox nc >/dev/null`. Three runs before reached 29.97/30.19/30.99 MiB/s and
  three after reached 26.86/31.44/28.97 MiB/s; medians before/after were
  30.19/28.97 MiB/s, a 4.0% drift. Wisp in the middle used a fresh random file
  with a matching hash, but the path went direct → relay → direct → relay and
  95.0% of the payload was attributed to relay. That run averaged only
  11.79 MiB/s, p10/p50 near 0/14.22 MiB/s, CV 0.460, with one transport-idle stall
  of 3,529 ms; against a mean-of-medians raw figure of 29.58 MiB/s, utilization was
  only 39.9% on average and 48.1% at p50. The provider tracked the bulk this time,
  counting 251.2 MiB UDP TX, 16 lost packets, 4 congestion events and 3
  discontinuities; the receiver saw 5 discontinuities. This is a genuine
  path-migration smoke proving the lower-bound handling works, and it puts D1 ahead
  of any window or CC A/B.
- Opt-in Rust telemetry on Android is wired from the same Dart define. The target
  is separated from the text `tracing_android` formatter and emits typed JSON with
  no span context; the smoke produced 201 events, `0` legacy text lines and `0`
  matches for path/peer/session ID. Do not use a regex parser over text with
  concatenated fields: it is both fragile and liable to pull sensitive metadata out
  of spans.
- Both sender runs reproduced stalls despite good RSSI and thermals. The data
  localizes the problem to the network/QUIC path after prepare, but Wi-Fi 4 noise
  and retries cannot yet be separated from loss-recovery/CUBIC behaviour because
  there was no stable raw baseline bracketing the batch.
- A stopgap baseline that installs no extra binaries used Android
  `toybox dd | nc` to send raw TCP 128 MiB in the same direction to Windows. Two
  runs reached 1.526 and 1.556 MiB/s, about 2% apart; the run with a 1-second timer
  gave p10 1.298, median 1.591 MiB/s, CV 0.186 and no zero-window events. Wisp
  reached about 89% of the raw TCP ceiling on average but was markedly less stable
  in application delivery during that session. Two fresh raw runs immediately
  before the next batch reached only 1.119/1.087 MiB/s with CV 0.553/0.444 and 8 of
  10 one-second windows empty, despite RSSI around -34 dBm. The conclusion "the
  problem is not in the link" is therefore no longer supported; Wi-Fi 4 / 2.4 GHz
  varies strongly over time and this batch is excluded from A/B tuning. This is a
  stopgap denominator and does not replace iperf3, which has proper
  retransmit/cwnd and multi-stream reporting. ICMP in the same session ranged
  7-713 ms, useful only as a queue/jitter hint and not as a throughput baseline
  since it can be deprioritized.
- Android provider/receiver schema v5 and the Dart boundary are confirmed on
  release; the iOS boundary is not. Provider-only support is retained for
  sender-only logs and was fixed in commit `7e7b3c2`.

### Round 2 review outcomes

- V1, V2 and V5 were fixed in `bcbde90`, with the Dart boundary brought into line
  in `b5820c5`: the token is no longer a reversible base conversion in any emitter,
  timestamps come from the hot loop, and a failed terminal does not lose the active
  stall.
- V3, V4 and the path-counter part of V8 were fixed in `26b4f7c`: provider windows
  carry local meaning, config source separates configured/assumed/unknown,
  migrations raise a discontinuity flag, and the analyzer treats network totals as
  lower bounds when one occurs.
- The BDP/coverage part of V8 and V11 were fixed in `f978003`: throughput and RTT
  use the same 1-second window, and provider loss is joined only when the token is
  unique and the terminal timelines are close enough. A discontinuity overlapping a
  stall downgrades attribution to `unknown`.
- V6 was checked against the `iroh-blobs 0.99.0` contract; V7 was checked against
  the `n0-watcher 0.6.1` cancel-safety documentation, with the evidence recorded
  next to the `select!`.
- V9 became E1b but only runs after the raw bracket is stable; V10 no longer
  supports the conclusion "not the link", because the fresh raw Wi-Fi 4 runs also
  burst and zero-window.
- V12 keeps a single Gate 1 status in §11. V13 is a trust-proxy/security issue
  outside this plan's scope and must be handled in its own commit.

### A1. Read QUIC stats from the right direction

The receiver is the bulk-receiving end. On the receiver:

- Usable: application bytes, `udp_rx.bytes`, RTT, selected path and current MTU.
- `cwnd`, sent loss and congestion events describe the receiver's own sending
  direction, mostly ACK and control traffic; keep them under `local_*` names and do
  not draw bulk conclusions from them.
- Judging CUBIC vs BBR or payload loss requires path stats collected at the blob
  provider on the sender, with logs tied together by a shared correlation token.
  Since `bcbde90` the token is a BLAKE3 domain-separated pseudonym rather than a
  base-converted session ID.

Add to every sample and config record:

- The receiver's actual `stream_receive_window_bytes`,
  `connection_receive_window_bytes` and `send_window_bytes`. The provider must
  prefix receive-side fields with `local_` so they are not misread as the
  receiver's flow-control credit.
- The congestion controller and build profile in use.
- `config_source=measured|configured|assumed_upstream_default|unknown`; a value
  hand-copied from noq-proto must not be marked `known=true`.
- `role=receiver|provider` so a parser does not mix the two sides' semantics.
- Connection-scoped counters from `ConnectionInfo::stats()`:
  `connection_udp_tx_bytes_delta` / `connection_udp_rx_bytes_delta` plus
  `connection_stats_available`. These are the trustworthy byte figures; path
  counters are only lower bounds and must always be read with
  `path_counter_coverage`.
- `path_count` and `active_path_count`: how many paths the connection holds, and
  how many actually carried bytes in that sample. Above 1 means the selected-path
  counter describes only part of the traffic — a different situation from having no
  path selected at all.
- Aggregates over **every** path: `all_paths_udp_tx/rx_bytes_delta` and
  `all_paths_lost_packets_delta`. Summed per `PathId`, so loss stops being a lower
  bound. Two conditions are required for these numbers to be right: deduplicate by
  `PathId` within each snapshot (a path can appear once per transport address, each
  entry carrying the same counters), and retain entries for paths that have left
  the list (a path can come back). Dropping either skews the total to 1.07x and
  1.70x of the connection figure; with both, it matches to 1.0001x.
- `direct_path_udp_bytes_delta` / `relay_path_udp_bytes_delta` /
  `aoa_path_udp_bytes_delta`: wire bytes attributed to the kind of path that
  carried them. This is the only way to answer "how much went over the relay" on a
  multipath connection.
- `stream_data_blocked_tx/rx` and `data_blocked_tx`. These are direct evidence of
  flow control: the sender had data ready and the receiver's window held it back.

Derived diagnostic:

```text
bdp_window_ratio = app_bytes_per_sec * rtt_seconds / stream_receive_window_bytes
```

A sustained `bdp_window_ratio >= 0.8` is only a signal that the transfer **may be**
window-bound. Compute the ratio from the same 1-second throughput window; do not
mix a 250 ms app rate with 1-second percentiles. From schema v6,
`stream_data_blocked` is far stronger evidence: unlike an inferred ratio, the frame
is only emitted when the sender is genuinely blocked by the window. Use the ratio
to screen and the frame to conclude. Confirm only when changing the window changes
throughput reproducibly and the raw path still has headroom. Also compute
`rtt_inflation = current_rtt / min_rtt` to detect queueing.

### A2. Fix the definitions of warm-up, stall and terminal outcome

- Measure `time_to_first_byte_ms` separately; the time from connect to the first
  byte is not a mid-transfer stall.
- Arm the stall detector only after the first byte increase, and end it on an
  explicit `GetProgressItem::Done`.
- A stream ending in `None` without `Done` must be `failed/incomplete`, never
  `outcome=complete`.
- The byte counter and the first/last increase timestamps must be published from
  the download loop. Timestamping only at the 250 ms sampler over-measures TTFB and
  under-measures stalls by up to 250 ms in one direction; a real 600 ms stall can
  slip past a 500 ms threshold.
- Keep `stall_count`, `stall_total_ms` and `longest_stall_ms` distinct from warm-up
  and finalization pauses. A stall still open at `complete` is finalization; at
  `failed` it must stay a failure stall and not be subtracted from the summary.
- The `iroh-blobs 0.99.0` contract confirms `Done` means completed, `Error` means
  closed but incomplete, and `GetProgress::complete()` turns a stream closed
  without a result into `LocalFailure`; `None -> Failed` therefore matches upstream
  semantics and has a unit test.

### A3. Remove the observer effect from the download loop

Telemetry-on must not repeatedly cancel `stream.next()` every 250 ms.

Preferred design:

- The download loop keeps using `stream.next().await` in both production and
  benchmark builds.
- With telemetry on, the loop only updates an atomic byte counter.
- An optional sampler task reads the byte counter and `ConnectionInfo` every
  250 ms, with an explicit stop and final flush and no unbounded data channel.
- `n0-watcher 0.6.1::Watcher::updated()` is documented upstream as cancel-safe;
  keep that version/source evidence next to the `select!` and test path-change
  accounting.
- If `select!` is kept, `next()` must be shown cancel-safe from API documentation
  and an integration test; without that evidence, do not use the design.

### A4. Path metrics and bottleneck classification

- `time_to_direct_ms`: from the start of the dial/transfer to the first time the
  selected path is direct; record `never_direct=true` if it never happens.
- `relay_bytes_ratio`: total application byte delta while the selected path is
  relay, divided by total transferred bytes.
- `direct_bytes_ratio` and the number of path migrations.
- Compare `udp_rx_bytes_delta` against `app_bytes_delta`:
  - UDP still rising while the app offset stands still across several consecutive
    samples, with provider loss on the same timeline ⇒
    `transport_active_loss_recovery` (HOL/retransmit).
  - UDP still rising, app flat, but no provider loss ⇒
    `transport_active_delivery_gap`; only now investigate
    reorder/verify/store/disk.
  - Both UDP and app flat ⇒ suspect the sender, the path, flow/congestion control
    or the upstream store read.
- Join loss only when there is exactly one matching provider and the terminal
  timelines are close enough; otherwise keep `unknown` or a broad delivery gap
  rather than asserting recovery.
- This is a heuristic; `GetProgressItem::Progress` is an aggregated payload prefix,
  so out-of-order packets can freeze the offset while UDP keeps rising. Always
  combine provider stats and phase timing before concluding.

### A5. Complete phase timing

Instrument both sender and receiver:

- Walk/metadata, SAF read/copy, import/hash, and time until the blob ticket is
  ready.
- Handshake, time-to-first-byte and network transfer.
- Receiver store/verify and final export.
- On Android and iOS, also measure background save until the file is genuinely
  ready, not just when the protocol reports completed.

### A6. Analyzer and harness

- Stream the JSONL parser, grouping by `(source_log, transfer_id)` so several
  processes each starting their counter at 1 are not merged.
- Bound file size, line length and sample count; use only JSON primitives, never
  `pickle` or `eval`.
- Emit both machine-readable JSON and a human-readable table.
- Resample 250 ms samples into non-overlapping 1-second windows before computing
  p10/p50/p90 throughput and the coefficient of variation; weight by `sample_ms`
  when a tick is late.
- Compute low-speed episodes, stalls, path byte ratios and phase timings; force a
  sample on path change to reduce bytes misattributed to the new path.
- Allow provider-only reports when a mobile release has only boundary phases;
  still join phases by the pseudonymous correlation token, but never fabricate
  receiver throughput or stability. `--fail-on-unstable` must fail when there are
  no valid receiver samples.
- Report a path-counter discontinuity when the selected path changes instead of
  silently under-counting totals; `network_stats_coverage` must divide by sample
  count, not by throughput window count.
- `tools/analyze_transfer_telemetry.py` currently emits schema v6 reports; treat a
  schema as accepted only after an emission smoke on the targets still open at
  Gate 1.

## 6. Phase B — Build baselines and attribute P0

### B1. Absolute baselines

Each path needs two reference points:

1. **Link baseline:** iperf3 for LAN/AOA, to know the real TCP/UDP capacity.
2. **Transport baseline:** a raw QUIC echo/source-sink using the same iroh/noq,
   encryption and path as Wisp but without the blob store, hashing or export.

Also measure local baselines:

- Sequential file read/write on the real filesystem.
- Hash throughput and AEAD throughput at the exact release profile and
  architecture.
- SAF/provider read on Android; file-provider access on iOS if applicable.

First-class metrics:

```text
link_utilization      = app_payload_throughput / link_baseline_throughput
transport_utilization = app_payload_throughput / raw_quic_throughput
```

The initial threshold for investigation is below 70% of the appropriate baseline.
The final release threshold will be fixed once there is data per device and path
class; a single shared number must not be used to hide mobile disk or CPU limits.

#### Measured: phone to desktop over Wi-Fi

Link baseline, phone to desktop, 256 MiB of TCP through `nc` into a socket sink,
four runs: **27.7 MiB/s** (27.5 / 27.7 / 27.7 / 27.9 — the link is remarkably
steady). Same direction, same subnet and same session as the transfer runs.

Local baselines (`cargo test --release -p wisp-core --test baselines --
--ignored --nocapture`):

| baseline | desktop receiver | phone sender |
| --- | --- | --- |
| BLAKE3 hash | 2,879 MiB/s | not measured |
| Sequential disk write incl. `fsync` | 601 MiB/s | not measured |
| Sequential read | 1,893 MiB/s (warm cache) | ~1,100 MiB/s (warm cache) |

So on this path:

```text
link_utilization (relay disabled) = 21.0 / 27.7 = 76%
link_utilization (relay enabled)  = 18.6 / 27.7 = 67%
```

**Neither end is locally bottlenecked.** Hashing has ~137x headroom over app
throughput and the receiver's disk ~29x; the phone's storage read is over 50x
the link. The transfer sits at 76% of raw TCP with the relay out of the way, and
drops below the 70% investigation threshold only when the relay is striping (see
D2). That reframes the desktop-receive Wi-Fi case: there is no local bottleneck
left to find, and the remaining ~24% is QUIC/AEAD overhead plus whatever the blob
layer adds.

#### The transport baseline closes the gap

`crates/core/examples/quic_baseline.rs` is a source/sink over the same iroh,
encryption, transport config and path, with the blob store, BAO verification,
record writes and export removed. Cross-compiled for `aarch64-linux-android`
and run from `adb shell`, so the source is the same phone that runs the app.

Ten baseline runs against nine app runs (relay disabled on both, 256-273 MB,
same session):

| | app payload | raw QUIC | TCP |
| --- | --- | --- | --- |
| whole-transfer average, median | 23.0 MiB/s | 24.4 MiB/s | 27.7 MiB/s |
| range | 16.0-25.9 | 20.6-26.6 | 27.5-30.0 |
| p10, median | 20.9 MiB/s | 21.2 MiB/s | not windowed |
| p10/p50, median | 83% | 88% | not windowed |

```text
transport_utilization = 23.0 / 24.4 = 94%
link_utilization      = 23.0 / 27.7 = 83%   (raw QUIC itself is 88% of TCP)
```

**Everything above the transport costs 6%.** The earlier ~24% gap between app
throughput and raw TCP is now attributed, and almost all of it is QUIC against
TCP (12%), not the blob layer. BAO verification, the store, export, record
checkpoints and progress plumbing together account for the remaining 6%.

That closes the question this section opened. On this path there is no
meaningful headroom above the transport, so further work on the record/export/
progress path cannot be justified on throughput grounds — which is consistent
with B2 finding P0.2 and P0.3 already down at 0.34% and 0.07% of wall time.

Two things the comparison does surface:

- **The app's one stall was slow connection setup, not a stall.** One app run in
  nine showed two `transport_idle` stalls. Reading that run window by window
  places both at the *start* of the fetch phase: six seconds carrying only
  2.4 KB probe-sized samples, then `path_count` goes 3 to 5 and the transfer
  immediately runs at full rate to the end. Every other run reached 1 MB/s
  within 0.3 s, on both relay arms (17 runs). So this is the blob connection
  taking six seconds to find a usable path, not the sender going quiet
  mid-transfer — **a D1 question, not D3.** With one occurrence there is no
  basis for saying whether the relay would have covered it; the run that hit it
  had the relay disabled.
- **One run in nine failed after transferring at full speed**, with
  `running receiver session: reading message length: connection lost` during the
  final control exchange. Throughput was 25.4 MiB/s right up to the end. That is
  a reliability item, not a performance one, and it is not tracked anywhere else
  in this plan.

### B2. A/B to attribute the changes already landed

Rebuild a baseline from the commit before `8a33818`, then produce builds differing
in one variable each. Historical builds run only in a throwaway benchmark
directory; never point one at user data, because the old record write is not
atomic.

| Build | Record format/write | Coalescer | Record checkpoint | Release dependency |
|---|---|---:|---:|---:|
| H — exact historical | pretty / direct write | off | per-progress, blocking in async | z |
| A — controlled baseline | compact / atomic | off | per-progress via blocking pool | z |
| B — record-only | compact / atomic | off | 1 s / 64 MiB + blocking pool | z |
| C — coalescer-only | compact / atomic | 10 Hz | per-progress via blocking pool | z |
| D — P0 runtime | compact / atomic | 10 Hz | 1 s / 64 MiB + blocking pool | z |
| E — current release | compact / atomic | 10 Hz | 1 s / 64 MiB + blocking pool | 3 |

- H vs A only gives the total historical/correctness-rewrite difference;
  attribution for the coalescer and the checkpoint comes from A-D, where atomic
  writes are held constant.
- Include a direct-write-vs-atomic microbench in a scratch directory to isolate
  record format and write overhead without producing an unsafe production build.
- Include a microbench feeding progress at 10, 100, 1,000 and roughly 6,400
  events/s to measure CPU, write count and scheduler delay.
- Report effect size and confidence intervals, not just the fastest run.

Required output: an attribution table for the coalescer, the checkpoint and z-vs-3.
If a change shows no measurable win, keep or drop it on correctness and complexity
grounds rather than continuing to credit it with performance.

#### Measured: unit cost times event rate

The build matrix above has not been run. What has been measured is the two
quantities it multiplies, which is enough to attribute P0.1 against P0.2 without
double-crediting either.

**Event rate.** The receiver now counts every progress item the blob layer
produces, before coalescing (`progress_events_delta` per window,
`progress_events_total` in the summary). On a 273 MB phone-to-desktop Wi-Fi
transfer: **35,009 items in 21.5 s = 1,625 events/s**, roughly one item per
9 KB of payload.

**Checkpoint cost.** `baseline_record_checkpoint_write`, release, 200-item
manifest (15.6 KB compact record), desktop NVMe:

| write shape | cost per save |
| --- | --- |
| shipped: compact JSON, `create_new` temp, same-dir rename | 3.4 ms |
| pre-P0.3: pretty JSON written over the destination | 2.8 ms |

Multiplying through, per second of transfer:

| stage | updates/s | record-write cost | share of wall time |
| --- | ---: | ---: | ---: |
| raw blob progress | 1,625 | 5,530 ms/s | impossible |
| after P0.1 (10 Hz coalescer) | 10 | 34 ms/s | 3.4% |
| after P0.2 (1/s or 64 MiB) | ~1 | 3.4 ms/s | 0.34% |

So the attribution is:

- **P0.1 is the change that mattered.** It removes 99.4% of downstream updates.
  At the measured rate a per-update checkpoint would need 5.5 s of record writing
  per second of transfer — the pre-P0.1 pipeline could not have kept up at all.
- **P0.2 is real but second-order given P0.1**: it takes 3.4% of wall time down
  to 0.34%. Worth keeping, and worth more on mobile storage than this desktop
  figure suggests, but it must not be credited with the coalescer's win.
- **P0.3 is effectively free.** Atomic replacement adds ~0.7 ms per save, which
  at one save per second is 0.07% of wall time. There is no performance argument
  for dropping it, which was the point of measuring it separately.

Caveats, so this is not read as more than it is: unit-cost times rate is not the
end-to-end A/B the table above specifies, and it cannot capture scheduler effects
(a blocking write on a Tokio worker costs more than its own duration). The
figures are desktop Windows on NVMe with a 200-item manifest; a single-file
record is smaller and cheaper, and Android storage is slower. P0.4 (`z` vs `3`)
is still unmeasured and needs the aarch64 comparison, not this.

## 7. Phase C — Reproducible benchmarks

### C1. Desktop A/B first

Prefer desktop-to-desktop with Linux `netem`; on Windows use an equivalent
impairment tool such as clumsy when needed. Desktop allows many repetitions,
controlled RTT and loss, and avoids thermal and mobile flash noise.

Minimum matrix:

| Scenario | RTT | Loss | Rate cap | Purpose |
|---|---:|---:|---:|---|
| Direct LAN | 2-5 ms | 0% | 1 Gbit/s or the real link | CPU/store ceiling |
| Simulated direct WAN | 50/100/200 ms | 0% | 100 Mbit/s | flow-control / RTT scaling |
| Loss sweep | 50/100 ms | 0.1% and 1% | 100 Mbit/s | CC/recovery stability |
| Real or self-hosted relay | measured | measured | real relay | relay/path overhead |

- A 1-2 GiB random file is the default; size it so each run measures at least
  30-60 seconds. Larger files only when the link is very fast, or for soaks.
- Keep warm-cache network benchmarks separate from cold-cache end-to-end
  benchmarks; never mix the two in one statistic.
- Each variant gets at least 1 warm-up and 10 measured runs.
- Interleave or randomize A/B order; never run all of A then all of B.

### C2. Mobile only confirms the winning configuration

- Android-to-Android, desktop-to-Android, and iOS-to-desktop/iOS as the lab
  allows.
- Confirm only the 1-2 configurations that won on desktop, with at least 5
  measured runs per device class.
- Wait for temperature and clocks to return to a predefined band before the next
  run; record thermal state over time, not just at the start and end.
- Run separate 10-20 minute or large-file soaks to catch thermal throttling,
  linear memory growth and background restrictions. Do not use a soak as a routine
  A/B run.

### C3. Wi-Fi 4/5/6 stratification

Benchmarks must account for the Wi-Fi generation, but the Wi-Fi 4/5/6 label and
the PHY rate are not a throughput baseline. Generation, band and channel width are
**strata**; the denominator is an iperf3 measurement taken on the same device
pair, in the same direction, at the same time.

With mesh, the same SSID and the same subnet are not enough to call it the same
environment. `same-node`, `cross-node wired-backhaul` and
`cross-node wireless-backhaul` are three different strata. The primary optimization
runs must pin the same AP node/BSSID and band if the lab allows; cross-node mesh is
a separate robustness track.

Minimum lab matrix, running only rows the hardware and AP actually support:

| Class | Standard | Band | Target channel width | Role |
|---|---|---:|---:|---|
| Wi-Fi 4 / 2.4 | 802.11n | 2.4 GHz | 20 MHz; 40 MHz as its own stratum | Common, interference-prone environment |
| Wi-Fi 4 / 5 | 802.11n | 5 GHz | 40 MHz | Separates band effects from generation |
| Wi-Fi 5 | 802.11ac | 5 GHz | 80 MHz; 160 MHz separately if available | Modern mobile/desktop baseline |
| Wi-Fi 6 | 802.11ax | 5 GHz | 80 MHz | OFDMA/ax, without assuming a single flow is faster |
| Wi-Fi 6E | 802.11ax | 6 GHz | 80/160 MHz, each width its own stratum | Optional when the lab has 6 GHz AP and client |

Record per run, where the OS allows:

- 802.11 standard, band/frequency, channel width, negotiated TX/RX PHY, RSSI,
  NSS/MCS, retry and channel utilization; any field that cannot be read must be
  `unavailable`, never guessed.
- SSID and BSSID of both endpoints, AP/mesh node ID, backhaul type, client
  isolation and multicast filtering, and the Windows network profile/firewall. Do
  not pool runs with different BSSIDs or nodes; an unreadable BSSID must be
  recorded as `unavailable`.
- AP model and firmware, client model and OS, fixed distance and position, power
  source, thermal status and temperature over time.
- The selected Wisp path, direct/relay ratio, transfer direction and each device's
  role.

Topology preflight before every batch:

1. Confirm IP/subnet, SSID/BSSID/node and band of both endpoints; a run is invalid
   if the BSSID or node roams mid-transfer, unless that is the path-migration test
   itself.
2. Check bidirectional unicast and mDNS/broadcast discovery. mDNS failing while
   unicast passes must be flagged `multicast_blocked` and not blamed on the
   transfer core.
3. Bidirectional ping is only a queue/asymmetry warning, never a throughput
   baseline. When RTT or loss is clearly asymmetric, confirm with bidirectional
   iperf3 or a raw test, and do not run QUIC tuning A/Bs until the topology is
   stable.
4. Confirm the Windows network profile/firewall and the AP's client isolation and
   multicast snooping against the lab configuration; never disable a production
   security control to paper over a failure.

Baseline protocol per direction:

1. Run single-stream iperf3 TCP in the same direction as Wisp immediately before
   the batch; optionally add a 4-stream run to see how far the link ceiling differs
   from the single-flow ceiling.
2. Repeat immediately after the batch. Use the median of before and after as
   `link_baseline`; if the two differ by more than 10%, treat the environment as
   changed and rerun the batch.
3. Compute `link_utilization = app payload throughput / iperf3 throughput`. PHY is
   metadata and an upper bound only; never use `payload / PHY` to pass or fail.
4. Do not pool results across generation, band or channel width. Compare p10/p50,
   CV, stalls, direct ratio and utilization within each stratum.

The current Pixel 4 smoke is an observation, **not a baseline**: Wi-Fi 4 /
2.4 GHz, 2442 MHz, RSSI around -35 to -41 dBm, PHY TX varying about 52-117 Mbps
and RX about 130-173 Mbps, thermal status 0; battery around 37.9-38.7 °C at the end
of the runs. Host-side PHY was unavailable due to Windows permissions and is
recorded as `unavailable`. It needs repeating with release builds at both ends, a
representative file, an iperf3 bracket and at least 5 measured runs within one
Wi-Fi stratum.

A newer schema-v5 smoke recorded the Pixel on BSSID `c2:49:43:1f:a6:77`, Wi-Fi 4 /
2.4 GHz 2442 MHz, RSSI -44 dBm, PHY TX/RX 52/117 Mbps. The laptop used an AX211 on
the Windows profile `TienNA 5G 2`, category `Public`; the Pixel used SSID
`TienNA`, so the two endpoints were on different SSIDs and bands despite sharing
`192.168.1.0/24`. Host BSSID and PHY were unreadable because of the Windows
location permission. During the transfer, ping from Pixel to laptop was only
3.1-3.5 ms while laptop to Pixel reached 119-209 ms; mDNS did not find the peer,
Windows → Android dropped the control handshake twice, and Android → Windows went
direct. This batch is tagged `cross_band_suspect_mesh_or_ap_asymmetry` and is
excluded from any window or CC conclusion.

A controlled retry moved the Pixel to SSID `TienNA 5G`, BSSID
`c2:49:43:3f:a6:78`, Wi-Fi 5 / 5805 MHz, RSSI -44 dBm, negotiated TX/RX
866/780 Mbps and IP `192.168.1.83`. The laptop stayed on the `TienNA 5G 2`
profile; a Windows profile suffix does not prove a different SSID or BSSID, and the
host BSSID still has to be recorded as `unavailable`. Bidirectional ping lost no
packets; after warm-up, laptop → Pixel fell from 178 ms to 2 ms and Pixel → laptop
from 21 ms to about 3 ms. Both directions of a 256 MiB random Wisp transfer went
direct at roughly 19.84-20.89 MiB/s average with no stalls and matching hashes.
This is a controlled topology smoke confirming the cross-band/mesh hypothesis, but
it does not replace an iperf3 before/after bracket or a matrix of at least 5
measured runs.

The next raw bracket for the desktop → Android direction kept its before/after
medians within 4.0%, yet the Wisp run in between had a direct ratio of only 5.0%
and a relay ratio of 95.0%, despite unchanged BSSID and band and a final RSSI of
-47 dBm. So the old Wi-Fi topology explains the initial very slow batch, but "same
5 GHz" is not enough to guarantee Wisp keeps a direct path. Stratify further by
selected path and exclude relay runs from direct-LAN transport tuning A/Bs.

mDNS/broadcast still fails in both directions on the new topology: desktop
`--nearby` finds 0 receivers, and Android also reports no nearby devices while a
desktop receiver is alive, while the short-code unicast handshake and transfer
succeed. Flag the batch `multicast_blocked`; investigate the Windows `Public`
firewall and mesh multicast forwarding separately, without mixing it into
direct-transfer throughput.

### C4. Separate workloads

- **One large file:** MB/s, utilization, p10/p50/p90, CV and stalls.
- **Many small files:** for example 1,000 files in 4/64/1,024 KiB buckets;
  measure files/s, total completion time and round trips. Never judge this
  scenario by MB/s alone.
- **Prepare/SAF:** time-to-ticket and effective source-read MB/s.
- **Finalize/export:** time-to-file-ready and effective export/write MB/s.
- **Path establishment:** direct-success rate, time-to-direct and relay byte
  ratio.

## 8. Phase D — Experiment order once there is data

### D1. Direct-path reliability first

If `relay_bytes_ratio` is high or `never_direct` happens often, prioritize
discovery, hole punching, address freshness and path migration. A transfer on the
wrong path can be slower than every micro-tuning put together.

A 2026-08-14 smoke on the same subnet showed Windows browse not seeing
mDNS/broadcast, while a WSPD unicast packet sent directly to the Pixel got a valid
reply and a code-based transfer went direct. The receiver/responder was alive; the
branch to investigate is multicast/broadcast, the Windows firewall and AP
isolation, and how to obtain a peer address safely for targeted unicast. Do not
blind-scan an entire `/24` in production.

**Resolved — it was three bugs, none of them the network.** Neither the firewall
nor AP isolation was involved: a raw socket on the desktop heard 19 `_wisp`
packets from the phone in 30 s, including full announcements, at the same time
the app's browse reported zero.

1. **`mdns-sd` ignores unsolicited announcements** (`accept_unsolicited` defaults
   to false), so it discarded every one of those packets. Enabling it took
   desktop-finds-phone from never to 0.2 s.
2. **The phone could not receive multicast at all.** Android filters inbound
   multicast unless an app holds a `WifiManager.MulticastLock`; sending is
   unaffected, which is exactly why the phone appeared to be advertising
   correctly while hearing nothing. `CHANGE_WIFI_MULTICAST_STATE` had been in
   the manifest all along with nothing taking the lock. This is also why the
   phone never answered the desktop's queries, which is what made bug 1 fatal
   rather than cosmetic.
3. **The CLI receiver never advertised.** `ReceiverService` starts with
   discovery off and only the Flutter app ever called `set_discoverable`, while
   the sender's own error message tells the user to run `wisp receive` on the
   other machine.

After the three fixes, verified in both directions: the desktop lists the phone,
the phone lists the desktop, and a 273 MB transfer completed over discovery
alone with no short code. The lesson worth keeping is the diagnostic order — a
raw socket next to the application's own browse separated "the packets are not
arriving" from "we are throwing them away" in one step, and the answer was the
latter.

An early schema v5 smoke showed Windows → Android timing out on
`control_handshake`/`LastOpenPath` while Android → Windows completed direct. After
the Pixel moved from Wi-Fi 4 / 2.4 GHz to `TienNA 5G`, the handshake took 24-37 ms
in both directions and two 256 MiB transfers reached roughly 20 MiB/s with no
stalls. Pinning band/node/BSSID and checking topology therefore still comes before
D2/E1/E2. mDNS still fails even though short-code direct passes, so
discovery/firewall is an independent D1 branch rather than evidence that the
transport core is slow.

A raw/Wisp/raw bracket isolated it further: raw TCP held about 29.58 MiB/s by
mean-of-medians, while Wisp migrated three times and then sent 95.0% of the payload
over relay, averaging 11.79 MiB/s with a 3,529 ms stall. That is the D1 decision:
find out why the direct path falls back to relay on the same BSSID before trying
parallel streams, receive windows, MTU or a congestion controller.

**Important update — relay is not all-or-nothing.** D1 previously looked only at
`relay_bytes_ratio`, i.e. application bytes attributed to the **selected** path.
Schema v6 measures wire bytes per path kind and gives a very different answer: a
desktop → Android run where the receiver reported `path=direct` throughout and
`relay_bytes_ratio` was 0 still put **25.7% of its bytes over a relay path**, while
the selected path carried only 42.6%. The direct and relay routes run **in
parallel**; one does not replace the other.

Consequence for D1: the question is no longer "did it reach direct" but "what
fraction actually went direct". Use `wire_relay_bytes_ratio` as the metric, not
`relay_bytes_ratio` — the old metric reported 0% for exactly the run that was 25.7%
relay.

### D2. Parallel child/stream experiment

This comes before AOA and general QUIC tuning because upstream issue #4286 already
points at a single-stream throughput risk.

**The premise has changed:** 3-8 paths were measured sending bytes concurrently, so
parallelism already exists at the path layer, unintentionally. Before adding
parallelism at the stream or child layer, it must be known whether the existing
parallelism helps or hurts: several paths with differing RTT and loss cause
reordering, and at the BAO layer reordering becomes head-of-line blocking — exactly
the "transport-active delivery gap" shape being hunted. The cheapest experiment is
to limit the number of paths and compare CV, delivery gaps and
`wire_relay_bytes_ratio`, run before any protocol change.

**The experiment cannot be run at iroh 0.97, and that is now measured rather than
assumed.** Two levers were tried and both fail:

1. *Transport config.* `max_concurrent_multipath_paths` and
   `set_max_remote_nat_traversal_addresses` reject any value below their
   recommended floors (13 and 12), logging a warning and keeping the default
   (`src/endpoint/quic.rs:467`, `:530`). The count can be raised, never lowered,
   and multipath cannot be turned off.
2. *Narrowing the dial.* `WISP_BENCH_SINGLE_PATH` offers a single direct address
   and no relay at the blob dial. Measured on loopback it took `path_count` from
   1/3/4 down to 1/3 — a narrower start, not control. The reason is in
   `src/socket/remote_map/remote_state.rs`: iroh tracks addresses per *remote*
   rather than per dial, and once a direct path comes up it deliberately reopens
   any relay addresses it holds ("we may have raced this with a relay address")
   and then triggers hole punching for more.

Point 2 also explains the 25.7% relay share in a direct transfer: keeping the
relay path alive alongside direct is intentional iroh behaviour, not a failure to
upgrade the path. The Wi-Fi A/B below measures what that costs.

The third lever — a benchmark endpoint at `RelayMode::Disabled`, which removes
relay paths at the source instead of asking iroh not to reopen them — **does
work, and has now been run.** `WISP_BENCH_NO_RELAY` binds both endpoints without
a relay; because such an endpoint never comes online, receiver registration
publishes a direct-addresses-only ticket instead of waiting for
`Endpoint::online()`.

**Result on loopback: no reliable difference.** Release build, 256 MiB payload,
one warm-up run then five alternating pairs:

| arm | p50 median | range | max paths | relay bytes | stalls |
| --- | --- | --- | --- | --- | --- |
| relay on | 49.6 MiB/s | 5.5-82.5 | 4 | ~16-20 KB/run | 0 |
| no relay | 73.6 MiB/s | 15.8-84.2 | 3 | 0 | 0 |

The medians differ but the ranges overlap almost completely, and the spread is
explained by warm-up, not by the arm: the last two pairs land at 67.6/82.5
(relay on) against 75.9/73.6 (no relay), with the arms swapping the win. A first
debug-build pair looked like a 5.7x win for no-relay and was cold-start noise —
worth recording as the trap this experiment sets.

Three things the run does settle:

- **The relay is not stealing payload.** It carried ~16-20 KB per transfer, all
  of it in the pre-hole-punch window. That matches iroh's design: relay defaults
  to `TransportBias::backup()`, i.e. QUIC `PathStatus::Backup`, and is only used
  when no primary transport is available (`src/socket/transports.rs:743`).
- **The receive window is not the ceiling here.** `stream_data_blocked` was 0 in
  all ten runs, in both directions.
- **Extra paths alone do not cost throughput.** The relay-on arm ran 4 paths to
  the no-relay arm's 3 and matched it once warm.

Loopback only rules out a large effect on a link where the direct path is so
fast the relay is never competitive. It is the testbed least likely to show
reordering costs, and it showed none.

#### The Wi-Fi A/B answers it: multipath striping is real and it costs the tail

Phone to desktop over Wi-Fi, 273 MB payload, `WISP_BENCH_NO_RELAY` on the
desktop receiver (enough on its own — the dialer can only use relay addresses
the receiver advertises), four alternating pairs, release build:

| metric | relay on | no relay |
| --- | --- | --- |
| p50 median | 18.6 MiB/s | 21.0 MiB/s |
| **p10 median** | **8.3 MiB/s** | **16.8 MiB/s** |
| p10/p50 median | 45% | 79% |
| CV median | 0.33 | 0.16 |
| relay share of wire bytes | 16.1% | 0.0% |
| stalls | 1 | 0 |
| **passes the Gate-1 stability criteria** | **0/4** | **4/4** |

Every pair pointed the same way on p10. Median throughput barely moves; what
moves is the tail, and the tail is what the acceptance criteria measure.

**The relay bytes are not a hole-punching startup cost.** Split each relay-on
run into deciles: relay share peaks at 43-68% *mid-transfer* in three of four
runs, and never falls to zero — the last decile still carries 6-7% over relay,
and one run (A2) rises monotonically to 19% in its final decile. iroh is
striping payload across the relay concurrently with a live, selected direct
path, for the whole transfer.

That contradicts iroh's own documentation, which says relay is a backup
transport "only used when no primary transport is available"
(`src/socket/transports.rs:743`, where relay is registered as
`TransportBias::backup()`). Observed behaviour and documented intent disagree,
and it is worth reporting upstream.

The mechanism fits the shape being hunted: the relay path has a much higher RTT,
so blocks arriving over it lag the direct path's; BAO verification only advances
on a contiguous verified prefix, so a late relay-carried block holds up reported
progress. That is the transport-active delivery gap, arriving on schedule.

**There is no shippable fix at iroh 0.97.** `RelayMode::Disabled` is not an
option in production — it removes the fallback that makes remote transfers work
at all. The only public knob, `transport_bias`, cannot help either:
`TransportBias::backup()` is `pub(crate)`, so an external caller can only build
a `primary()`-based bias, and moving relay from Backup to Primary-with-a-penalty
would raise its standing, not lower it. This becomes a strong argument for E4
and an upstream issue, not a local patch.

**D2 keeps its original scope** — the correction runs the other way. An earlier
revision of this section read the loopback null result as evidence that relay
striping was not real and re-scoped D2 toward D1. The Wi-Fi A/B shows that
reading was wrong, and wrong because loopback could not exhibit the effect.

- For many small files: A/B concurrency 1/2/4/8 with a bounded queue and a memory
  limit.
- For one large file: a separate 1-vs-2/4 stream spike, on a benchmark branch
  only, to test the single-stream ceiling; do not change the production protocol
  before there is a clear win.
- Evaluate files/s, throughput, CPU, memory, fairness, cancel/resume and hash
  verification.
- Only re-evaluate the connection-level `send_window` once there are several
  streams; with one stream `MAX_STREAM_DATA` is the tighter constraint and the "8x
  so serving does not bottleneck" reasoning has no basis.

### D3. Sender prepare and the mobile provider

- Move walk/metadata blocking I/O to the blocking pool if a profile confirms it
  blocks the runtime.
- Import and hash with bounded concurrency, starting at 2-4 on mobile and 4-8 on
  desktop.
- Profile SAF read/copy on Android and provider access on iOS before raising
  network concurrency.
- Keep `TryReference` and source consistency for the duration of serving.

For the current 64 MiB Android file, a SAF copy of 71-106 ms is roughly
604-901 MiB/s and total prepare is about 131-148 ms, far below the 46-60 seconds of
network fetch. SAF optimization is not a priority for this large-file path; reopen
D3 only when a many-small-files workload, a different provider, or iOS gives a
different result.

**Measured: the sending side's storage read costs nothing.** `quic_baseline
source --from-file` adds a real file read to the baseline's send loop, so the
difference against the memory source is the sending read in isolation. Five runs
of each from the phone over Wi-Fi:

| source | median | range | idle windows |
| --- | --- | --- | --- |
| memory | 24.4 MiB/s | 20.6-26.6 | 0 of 4 measured |
| file on `/sdcard` | 25.7 MiB/s | 23.0-28.1 | 0 of 5 |

The file source is not slower — the difference is inside run-to-run Wi-Fi
variation — and it never produces an idle window. At 25 MiB/s the read is two
orders of magnitude below the phone's ~1,100 MiB/s sequential storage read, so
this is the expected result rather than a surprising one. It does close the
question the `transport_idle` stall opened: **storage on the sending side is not
what stalls a transfer**, which is what sent that investigation to D1 instead.

### D4. Finalize and export

- Optimize only when time-to-file-ready or an export baseline shows a bottleneck.
- Keep atomic records and conflict/path validation.
- Separate protocol-completed from user-visible file-ready in telemetry and UI.

### D5. AOA copy and GC

Only once a USB/GC profile confirms it:

- A reusable batch buffer instead of `toByteArray`/`copyOf`.
- Double buffering or a bounded ownership pool.
- A ring/compacting reassembler instead of `buffer + chunk`/`copyOfRange`.
- A/B 16/32/64 KiB per controller; do not raise it everywhere at once.
- Never retry a partial write with the whole batch, and always keep memory
  bounded.
- Maintain `7900 + IPv4/UDP overhead <= TUN MTU 8000`.

### D6. UI heartbeat, without hiding real stalls

The coalescer only sets a 10 Hz ceiling; it produces no update when the transfer
stops. If the UI holds a stale rate or stutters:

- The consumer/UI uses a heartbeat over the latest byte counter so speed and ETA
  decay to 0 when no new bytes arrive.
- The heartbeat writes no record and sends no additional bulk progress frame.
- Real stalls are still displayed and recorded; never use smoothing to hide a
  pipeline pause.

## 9. Phase E — QUIC tuning only with evidence

### E1. Window

Current configuration:

- Android stream receive window: 8 MiB.
- Desktop: 16 MiB.
- Connection send window: 8x the stream window.

These numbers must be read per endpoint: in an Android **sender** → desktop
receiver run, the 8 MiB is a local receive window that does not govern the bulk;
the desktop receiver advertising 16 MiB is the `MAX_STREAM_DATA` that matters.
`send_window` is a sender-side memory cap for when the peer grants a lot of
flow-control credit, not evidence that that many bytes are on the Wi-Fi.

At LAN/AOA RTTs of about 2-5 ms, 8 MiB gives a theoretical flow-control ceiling of
about 1.6-4 GiB/s. Treat the window as **mathematically ruled out** on those paths
unless telemetry shows a different actual configuration or the BDP ratio
contradicts it; do not spend an 8/16/32 MiB matrix on LAN/AOA.

Relay must not be compared against the phantom `window / RTT` target if the relay
server or its rate limit is lower. Only try 16/32 MiB on Android when all of the
following hold:

- `bdp_window_ratio` is regularly near 1;
- the raw QUIC/relay baseline still has headroom;
- the sender/provider is not CPU- or disk-bound;
- the memory budget for the larger window has been measured.

**E1b — reduce the window / in-flight cap when RTT inflation is high:** the current
direct runs show provider RTT p50 around 1.67 s, p90 4.16 s and cwnd p50 around
4.87 MB on a link doing only 1.1-1.6 MiB/s. This is a queue/bufferbloat
hypothesis, not yet a cause, because the fresh raw TCP runs also stutter. Once the
raw baseline is stable, A/B receiver windows of 4/8/16 MiB (and the matching
send-memory cap if needed) in randomized order. Accept a lower configuration only
when it reduces RTT inflation, loss and stalls without dropping utilization or
average throughput past the threshold.

### E2. Congestion control

- Keep CUBIC as the default until there are provider-side cwnd and loss figures.
- A/B CUBIC and BBR on the same iroh/noq version, the same impairment and in
  randomized order.
- Decide on utilization, p10/p50, CV, stalls and loss recovery, not on average
  alone.
- Do not auto-select a controller from the direct/relay label alone without
  per-device and per-path data.

Provider-side evidence now exists on the Android sender: one 64 MiB direct run
recorded 4,813 lost packets, about 6.99 MB of lost bytes, 71 congestion events and
a final RTT over 3 seconds. That is enough to prioritize recovery classification
and E1b once the raw bracket exists, but not enough to change a default: run
randomized CUBIC/BBR within one Wi-Fi stratum, at least 5 measured runs per cell,
and discard a batch when the before/after baseline differs by more than 10%. BBR
was enabled in commit `d386240` and reverted in `05c9e4d` because of stutter on
release phone-to-phone runs; that older data is qualitative only. Any retest must
use a benchmark-only override, must not change the production default, and must win
on p10, CV and delivery gaps as well as average.

### E3. MTU

- Keep default PMTUD on LAN.
- Only raise or tune it for AOA after MTU samples, probe loss and a USB profile
  show a benefit; check for fragmentation and controller-specific regressions.

### E4. Upgrading iroh

- Run a separate spike for a newer compatible iroh/iroh-blobs.
- Rerun baselines and benchmarks, since noq/noq-proto may change CC and multipath
  behaviour.
- Do not mix a dependency upgrade with an AOA rewrite or a parallel-stream
  protocol change.

## 10. Metrics and acceptance criteria

Every report must carry both absolute speed and stability:

- Link and transport utilization.
- p10, p50, p90 throughput over 1-second windows; do not record "median" and "p50"
  as two separate metrics.
- Coefficient of variation, and low-speed episodes below 10% of p50.
- Mid-transfer stall count/total/longest, with warm-up and finalize separated.
- Time-to-first-byte, time-to-direct, direct/relay byte ratio.
- Prepare, transfer, finalize and file-ready durations.
- CPU, RSS and thermal state, separately for sender and receiver.
- For many small files: files/s and completion latency.

Initial thresholds:

- p10 at least 70% of p50; 80% is the target once warm-up and phase boundaries are
  excluded.
- No mid-transfer stall over 500 ms on a stable LAN/AOA link.
- No linear memory growth during a soak.
- No regression in resume, cancel, reconnect, hash verification or path safety.
- Slow-but-steady throughput does not pass if utilization is low; every run must
  report its baseline ratio.

## 11. Decision gates

### Gate 1 — Trustworthy telemetry

- Correct receiver/provider semantics.
- Window and config are logged.
- Warm-up is not counted as a stall; `None` does not fake completion.
- The benchmark loop does not cancel `stream.next()` on a tick.
- Phase timing, path byte ratios and parser tests exist.
- Progress timestamps are free of sampler bias; a failed-terminal stall does not
  vanish.
- Active gaps are separated from loss recovery using provider stats; the
  correlation token is not a base-converted session ID; config source and endpoint
  semantics are not misleading.
- Network byte figures come from connection-scoped counters rather than path
  counters, and every report carries `path_counter_coverage` stating how much of
  the traffic the path counters covered.

Android in both sender and receiver roles has verified schema emission, receiver
samples, SAF/background-save phase joins and a JSON boundary with `0` malformed
lines. The round-2 code blockers were fixed in `bcbde90`, `f978003`, `26b4f7c` and
`b5820c5`. A direct → relay → direct → relay path-migration smoke confirmed the
discontinuity flag and lower-bound reporting.

The provider counter provenance blocker is **closed** (schema v6, see §5). The root
cause was `selected_path()` returning `None` or skipping non-selected paths; the
fix adds connection-scoped counters, `path_count` and `path_counter_coverage`. The
CLI-to-CLI smoke showed 83.5% coverage on a clean direct link with 4 paths, and the
real-device desktop → Pixel 4 release smoke reproduced the `path=unknown` /
counter-0 case at 0.0% coverage with 6 paths — in both cases the connection
counters recorded the byte totals correctly, 2,322 bytes apart across 128 MiB.

Both directions are verified under schema v6 on Android release: desktop → Android
(provider coverage 0.0%) and Android → desktop (provider coverage 11.1%, with
`path_counter_discontinuity_count=1` on the receiver, so the migration flag works
alongside coverage). The round-trip hash matched byte for byte.

**Gate 1 is closed for desktop and Android.** iOS is tracked as a separate item to
be done later and does not block Gate 2 or Phase C/D on the two accepted
platforms. When iOS is picked up, the scope is: the file-provider boundary,
background save through to the file genuinely being ready, and one bidirectional
schema v6 run to compare against the tables in §5. Until then, every conclusion in
this document applies to desktop and Android only.

A provider-only smoke is not sufficient to accept p10/p50/CV/stall.

### Gate 2 — Baselines and attribution complete

- iperf3, raw QUIC, disk and hash baselines exist.
- Historical H and A/B builds A-E for P0 exist, with at least 10 measured runs on
  desktop.
- It is known which change produced a win, and its effect size.

**Status: partly met, on one path.** For phone-to-desktop Wi-Fi:

| criterion | state |
| --- | --- |
| link baseline | done — 27.7 MiB/s TCP, five runs (B1) |
| disk and hash baselines | done — desktop 601 MiB/s write, 2,879 MiB/s hash (B1) |
| raw QUIC transport baseline | done — 24.4 MiB/s median, ten runs from the phone (B1) |
| historical H and builds A-E | **not run** |
| which change won, and by how much | attributed by unit cost x measured event rate (B2), not by the build matrix |

Only the build matrix is outstanding, and it is deferrable: the
unit-cost-times-rate work already separates P0.1 from P0.2 from P0.3 and gives
effect sizes, so the matrix would refine numbers rather than change the ranking.
It is also now bounded from above — everything the matrix varies lives in the 6%
between app throughput and the raw transport, so no build in it can be worth
more than that on this path.

Gate 3's routing question is answered for this path, and by two measurements
rather than one:

- **Not single-stream-bound, and not bound by anything above the transport.**
  `transport_utilization` is 94%; the transport is 88% of raw TCP. D2's
  stream-count experiments and D4's export work have at most 6% to win here.
- **Relay striping is the one large effect**: 16% median of wire bytes, and
  removing it doubles p10. So **D1 and the upstream iroh issue come first**.

D3 is not second: the sending side's storage read was measured against the
baseline and costs nothing (see D3), and the one stall that looked like a sender
problem turned out to be six seconds of blob-connection path setup, which is
also D1. Nothing currently routes to D3 on this path.

### Gate 3 — Choosing the right optimization branch

- High relay ratio ⇒ D1.
- Small-file RTT-bound or single-stream-bound ⇒ D2.
- Prepare/SAF-bound ⇒ D3.
- Export-bound ⇒ D4.
- AOA CPU/GC-bound ⇒ D5.
- Window/CC-bound with provider stats ⇒ Phase E.

Tuning from a later phase does not merge until its gate is met.

## 12. Check commands

```powershell
cargo fmt --all -- --check
cargo test --workspace --exclude wisp-web-receiver
cargo check --workspace --exclude wisp-web-receiver
cargo check -p wisp-web-receiver --target wasm32-unknown-unknown
cargo metadata --manifest-path flutter/rust/Cargo.toml --no-deps --format-version 1

Push-Location flutter/rust
cargo fmt --all -- --check
Pop-Location

Push-Location flutter
flutter analyze
flutter test
Pop-Location

python -B -m unittest tools/test_analyze_transfer_telemetry.py
git diff --check
```

Tests needing a real socket, relay or device belong in a separate suite and must
say plainly when they are skipped. Benchmarks always record the commit, release
profile, device/OS, CPU governor and thermal state, filesystem, Wi-Fi/USB, selected
path, impairment configuration, and the baseline from the same session.

## 13. What not to do yet

- Do not raise windows or MTU across the board without BDP and baseline data.
- Do not switch to BBR based on a benchmark from a different environment.
- Do not use receiver-local cwnd or loss to conclude anything about payload
  congestion.
- Do not treat UI smoothing as evidence that the pipeline is stable.
- Do not run a tens-of-hours mobile matrix before a desktop A/B has picked
  candidates.
- Do not drop hash verification, atomic records or path validation for throughput.
- Do not call a change a "successful optimization" just because functional tests
  are green.

## 14. References

- Iroh QUIC transport configuration: <https://docs.rs/iroh/latest/iroh/endpoint/struct.QuicTransportConfigBuilder.html>
- Iroh path statistics: <https://docs.rs/iroh/latest/iroh/endpoint/struct.PathStats.html>
- Tokio filesystem tuning: <https://docs.rs/tokio/latest/tokio/fs/>
- Cargo profile overrides: <https://doc.rust-lang.org/cargo/reference/profiles.html#overrides>
- Iroh single-stream throughput investigation: <https://github.com/n0-computer/iroh/issues/4286>
