# Review of the file transfer performance plan

Subject: `docs/transfer-performance-plan.md`

Three review rounds:

- **Round 1 (2026-08-13, commit `afc652f`)** — sections 1-8 below. Checked against
  `crates/core/src/blobs/receive.rs`, `crates/core/src/blobs/telemetry.rs`,
  `crates/core/src/transfer/receiver.rs`, `crates/app/src/quic_keepalive.rs`,
  `Cargo.toml`, `flutter/rust/Cargo.toml`.
- **Round 2 (2026-08-14, commit `3b6aff0` + working tree)** — after the plan was
  rewritten and A1-A6 implemented.
- **Round 3 (2026-08-14, commit `b5820c5` + schema v6)** — after V1-V13 were
  addressed.

Rounds 1 and 2 were feedback only, with no code or plan changes made at the time.

# Round 1 — 2026-08-13

## 1. The biggest methodological problem: P0 shipped before P1 measured

The document itself says "P1: measure properly before tuning", yet P0 changed four
things and marked them "complete". Its "Verification" section (plan lines 95-100)
lists only `cargo test`, `rustfmt` and `git diff --check` — **not one throughput
number**. Consequences:

- Nobody knows what P0 achieved. The baseline is gone (commit `8a33818` is on
  main), so an A/B now requires a revert.
- The four fixes overlap, so none can be attributed. Specifically, the checkpoint
  throttle (P0-2) sits **downstream** of the coalescer (P0-1): once progress is
  coalesced to 10 Hz, record writes are already down to 10/s, and the 1s/64MiB
  throttle only takes 10 to 1. Shipping both together means it will never be
  known which one mattered.
- Hypothesis: nearly the whole win comes from no longer serializing and
  rewriting `record.json` every 16 KiB — one synchronous filesystem write per BAO
  leaf, roughly 6,400 writes/s — rather than from the channel or the progress
  frames. A single measurement would settle it, and the answer changes how P2/P3
  should be prioritized.

The document should say plainly that P0 is an *unverified bet*, not *done*.

## 2. Telemetry reads its numbers from the wrong end of the connection

This is the most serious technical error in the new code.

A transfer is a **pull**: the receiver opens the stream and data flows from sender
to receiver. But `NetworkSnapshot::capture`
(`crates/core/src/blobs/telemetry.rs:246`) reads `path.stats()` on the receiver,
and in quinn those fields describe **the local endpoint's own sending direction**:

- `cwnd` — the receiver's congestion window, i.e. the window for its ACK traffic.
  Nearly meaningless for the transfer.
- `lost_packets` / `lost_bytes` / `congestion_events` — loss among packets the
  **receiver sent**, not loss on the data flow.

Only `rtt`, `udp_rx.bytes` and `current_mtu` are usable. That means the two
numbers P3 relies on to decide window size and CUBIC-vs-BBR (cwnd and loss) are
being taken from the wrong branch. Real diagnosis needs **sender-side** stats, or
at minimum the document must state that cwnd/loss here do not describe the bulk
flow.

Related:

- **The configured window value is never logged.** All of P3 turns on the
  "window-bound" hypothesis, yet the log has no `stream_receive_window`. Samples
  already carry `app_bytes_per_sec` and `rtt_us`, so logging the window makes
  `app_bps × rtt / window` computable immediately; a ratio near 1 is evidence of
  being window-bound, with no sender-side stats required. This is the cheapest
  addition with the highest value.
- **`udp_rx_bytes_delta` vs `bytes_delta` is a free diagnostic going unused**:
  UDP rising while the application offset stands still means the bottleneck is
  *after* the network (store write / hash / disk), not QUIC. The document lists
  "blob store write wait" as extra work to do while this signal is already
  available.

## 3. The stall metric will raise false alarms

- `BlobTransferTelemetry::new` is called right after connect, before the first
  byte (`crates/core/src/blobs/receive.rs:288`), with `last_progress_at = now`.
  So all the time the sender spends opening its store, handshaking or lazily
  hashing counts as a stall. The acceptance criterion "no stall over 500 ms" will
  fail on the first run for an entirely benign reason. Warm-up has to be
  separated from mid-transfer stalls.
- `Some(Done(_)) | None` are handled together
  (`crates/core/src/blobs/receive.rs:100`), so a stream that ends **without
  `Done`** is still logged as `outcome = "complete"`. That is a transport failure
  — `crates/core/src/transfer/receiver.rs:1014` classifies it that way itself.
  The failure and stall statistics P1 intends to decide on are therefore
  untrustworthy. The conflation is pre-existing behaviour, but it now
  contaminates measurement data.
- Stall resolution is 250 ms because `detect_stall` only runs inside `sample()`.
  Acceptable against a 500 ms threshold, but `stall_total` is quantized; the
  document should say so, to avoid comparing differences finer than that.

## 4. Observer effect: measuring a different code path than production runs

With telemetry off the loop is untouched. With it on, `stream.next()` runs inside
a `select!` and is **cancelled every 250 ms**
(`crates/core/src/blobs/receive.rs:293`). The document presents this as
"preserving the original hot loop", but the other side of it is that every P1
benchmark runs a loop users never run.

Beyond that, the cancel-safety of the stream from
`store.remote().fetch(...).stream()` is not established anywhere — if it is not
cancel-safe, telemetry-on can lose or skew progress. This needs an explicit
statement or a test rather than being left implicit.

## 5. Several technical conclusions in the document do not hold up

### The window ceiling table (plan lines 179-183)

The arithmetic is right but it leads somewhere misleading:

- At LAN/Wi-Fi RTTs of 2-5 ms, 8 MiB gives a ceiling of 1.6-4 GB/s. The window
  **cannot** be the bottleneck on LAN/AOA. The entry "LAN/AOA: keep the current
  window" is correct, but it should say outright that this is *ruled out* and
  needs no benchmark.
- On relay, the real cap is the relay server's bandwidth and rate limits —
  usually a few MB/s to a few tens — not flow control. Chasing "168 MB/s @100 ms"
  is a phantom target. Trying a 32 MiB window on Android only buys more RAM and
  bufferbloat unless the relay is self-hosted.

### `send_window = 8 × stream window`

`crates/app/src/quic_keepalive.rs:61` — the reason given in the comment ("so the
serving side doesn't become the new bottleneck") does not match the current
architecture. A transfer runs on **one** stream, so `MAX_STREAM_DATA` is always
the tighter constraint; a connection-level `send_window` only matters with
several streams. Harmless, but the stated reason is wrong — and it would become
correct if P4 adds parallel children, which is when it should be written that
way.

### `opt-level = "z"` → CPU bottleneck (plan lines 47, 88-93)

The mechanism the document describes is doubtful:

- On aarch64, blake3's hot core is NEON compiled by `cc` through a build script,
  which **is not affected by rustc's opt-level**. If so, "z disables loop
  vectorization and slows BLAKE3" names the wrong cause; any real win comes from
  `ring`/AEAD and from iroh-blobs' own Rust code.
- The only number the document leans on (28 vs 747 MB/s) is **opt 0 vs 3**, not
  **z vs 3**. It does not extrapolate.
- `[profile.release.package."*"]` does apply to `wisp-core`/`wisp-app` — they are
  path dependencies, not members of the `flutter/rust` workspace — so that part
  is fine. But generic and `#[inline]` code from core/app is monomorphized
  **inside** `wisp_bridge`, i.e. compiled at `z`. The magnitude is unknown, which
  again calls for measurement rather than a claim of completion.

Cheapest route: an on-device blake3 + AEAD bench, z vs 3, before crediting this
fix.

## 6. Gaps that matter more than all of P2/P3

- **Time to reach a direct path, and the share of bytes over relay.** This is the
  real 10x variable in the field: a transfer that spends its whole life on relay
  because hole punching failed makes every window/CC tuning irrelevant.
  Telemetry already logs `path` per sample so it is derivable, but the plan does
  not treat it as a first-class metric — `time_to_direct_ms` and
  `relay_bytes_ratio` belong in the acceptance criteria.
- **No absolute baseline.** "p10 ≥ 70-80% of median" is a *smoothness* criterion:
  a uniformly slow transfer passes. Without iperf3 or a raw QUIC echo per path
  there is no way to know whether the link is being used at 30% or 90%. This is
  a serious gap in the benchmark matrix.
- **The sender side is empty.** For Android sends, the earlier conclusion was
  that the bottleneck is the SAF read during pick/copy — the plan has one line
  about import concurrency in P4 and no target for the prepare phase. All
  current telemetry is receiver-side.
- **Finalize/export.** The receiver only writes files out during the finalize
  phase, and on Android the save runs in the background after `completed`. So
  "time the user perceives" is not transfer time. The plan mentions measuring
  export but sets no threshold and adds no instrumentation.
- **Many small files.** Each HashSeq child is a round trip; 1,000 files over a
  relay at 200 ms RTT is about 200 s of pure RTT with nothing to do with
  bandwidth. This needs its own scenario measured in **files/s**, not MB/s — it
  is currently folded into P4.
- **iOS is absent from the matrix** even though the repo ships an IPA and iOS has
  its own network/background constraints.
- **iroh issue 4286 (single-stream throughput)** is listed as a reference and
  then ignored: P4 says "no striping before the numbers show the pipeline cannot
  keep one stream full", while the cited issue says the single stream is itself
  the limit. If upstream already knows that, parallel children is the cheapest
  experiment with the highest upside and should not be last.

## 7. The benchmark matrix as written cannot actually be run

3 paths × 2 devices × several variants × ≥5 repetitions × 4-8 GiB files means
tens of hours of manual transfers on phones, plus flash wear and 8-16 GiB free on
each machine. Beyond that:

- No cooldown between runs is required, so thermal throttling will mix into the
  results, and the document only records temperature at the start and end.
- A median of n=5 is far too weak to separate two configurations differing by
  10-15%.
- There is no harness: no benchmark command exists, and no script parses
  telemetry logs into p10/CoV. Without both, P1 will never happen.

Suggested reshape: run the primary A/B **desktop-to-desktop through netem or
clumsy** (simulated RTT and loss, reproducible, repeatable dozens of times) and
use phones only to *confirm* the one or two winning configurations; 1-2 GiB files
for A/B, with long soaks kept separate for thermal work.

## 8. Contradictions and nits

- "No unbounded channel for high-frequency progress" (plan line 218) — the code
  **still** uses `mpsc::unbounded_channel`
  (`crates/core/src/blobs/receive.rs:358`). Fine in practice because the
  coalescer caps it at the source, but the document should say "unbounded is
  kept; coalescing at the source is the mitigation" rather than list it as a rule
  being followed.
- The goal of "limiting burst/pause" versus the actual mechanism: the coalescer
  only sets a **ceiling** of 10 Hz, with no **floor**. During a stall there is no
  progress item, so nothing is emitted, so the UI freezes its number and
  speed/ETA never decay. A smooth UI needs a heartbeat tick on the consumer side
  — the plan has none.
- "Median throughput" and "p50 throughput" (plan lines 133-135) are the same
  thing.
- The check commands (plan lines 224-230) omit `cargo test --workspace` and the
  entire Flutter side (`flutter analyze`, `flutter test`).
- `noq 0.17.0` / `noq-proto 0.16.0` appear in the version lock without explaining
  their relationship to quinn/iroh, so a later reader will not understand why
  upgrading iroh can invert a CUBIC/BBR result.

## Round 1 summary

The document is strong on *discipline* — no blind window increases, no switching
to BBR on a hunch, hash and atomic record preserved — and the app-level telemetry
(bytes/s, stalls, udp_rx delta) points the right way. Three things to fix before
going further:

1. Downgrade P0 from "complete" to "unverified", and measure the
   `record.json`-per-16KiB cost to find where the win actually is.
2. Correct how telemetry is read: receiver-side cwnd/loss do not describe the
   bulk flow; add `stream_receive_window` to samples so the window-bound question
   answers itself; separate warm-up from stalls; stop reporting
   `outcome=complete` for a stream that ends without `Done`.
3. Add an absolute baseline (iperf3 / QUIC echo) plus time-to-direct and
   relay-ratio metrics, and move the benchmark matrix to netem on desktop —
   otherwise P1 will never run and P2/P3 will be bets again.

# Round 2 — 2026-08-14

Checked against the 2026-08-14 plan, commit `3b6aff0`, and uncommitted working
tree changes (`tools/analyze_transfer_telemetry.py`, `crates/server/src/lib.rs`,
`flutter/android/app/build.gradle.kts`).

## Handled well

A1-A4 have real code behind them, not just prose: the `local_*` prefix on
receiver-side stats, a separate provider sampler with cwnd/loss/UDP TX from the
right direction, `blob_config` logging window/CC/build profile,
`time_to_first_byte` separated from stalls, `None` → `Failed`, and a sampler task
replacing the `select!` around `stream.next()`
(`crates/core/src/blobs/receive.rs:377`). The analyzer has 1-second windows
weighted by `sample_ms`, role separation, and `measurement_valid`. All three main
round-1 recommendations (downgrade P0, absolute baseline, netem-first) are in the
plan.

## V1. `benchmark_run_id` is not anonymous

`crates/core/src/blobs/telemetry.rs:35`

`u64::from_str_radix(session_id, 16)` is a reversible one-to-one transform of the
session ID — only the base changes. The comment justifies a different goal
correctly (blocking log injection from peer-controlled strings), but the plan
calls it an "anonymous run ID", and the smoke check in §5 ("`0` matches for
path/peer/session ID") cannot detect this because it compares a hex string
against a decimal number. That check provides false assurance.

The repo already does this properly elsewhere: `crates/server/src/lib.rs` uses
`blake3::keyed_hash` with a per-process random key for `client_label`. The two
places are inconsistent. Either use the same technique, or drop the word
"anonymous" and say plainly that this is the session ID in decimal.

## V2. A3 trades away timestamp resolution, and the bias is one-sided

`crates/core/src/blobs/telemetry.rs:239`

`observe_progress` is now only called on the 250 ms tick, so `first_progress_at`
and `last_progress_at` are **tick times**, not byte-arrival times. Consequences:

- Stalls are **under-measured** by up to 250 ms — not a two-sided error. A real
  600 ms stall can measure 350 ms and never cross the 500 ms threshold. The
  effective detection floor is about 750 ms, meaning the "no mid-transfer stall
  over 500 ms" criterion in §10 does not measure what it claims.
- `time_to_first_byte_ms` gains up to 250 ms. For transfers shorter than 250 ms,
  TTFB approaches the whole duration.

A2 notes "quantization error of about one sample" but does not state the
direction of the bias or convert it into the real threshold.

Cheap fix that keeps A3's intent: add an `AtomicU64` holding nanos-since-start of
the most recent byte increase, written by the download loop alongside
`fetch_max`. One more atomic store, no await, no timer.

## V3. The provider logs a window that does not govern the transfer

`crates/core/src/blobs/telemetry.rs:390`

The receiver is the side that advertises `MAX_STREAM_DATA`, so the **receiver's**
`stream_receive_window` is what limits bulk. The provider record logs the
sender's own `stream_receive_window_bytes` under exactly that field name.

The analyzer only computes `bdp_window_ratio` from role=receiver, so the
arithmetic is right — but a human misreads it immediately: §5 of the plan quotes
"stream receive window 8 MiB" from an Android sender run while the receiver was
the desktop CLI at 16 MiB. That 8 MiB has nothing to do with the transfer.

Suggested: rename to `local_stream_receive_window_bytes` on role=provider (as was
done for `local_cwnd_bytes`); leave `send_window_bytes` alone, since that is the
field with meaning on the sender.

## V4. `known: true` for a hardcoded value

`crates/core/src/blobs/receive.rs:36`

`NOQ_DEFAULT_STREAM_RECEIVE_WINDOW_BYTES = 1_250_000` is a hand-copied upstream
default, with a comment saying "noq 0.16" while the plan's version lock is
`noq 0.17.0`. It is still marked `known: true` and logged as
`config_known=true`. This is precisely the failure mode A1 set out to eliminate:
a guessed number presented as a measured one. If noq changes its default,
telemetry will lie confidently.

Alongside it, this code records a real behaviour worth putting in the plan: the
AOA dial builds a fresh `QuicTransportConfig::builder()` and therefore **does not
inherit** the endpoint's 8/16 MiB window, falling back to 1.25 MB. Harmless at
sub-millisecond RTT, but E1 should say so rather than let a reader infer that AOA
also uses 8 MiB.

Suggested: replace `known: bool` with
`config_source = measured | assumed_upstream_default`.

## V5. A stall still active at failure gets renamed "finalization"

`crates/core/src/blobs/telemetry.rs:692`

`finish()` decrements `stall_count` and does not add to `stall_total` for an open
stall, folding the whole thing into `finalization_pause`. Correct for
`outcome=complete`. For `outcome=failed`, the open stall *is* the stall that
caused the failure — and it disappears from every stall metric. The analyzer then
sets `measurement_valid = outcome == "complete"`, so a failed run is barely
analyzable at all.

This should branch on outcome: complete ⇒ finalization pause; failed/cancelled ⇒
keep it as a stall.

## V6. `None` → `Failed` is a production behaviour change, not just telemetry

`crates/core/src/blobs/receive.rs:185`

A stream ending without `Done` previously reported `Completed`; now it reports
`Failed`. Semantically right and in line with A2, but if any path in iroh-blobs
ends the stream after completing without emitting `Done`, transfers that succeed
today will start failing. The smoke test covers one scenario. This needs a test
or a citation of the upstream API contract before being treated as safe.

## V7. The new `select!` has not been shown to be cancel-safe

`crates/core/src/blobs/telemetry.rs:223`

A3 sets its own rule: "if `select!` is kept, cancel-safety must be proven from
API documentation and an integration test". The sampler now has a `select!` with
`path_watcher.updated()`. `&mut oneshot::Receiver` is documented;
`Watcher::updated()` has no recorded evidence and no test. If it loses an update
when cancelled, `application_path` drifts and `relay_bytes_ratio` goes wrong. The
plan's own rule is not being applied to the new code.

## V8. Two smaller points

- Provider `udp_tx_bytes_total`/`lost_*_total` accumulate deltas, but
  `delta_from` returns 0 when the path changes, so totals under-count after a
  migration with no flag to say so
  (`crates/core/src/blobs/telemetry.rs:409`).
- `bdp_window_ratio` is computed on 250 ms samples while throughput p10/p50 uses
  1-second windows (`tools/analyze_transfer_telemetry.py:869`) — the two metrics
  do not share a statistical basis.

## V9. The Android sender data suggests the window is too large, not too small

The run in §5 of the plan: roughly 65 MB of payload, 4,813 lost packets at MTU
1452, i.e. about **10% loss**; final RTT 3.057 seconds on a **direct Wi-Fi**
path. A 3-second RTT on a LAN is the signature of bufferbloat and queue
overflow, not of a slow link.

The arithmetic: at 1.4 MiB/s with a 3 s RTT, BDP is about 4.2 MiB. The desktop
receiver advertises 16 MiB and the sender has a 64 MiB send window, permitting
roughly 4x BDP in flight. The sender fills the AP's queue, RTT inflates, CUBIC
sees loss, cwnd collapses, repeat.

But E1 asks only one question — whether to **increase** the window — and a
`bdp_window_ratio` around 0.26 will be read as "not window-bound" when the real
story is over-buffering.

Needed:

- **E1b:** try **reducing** the window / in-flight cap on paths with RTT
  inflation.
- `rtt_inflation = rtt_current / rtt_min` in samples. A direct and cheap signal;
  `rtt_min` is not logged today.

## V10. The most decision-relevant number is buried in a bullet

§5 of the plan: raw TCP in the same direction on the same link reached p10 1.298
/ median 1.591 MiB/s, **CV 0.186**. Wisp on the same link: p10 **0.0** / median
1.2 MiB/s, **CV 1.62**.

Same link, same direction: TCP is stable, Wisp is not. And roughly 89%
utilization on average means there is essentially **no throughput headroom left**
on that path. The conclusion follows immediately: what remains is purely a
stability problem, and it is not in the link.

This belongs as a headline conclusion of Phase A rather than a bullet, and it
should reorder priorities: investigate the delivery gap first, A/B CUBIC/BBR
second. E2 is currently promoted on the grounds that "provider-side evidence now
exists".

## V11. Stall classification is missing the class that matters most for this data

`tools/analyze_transfer_telemetry.py:663`

`_classify_stall_episodes` has only two classes,
`transport_active_delivery_gap` and `transport_idle_stall`. A4 instructs the
reader to interpret "UDP rising, app flat ⇒ suspect store/disk/verify".

On a run with 10% loss, all three gaps are almost certainly head-of-line blocking
from retransmission: `GetProgressItem::Progress(offset)` only advances once a
contiguous prefix is verified, so while a lost packet is awaited, UDP keeps
flowing while the offset stands still — looking exactly like a post-network
bottleneck. Following A4 into store/disk optimization would be entirely the wrong
direction.

A third class `transport_active_loss_recovery` is needed, identified from
provider `lost_packets_delta` in the same time window (already joinable by run
ID). The plan should also state that the first 7.7-second gap, which has **no**
sender loss, is the exception worth investigating; the other two are not.

## V12. Contradictory Gate 1 status

§5 says "Gate 1 is **not closed**". §11 says "this gate **is met** for Android
sender + desktop receiver direct". Both have hedging clauses after them, but the
two headline sentences contradict each other. One place should be the source of
truth and the other should point at it.

## V13. Outside the plan's scope: `client_ip` trusts `X-Forwarded-For` unconditionally

`crates/server/src/lib.rs` (uncommitted)

The code takes the rightmost parseable entry from XFF without checking whether
the peer socket is a trusted proxy. The comment states the assumption ("exactly
one trusted proxy") but does not enforce it. If anyone can reach the server port
directly — an exposed container port, or a deployment without Caddy — a client
setting its own XFF bypasses the rate limiter by rotating fake IPs, and
`client_label` in the logs becomes attacker-controlled. The test
`forwarded_for_injected_by_the_client_is_ignored` only demonstrates the case
where Caddy *does* append; it does not cover the no-proxy case.

Fix: read XFF only when `socket_ip` is in a trusted-proxy list (or the compose
network's subnet), and otherwise use the socket address.

## Round 2 summary

The plan is now methodologically sound and the code has A1-A4 mostly right. Three
items should be treated as blocking Gate 1:

1. **V2** — quantized timestamps make the 500 ms stall threshold unmeasurable.
2. **V11** — the missing loss-recovery class means A4 will point the wrong way
   for exactly the data already in hand.
3. **V9** — the over-buffering hypothesis must be added before Phase E goes
   looking only for larger windows.

Beyond those, **V1** and **V5** should be fixed before anyone bases a decision on
these logs.

# Round 3 — 2026-08-14

Checked against the 2026-08-14 plan after V1-V13 were addressed, commit
`b5820c5` plus the schema v6 changes.

## Status of round 2

All of V1-V13 were addressed: the token became a domain-separated BLAKE3
pseudonym, progress timestamps are published from the hot loop so the 250 ms
quantization is gone, a failed terminal keeps its active stall, provider windows
were renamed `local_*`, `config_source` replaced `known`, path migration raises a
discontinuity flag, `transport_active_loss_recovery` was added, E1b covers the
over-buffering hypothesis, and Gate 1 has a single source of truth in §11.

## V14. Root cause of "transport counters do not track the payload"

This was Gate 1's last blocker, and it has a cause identifiable in upstream
source rather than being a random device phenomenon.

`NetworkSnapshot::capture` read `PathStats` only through
`ConnectionInfo::selected_path()`. In iroh 0.97
(`src/endpoint/connection.rs:1252`) that function is
`paths().into_iter().find(|p| p.is_selected())`, and `is_selected` is set in
`src/socket/remote_map/remote_state/path_watcher.rs:111` by
`selected_path == p.remote_addr()` and **only while** the selected-path watcher
holds a value. Consequences:

- No path marked selected ⇒ `selected_path()` returns `None` ⇒ `path=unknown`,
  `stats_available=false`, every counter zero. This is exactly the Android
  provider's `udp_tx_bytes_total=0` symptom.
- Bytes carried on a non-selected path were never counted.
- Every migration made `delta_from` report a discontinuity and drop a whole
  sampling interval.

The fix adds **connection**-scoped counters from `ConnectionInfo::stats()`
(`udp_tx`/`udp_rx` plus frame stats). These are monotonic for the whole
connection and independent of path selection, making them the trustworthy byte
figures; the ratio of path counters to connection counters becomes
`path_counter_coverage`, the provenance check Gate 1 was missing.

A 64 MiB CLI-to-CLI smoke over loopback with `path=direct`, `0` discontinuities
and no migration:

| Counter | Provider | Receiver |
|---|---:|---:|
| Path-scoped | 57,591,003 | 57,590,345 |
| Connection-scoped | 68,938,270 | 68,932,812 |
| Coverage | 83.5% | 83.5% |

Payload was 67,108,864 bytes. The connection counters correspond to 2.7%
overhead, exactly right for QUIC over UDP. The path counters come out *below the
payload itself*, so they cannot be a correct UDP byte count.

The point for the plan: the problem is **broader** than the device report. It
does not only occur when `path=unknown` or when a migration happens — the path
counters fall about 16.5% short even in the cleanest case. Every path-scoped
loss/cwnd figure recorded in the plan must therefore be read as a lower bound
tied to that run's coverage, including runs previously considered clean.

### Mechanism of the shortfall

The first assumption was that the shortfall came from samples where no path was
selected. Per-sample data ruled that out: `path=direct`,
`path_stats_available=true` and `path_counter_discontinuity=false` on **every**
sample, yet the gap between the two counters appeared intermittently — some
samples had a gap of exactly zero, others fell short by as much as 2.6 MB.

Adding `path_count` to samples settled it: the connection held up to **4 paths**
at once (observed values 1, 3, 4 at both ends) at 85.0% coverage. Cross-checking
`noq-proto 0.16` (`src/connection/mod.rs:1120-1130`), `stats.udp_tx` and
`path_stats[path_id].udp_tx` are incremented at the same sites with the same
values, so the two series do not contradict each other — the path counter simply
describes **one** path among several in use.

A stronger conclusion than the original claim: the selected path's `PathStats` is
fundamentally not the denominator for "how many bytes were sent or received" on a
multipath connection. This is not an edge case to handle but the semantics of the
API. Reports therefore distinguish two diagnoses with the same symptom:
`path_count > 1` means multipath, while `path_count == 1` with low coverage means
some samples had no selected path.

## V15. `stream_data_blocked` answers the window-bound question directly

`ConnectionStats.frame_tx`/`frame_rx` (noq-proto 0.16,
`src/connection/stats.rs`) carry `stream_data_blocked` and `data_blocked`. A
`STREAM_DATA_BLOCKED` frame is only sent when the sender genuinely has data ready
and the receiver's `MAX_STREAM_DATA` is holding it back.

This is much stronger evidence than `bdp_window_ratio`: the ratio infers from
throughput and RTT, while the frame is a protocol event. The plan should use the
ratio to screen and the frame to conclude, instead of leaving E1/E1b dependent on
a single ratio.

In the smoke above the two agreed: `0` blocked frames and
`bdp_window_ratio_p90 = 0.0033`.

### Verified on real hardware

Two 128 MiB direct transfers between the desktop and a Pixel 4 release build
(Wi-Fi 192.168.1.x), with a byte-for-byte matching round-trip hash:

| | desktop → Android | Android → desktop |
|---|---:|---:|
| Provider `path` | `unknown` | `unknown` |
| `path_count` | 6 | 6 |
| Provider coverage | 0.0% | 11.1% |
| Provider avg via path | 0.00 MiB/s | 1.96 MiB/s |
| Provider avg via connection | 11.38 MiB/s | 17.66 MiB/s |

The point to press for the plan: in the Android → desktop direction, the same run
and the same counters yield two numbers differing by a factor of **9**. 1.96
MiB/s is what the old telemetry would report; 17.66 MiB/s is reality, and it
agrees with the receiver's 16.4 MiB/s app median. Had Gate 2 run on the old
numbers, the entire A/B campaign would have been measuring a phenomenon that does
not exist.

The desktop → Android direction is even more clear-cut: provider coverage 0.0%,
meaning schema v5 would have had no network figures to report at all.

The connection-level discrepancy also carries physical meaning that was
previously invisible: the sender sent 146.55 MB for a 134.22 MB payload while the
receiver got 138.29 MB — roughly 8.2 MB of retransmitted data, consistent with
the loss and congestion events the provider recorded plus one path discontinuity.

## V16. The payload is spread across several paths, and some of it takes the relay

Following up on "why does the selected path see only part of the traffic" led to
a finding absent from the plan's hypothesis table.

Aggregating counters across **every** path rather than the selected one gives:

- `active_path_count` = 3-8 paths **sending bytes at the same time**, not merely
  existing. 3-4 on loopback, up to 8 on device Wi-Fi.
- The selected path carries 42.6% of the wire bytes; the rest sits on other
  paths.
- **25.7% of the bytes went over a relay path** on a run where the receiver
  reported `path=direct` throughout and `relay_bytes_ratio` was 0.

That last point matters most for D1. The plan assumes relay is all-or-nothing:
either a direct path is reached, or the transfer falls back to relay. What is
measured is the two running **in parallel**, and the current
`relay_bytes_ratio` — application bytes attributed to the selected path —
reports 0% for exactly the run that put 25.7% over a relay. D1 needs to move to
`wire_relay_bytes_ratio`.

For D2 the premise changes entirely: parallelism already exists at the path
layer. Before adding it at the stream layer, the question is whether the
parallelism already present helps or hurts — paths with differing RTT reorder,
and at the BAO layer reordering is head-of-line blocking, which is exactly the
"transport-active delivery gap" shape the plan is chasing.

### Two traps in per-path aggregation

Both were caught by the data rather than by review, so they are worth recording:

1. **Not deduplicating by `PathId`.** A path appears once per transport address
   it is reachable at, and every entry carries the **same** counters. Summing the
   list as-is drove the total to 1.70x the connection total.
2. **Dropping entries for paths that left the list.** A path can leave and
   return; forgetting its counters credits its whole history to the interval it
   reappeared in. That skewed the total by 1.07x.

With both handled, the all-paths total matches the connection total to 1.0001x on
real hardware — close enough that path-scoped loss and cwnd stop being lower
bounds.

## V17. iroh 0.97 cannot reduce its path count, and narrowing the dial does not either

Testing whether multipath helps or hurts calls for limiting the number of paths.
Two levers were tried; both fail, and the second failure is the more informative
one.

**Transport config.** `max_concurrent_multipath_paths` and
`set_max_remote_nat_traversal_addresses` reject any value below their recommended
floors (13 and 12), logging a warning and keeping the default
(`src/endpoint/quic.rs:467`, `:530`). The count can be raised but never lowered,
and multipath cannot be turned off through the public API.

**Narrowing the dial.** The blob dial already filters addresses for the AOA cable,
so the obvious next move was to offer one direct address and no relay. Implemented
behind `WISP_BENCH_SINGLE_PATH` and measured on loopback, it took `path_count`
from 1/3/4 down to 1/3. A narrower start, not control.

The reason is in `src/socket/remote_map/remote_state.rs`: iroh tracks addresses per
*remote*, not per dial. When a direct path comes up it explicitly reopens any relay
addresses it holds — the comment there reads "we may have raced this with a relay
address" — and then calls `trigger_holepunching()` to look for more. The dial is
one input to that address set, not the boundary of it.

**Binding without a relay.** The third lever works. `WISP_BENCH_NO_RELAY` binds
both endpoints at `RelayMode::Disabled`, which removes relay addresses at the
source rather than asking iroh not to reopen them. It needs one companion change:
a no-relay endpoint never satisfies `Endpoint::online()`, so receiver registration
has to publish a direct-addresses-only ticket rather than wait to come online.

Run on loopback — release build, 256 MiB, warm-up plus five alternating pairs — it
found **no reliable difference**: p50 median 49.6 MiB/s with the relay against
73.6 MiB/s without, ranges 5.5-82.5 and 15.8-84.2, and the last two pairs
splitting the win. The spread tracks warm-up, not the arm. The first debug-build
pair had looked like a clean 5.7x win, which is a good reminder of how easily a
cold machine manufactures an effect.

Two further readings from the same ten runs: `stream_data_blocked` was 0 in every
one, so flow control was not the ceiling on this link; and the relay-on arm ran 4
paths against the no-relay arm's 3 while matching its throughput. Loopback is the
weakest possible testbed for reordering — paths there differ by microseconds, and
the relay never becomes competitive, carrying only ~16-20 KB per run.

**On Wi-Fi the same A/B says the opposite, and it is the one to believe.** Phone
to desktop, 273 MB, four alternating pairs, switch on the desktop receiver only
(enough on its own — a dialer can only use the relay addresses the receiver
advertises):

| metric | relay on | no relay |
| --- | --- | --- |
| p50 median | 18.6 MiB/s | 21.0 MiB/s |
| p10 median | 8.3 MiB/s | 16.8 MiB/s |
| p10/p50 median | 45% | 79% |
| CV median | 0.33 | 0.16 |
| relay share of wire bytes | 16.1% | 0.0% |
| passes the Gate-1 stability criteria | 0/4 | 4/4 |

Median throughput barely moves; the tail doubles, and every pair points the same
way. Splitting each relay-on run into deciles kills the "it is just hole-punching
latency" explanation: relay share peaks at 43-68% *mid-transfer* in three of four
runs and never reaches zero, with the final decile still at 6-7% and one run
climbing to 19% at the end. iroh stripes payload over the relay alongside a live,
selected direct path for the whole transfer — which contradicts its own
documentation of relay as a backup transport "only used when no primary transport
is available".

So **the 25.7% relay share in V16 is real striping**, and it costs the tail
rather than the median. The mechanism is the one this plan has been hunting: the
relay path's higher RTT delays blocks, BAO only advances on a contiguous verified
prefix, and the wait shows up as a transport-active delivery gap.

An earlier revision of this section drew the opposite conclusion from the
loopback null result and moved the finding from D2 to D1. That was wrong, and
wrong for an instructive reason — the testbed could not exhibit the effect being
tested for. D2 keeps its original scope.

No local fix exists at iroh 0.97: `RelayMode::Disabled` cannot ship because it
removes the fallback remote transfers depend on, and `transport_bias` cannot
lower the relay below Backup because `TransportBias::backup()` is `pub(crate)`.
This is an upstream issue and an argument for E4.
Failing that, the hypothesis can only be tested observationally — correlating
`active_path_count` and `relay_path_udp_bytes_delta` against throughput and
delivery gaps across many runs — which is weaker evidence and must be labelled as
such. Otherwise it waits for E4 and a newer iroh.

## Remaining for Gate 1

The iOS boundary, if iOS is in release scope. Both Android directions are
verified under schema v6, including a run that had both low coverage and a
discontinuity flag.
