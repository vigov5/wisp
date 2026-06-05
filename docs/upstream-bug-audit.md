# Upstream bug audit

Tracks investigations of bugs filed against the upstream `vsamarth/drift`
repo, checked against the current Wisp fork code.  This file is internal
— excluded from the published GitHub Pages site.

For each upstream issue, record:

- **Status in Wisp fork** — fixed / present / partially-fixed / N/A
- **Code path** with file:line refs
- **Residual risks** worth tracking even when "fixed"
- **Follow-up actions** (tests to add, upstream PRs to consider)

---

## drift#18 — Ensure both the receiver and sender can cancel a transfer

**Upstream:** <https://github.com/vsamarth/drift/issues/18> (open, 2026-04-12)
**Audited:** 2026-05-22

### Status in Wisp fork: **Fixed (no regression)**

Both directions propagate cancellation through the wire protocol.  The
upstream report's concern that "the transfer still going through even
if one side cancels" does not reproduce against the current code paths.

### Code trace

**Sender cancels mid-transfer** — `crates/core/src/transfer/sender.rs:298-312`
- `tokio::select!` between `do_transfer(...)` and `wait_for_cancel(...)`.
- On cancel: writes `SenderMessage::Cancel(by=Sender, phase=Transferring)`
  on `control_send`, returns `TransferOutcome::local_cancel(...)`.
- After session returns: `registration.shutdown().await` on iroh-blobs
  upload, so the underlying byte stream stops.
- Receiver side at `crates/core/src/transfer/receiver.rs:671-675` reads
  `SenderMessage::Cancel` on `control_recv`, calls `download.abort()`,
  returns `TransferOutcome::from_remote_cancel(...)`.

**Receiver cancels mid-transfer** — `crates/core/src/transfer/receiver.rs:685-693`
+ outer caller at lines 423-435
- Inside `do_transfer`'s `tokio::select!`: `wait_for_cancel(cancel_rx)`
  arm calls `download.abort()` and returns
  `TransferOutcome::local_cancel(Receiver, Transferring)`.
- Outer `run_session` checks `if let TransferOutcome::Cancelled(c) =
  &outcome` and calls `send_receiver_cancel(&mut control_send, ...,
  c.by, c.phase, c.reason)` — writes `ReceiverMessage::Cancel` on the
  control stream.
- Sender side at `crates/core/src/transfer/sender.rs:493-495` reads
  `ReceiverMessage::Cancel` in `do_transfer`'s control-stream select
  arm, returns `TransferOutcome::from_remote_cancel(...)`.

### History

- `cfa9a39` (vsamarth, 2026-04-04) **feat: add transfer cancellation**
  added the wire-level cancel machinery.
- `fe0c0a7`, `fe1c14c` — follow-up UI fixes.
- Issue #18 was filed 8 days after `cfa9a39` (2026-04-12).  Either the
  issue was filed before the fix landed in the reporter's checkout, or
  it covers a more subtle case the protocol-level fix handles.

### Residual risks (not bugs, worth tracking)

1. **Race window — wasted bytes between abort and Cancel arrival.**
   When the receiver aborts the blob download, the sender's iroh-blobs
   uploader keeps pushing bytes for up to one RTT until the Cancel
   message arrives and `registration.shutdown()` runs.  At 10 MB/s and
   50 ms RTT that's ~500 KB of wasted bandwidth per cancellation.  Not
   a correctness bug.
2. **Cancel message lost on connection drop.**  If the QUIC stream dies
   before the Cancel frame is flushed, the peer sees a generic
   `channel_closed` error instead of an attributed "Cancelled by peer"
   outcome.  The transfer still ends; the failure message is just
   less informative.
3. **No integration test covers mid-transfer cancel.**  Existing tests
   exercise decline-before-transfer flows (`crates/core/src/protocol/
   send.rs` line ~367, `protocol/receive.rs` line ~338).  Anyone
   refactoring `do_transfer`'s `tokio::select!` arms can break cancel
   propagation without CI catching it.

### Follow-up

- [ ] Add integration tests in `crates/core/` (or inline `#[cfg(test)]
      mod tests`) that:
  - Start a transfer between in-memory streams, deliver some progress,
    fire `cancel_tx.send(true)` on the sender side, assert the receiver
    outcome is `Cancelled(by=Sender, phase=Transferring)` and the blob
    download aborted.
  - Same test mirrored: receiver-initiated cancel propagates a
    `Cancelled(by=Receiver, ...)` outcome to the sender.
- [ ] Consider replying on the upstream issue with the code refs above
      once the integration tests land, so the issue can be closed (or
      reframed if the reporter has a different repro).

---

## drift#29 — Failed transfer events drop the file plan and progress context

**Upstream:** <https://github.com/vsamarth/drift/issues/29> (open, low severity)
**Audited:** 2026-05-22

### Status in Wisp fork: **Present on both sides**

Receiver and sender failure paths both discard plan / item count / total
size / bytes transferred / snapshot / connection path / remote ticket
context that was already known when the failure fired.  The Completed
and Cancelled arms preserve all of this — the Failed arms just need to
do the same.

### Receiver side — `crates/app/src/receiver/session.rs`

`failed_offer_event` (lines 638-664) hard-codes:

```rust
destination_label: String::new(),
item_count: 0,
total_size_bytes: 0,
bytes_received: 0,
plan: None,
snapshot: None,
connection_path: None,
sender_endpoint_id: None,
sender_ticket: None,
total_size_label: String::new(),
files: Vec::new(),
```

Call sites and the context available at each:

| Line | Trigger | Available context being discarded |
|------|---------|-----------------------------------|
| 116, 131 | `offer_rx` failed (before offer ever arrived) | None — defensive zeros are correct here. |
| 169 | `TransferPlan::try_new` rejected the offer items | `offer.file_count`, `offer.total_size`, `offer.items` — could be carried. |
| 351 | `CoreReceiverEvent::Failed` mid-stream | `plan`, `last_progress_bytes`, `current_path`, `remote_id_str`, `offer.file_count`, `offer.total_size` — all known, all dropped. |
| 452, 459 | `outcome_rx` returns Err / channel error | Same as 351.  Compare to the `Completed` arm at 393-404 and `Cancelled` arm at 426-450 which DO preserve everything. |

### Sender side — `crates/app/src/send/session.rs`

`failed_event_from_error` (lines 463-482) hard-codes:

```rust
item_count: 0,
total_size: 0,
bytes_sent: 0,
plan: None,
snapshot: None,
remote_device_type: None,
remote_endpoint_id: None,
remote_ticket: None,
connection_path: None,
```

Call sites:

| Line | Trigger | Available context being discarded |
|------|---------|-----------------------------------|
| 154 | `destination.resolve()` failed (no peer reached) | Nothing — defensive zeros are correct. |
| 293 | Run loop ended with `Err(error)` outcome | `current_plan`, `current_label`, latest snapshot (via `last_event` mutex) — all known. |

`map_sender_event`'s `CoreSenderEvent::Failed` arm (lines 577-595) is a
partial fix already: it preserves `preview.file_count`,
`preview.total_size`, and `prepared_plan`.  But:

- `bytes_sent` is hard-coded to `0` even when Failed fires mid-stream
  after `TransferProgress` events have advanced byte counts.  The
  session loop tracks `current_plan` (line 244) but not
  `current_snapshot`.
- `snapshot` is `None` for the same reason.

### Fix plan (when implementing)

**Receiver** (`receiver/session.rs`):

1. Replace `failed_offer_event` with a builder or struct that takes
   `Option<TransferPlan>`, `item_count: u64`, `total_size_bytes: u64`,
   `bytes_received: u64`, `connection_path: Option<ConnectionPath>`,
   `sender_endpoint_id: Option<String>`, `sender_ticket: Option<String>`,
   `files: Vec<ReceiverOfferFile>`, plus the existing label / sender /
   error params.
2. Lines 351, 452, 459 pass the in-scope values (`Some(plan.clone())`,
   `offer.file_count`, `offer.total_size`, `last_progress_bytes`,
   `final_path.clone()`, `Some(remote_id_str.clone())`,
   `sender_ticket.clone()`, `files` — same way the Cancelled arm at
   426-450 already does).
3. Line 169 passes `offer.file_count` and `offer.total_size` but
   `plan: None` since plan construction is what just failed.
4. Lines 116, 131 keep passing zeros / `None`.

**Sender** (`send/session.rs`):

1. Add `let mut current_snapshot: Option<TransferSnapshot> = None;`
   next to `current_plan` at line 244.
2. Update `map_sender_event` to set `*current_snapshot = Some(snapshot
   .clone())` on `CoreSenderEvent::TransferProgress` (analogous to how
   `current_plan` is set on `TransferStarted`).
3. Update `map_sender_event`'s `CoreSenderEvent::Failed` arm to fill
   `bytes_sent` and `snapshot` from the captured snapshot when present.
4. Expand `failed_event_from_error` to accept `current_plan`,
   `current_snapshot`, and the running counts; or refactor the call
   site at line 293 to construct the SendEvent directly with full
   context.
5. Line 154 keeps the defensive zero-value path.

### Test plan

- Receiver: feed a `Failed` core event after at least one
  `TransferProgress`, assert the emitted `ReceiverOfferEvent` carries
  the last known `plan`, `bytes_received`, `connection_path`,
  `item_count`, `total_size_bytes`.
- Sender: feed a `TransferProgress` followed by a Failed core event,
  assert the emitted `SendEvent` carries `plan`, `snapshot`,
  `bytes_sent` matching the last progress.

---

## Wisp-only: receiver never surfaces offer, sender stuck "Waiting" until cancelled → "Unknown sender"

**Reported:** 2026-05-23 (user observation)
**Audited:** 2026-05-23
**Reproduces on:** QR-paired AND 6-character-code pairing flows (i.e. all
SendDestination variants — not pairing-method-specific).

### Status in Wisp fork: **Reproducible, root cause uncertain — instrumentation needed**

User-visible flow (A receiver, B sender, both on same Wi-Fi):

1. A opens "Pair via QR" or shows the 6-char code on home.
2. B scans QR (or enters the code).
3. B taps Send → transitions to `SendPhase::WaitingForDecision`.
4. **A never shows the offer-confirm prompt.**
5. B taps Cancel.
6. A immediately transitions to `Failed` with `sender_name = ""`,
   rendered as "Unknown sender".

### Diagnosis from code reading

The "Unknown sender" label is decisive — `sender_name = ""` is only
produced by the two offer-receive failure paths in
`crates/app/src/receiver/session.rs` (the `Ok(Err(error))` and
`Err(error)` arms around lines 116 and 131).  Both fire when
`offer_rx.await` resolves with an `Err`, which only happens when
core's `run_session` returns `Err` **before** reaching `offer_tx.send(Ok(...))`
in `crates/core/src/transfer/receiver.rs`.

But for B to be in `WaitingForDecision`, B has already completed the
sender side of `run_handshake_on_streams`:

```text
send_hello → read_peer_hello → send_offer → emit WaitingForDecision
```

`read_peer_hello` succeeding means A wrote its Hello on the bi-stream's
send half, which means A's `do_handshake` got at least past the
`accept_bi` + `read SenderMessage::Hello` + `write ReceiverMessage::Hello`
steps (lines 584-611 of receiver.rs).  The remaining steps before
`offer_tx.send(Ok(...))` are:

1. `read_sender_message(Offer)` at line 612 — read Offer from B.
2. `TransferPlan::from_manifest(...)?` (~line 226)
3. `local_record_dir(...).map_err(TransferError::from)?` (~line 228)
4. `build_expected_files(...).await` (~line 234, has explicit
   `offer_tx.send(Err)` + Decline write on its own error path)
5. `build_expected_transfer_files(...)?` (~line 255)

Failures at 2 / 3 / 5 propagate synchronously via `?` and would show
the "Unknown sender" event **immediately**, not after B cancels.  The
user reports the failure only appears *after* the cancel, so A is
**blocked on an `.await`** that the connection close unblocks.

The most plausible single suspect is step 1 — A's
`read_sender_message(Offer)` blocking until B's cancel drops the QUIC
connection, at which point `conn.closed()` (line 630 of `do_handshake`)
returns `Err(connection_closed)`.  This matches the observed delay
exactly.

The mystery is: B's `send_offer.await?` returned Ok, so the Offer
bytes were handed to B's QUIC stack.  Why doesn't A see them on the
same bi-stream?

### Hypotheses to verify with instrumentation

- **H1: Stream mix-up.**  iroh-blobs's own internal bi-streams (it
  multiplexes onto the same `Connection` via its own ALPN through
  `BlobDispatcher`) somehow get returned by A's `accept_bi` before
  B's wisp control stream, and A reads Hello off the blobs stream by
  accident.  Implausible — accept_bi is per-ALPN-handler in iroh's
  Router, so blobs streams should never reach wisp_handler.  Worth
  confirming with logs anyway.
- **H2: Bi-stream is bidi-but-one-sided.**  In QUIC, opening a
  bidi-stream sends the STREAM frame only when the peer first writes
  *or* explicitly flushes.  If B's `send_offer` flushes the second
  message before the first one has been ACKed by A, and the LAN path
  migrates to relay mid-flight (or vice versa), the Offer frame may
  sit in the sender's pacer queue while the connection state
  stabilises.  Unlikely under iroh's keepalive but possible.
- **H3: Stream-finish semantics.**  Sender's send half is left open
  (no `finish()` until much later).  If A's read_frame implementation
  buffers until a full frame OR stream finish, and B's framing leaves
  the second message partially below the buffer threshold, A blocks.
  Unlikely — `protocol_wire::read_frame` reads length-prefix first
  then exactly that many bytes, no special EOF semantics.
- **H4: Connection migration race.**  iroh's NAT-traversal often
  starts the connection on relay then upgrades to a direct LAN path.
  If the upgrade hands off in the middle of stream state, the receiver
  side might lose a frame.  Iroh's quinn fork is supposed to handle
  this correctly, but a bug here would match.

### Recommended next step: instrumentation

Before guessing further, add `tracing::info!` at each critical step in
`crates/core/src/transfer/receiver.rs::do_handshake` and
`run_session` so a single repro produces logs that pinpoint the stuck
step:

- After `accept_bi` success: log `remote = %connection.remote_id()`
- After read Hello: log `sender_endpoint_id`, `session_id`
- Before write receiver Hello / after write
- Before read Offer / after read (so we see "before" without "after"
  exactly diagnoses H1-H4)
- After do_handshake returns Ok: log `phase = "post-handshake"`
- Before `offer_tx.send(Ok)`: log `phase = "offer-tx-send"`

Mirror on the sender side in `do_handshake` /
`run_handshake_on_streams`:

- After `open_bi`
- After each of `send_hello` / `read_peer_hello` / `send_offer`
- After `await_decision` resolution path (or cancel)

Once we have a single repro with these logs we can localise to either
H1 (wrong stream) or H2/H3/H4 (iroh-level state).

### Workarounds the user might consider in the meantime

1. **Restart the app** between attempts — if the runtime's
   `OfferState` somehow got stuck in `Pending` after a previous
   half-failed handshake, only the offending session's `OfferFinished`
   resets it to `Idle`, and that command may not have fired.  A
   restart guarantees a clean Idle state.
2. **Try LAN-only via fresh Wi-Fi handshake** — if the issue is path
   migration (H4), bouncing both devices off and back onto Wi-Fi may
   force iroh to settle on a single LAN path before the handshake.

### Follow-up

- [ ] Add the receiver-side + sender-side handshake tracing described
      above.
- [ ] Catch a repro with the new logs and identify the exact stuck
      step.
- [ ] Depending on which step, fix at iroh-blobs / wisp protocol /
      app actor layer.

---

## drift#29 — Failed transfer events drop the file plan and progress context

(see entry above — moved to keep chronological order)

### Follow-up

- [x] Implemented receiver fix in `crates/app/src/receiver/session.rs`:
      `failed_offer_event` now takes plan / item counts / bytes_received /
      connection_path / sender_endpoint_id / sender_ticket / files
      explicitly; all 5 in-stream + post-loop call sites pass the
      context they have; defensive `offer_rx` failure paths still use
      zeros / `None`.  Tracks `latest_snapshot` across the run loop so
      the in-stream `CoreReceiverEvent::Failed` arm can carry it.
- [x] Implemented sender fix in `crates/app/src/send/session.rs`:
      `failed_event_from_error` takes `&preview` + `Option<TransferPlan>`
      + `Option<TransferSnapshot>` and derives item_count / total_size /
      bytes_sent from them (plan-then-preview-then-zero fallback).
      `map_sender_event` got a `&mut Option<TransferSnapshot>` so the
      TransferProgress arm caches the latest snapshot for the Failed
      arm and the post-loop call site to use.
- [x] Added 2 regression tests:
      - `receiver::session::tests::failed_offer_event_preserves_plan_and_progress_context`
      - `send::session::tests::failed_event_preserves_plan_and_snapshot`
      Existing `failed_event_uses_structured_error` tests updated for
      the new signatures (sender + draft).
- [ ] Reply on upstream issue with the fix link once merged.

---

## Wisp-only: sender errors with "Protocol mismatch" while receiver completes

**Reported:** 2026-05-22 (user observation, no upstream issue filed yet)
**Audited:** 2026-05-22

### Status in Wisp fork: **Present**

User report: receiver UI reaches Completed, sender UI stays in "Sending"
for several seconds, then transitions to Failed with a message along
the lines of "the devices could not agree on how to complete the
transfer" (the user-facing label for `UserFacingErrorKind::Protocol-
Incompatible`).

### Hypothesis

The sender's `do_transfer` (`crates/core/src/transfer/sender.rs:414-506`)
sits in a `tokio::select!` polling `progress_recv` and `control_recv`
during the receiver's Phase-5 export window.  During export the
receiver sends **no application data** on either stream — the only
thing keeping the connection up is the QUIC-level keepalive configured
in `crates/app/src/quic_keepalive.rs`:

```
default_path_max_idle_timeout    = 6_000 ms
default_path_keep_alive_interval = 4_500 ms
```

These are tight (iroh caps at 6.5s / 5s).  Losing a single keepalive
ping on a flaky mobile / Wi-Fi link is enough to trip the 6s idle
timeout.  When that fires the QUIC path is torn down, both reads on
the sender's `progress_recv` and `control_recv` resolve with
`ProtocolError::FrameRead`, and the sender's `do_transfer`'s
`match msg?` at sender.rs:482 propagates the error.  The app-layer
mapping at `crates/app/src/error.rs:450-475` turns FrameRead into
`UserFacingErrorKind::ProtocolIncompatible`, which renders as the
"could not agree on how to complete the transfer" message.

On the receiver side the byte transfer already finished and the
export already wrote files to disk before the path went idle, so the
receiver's outer session reaches `await_final_sender_ack`, never sees
the Ack, logs a warning, calls `finish_control_stream`, and returns
`TransferOutcome::Completed`.  The receiver's UI therefore shows
Completed even though the sender errored.

This explains:

- The lag: it's the receiver's export window plus whatever extra time
  before the idle timer fires.
- The asymmetry: receiver UI Completed, sender UI Failed.
- The error label: it's `ProtocolIncompatible` because FrameRead /
  ChannelClosed errors share that bucket in `From<ProtocolError> for
  UserFacingError`.

The window is more likely to be hit when the export is slow — large
files, Android MediaStore writes to public Downloads, slow SD-card
storage.

### Fix plan

Both layers should change:

**A. Keep application traffic flowing during export (`crates/core/
src/transfer/receiver.rs`)**

Around lines 475-484, the `tokio::select!` that drives
`export_downloaded_collection` should also tick an interval (e.g.
every 2 seconds) and write a finalizing `TransferProgress` message on
`progress_send` each tick.  This gives the keepalive a backup —
real frames go on the wire, so a lost ping doesn't kill the
connection — and as a bonus the sender's UI can show "Finalizing on
receiver" with a live timer instead of looking frozen.

**B. Make sender's control_recv read tolerant after the success
signal (`crates/core/src/transfer/sender.rs`)**

Track `let mut seen_transfer_completed = false;` in `do_transfer`,
set it to `true` on the `Ok(TransferCompleted)` arm of the progress
read.  Change the control arm at line 481-498 from `match msg?` to a
match that, on `Err(_)` and `seen_transfer_completed`, sets
`control_done = true` and continues (the equivalent of "trust the
in-band signal — bytes finished, export confirmed by
TransferCompleted, lost the explicit Ok confirmation but transfer
was successful").  Errors before `seen_transfer_completed` still
propagate.

This is a belt-and-suspenders approach: fix A reduces how often the
race fires; fix B prevents user-visible failure when it still does
(e.g. the keepalive really did fail because the network is gone, but
we managed to receive the TransferCompleted frame before that).

### Test plan

- Receiver-side test: mock a slow export (sleep 8s inside the export
  future), assert that periodic `TransferProgress` frames are written
  during the wait.
- Sender-side test: drive `do_transfer` against an in-memory pair of
  streams, deliver TransferProgress + TransferCompleted on progress,
  then close (Err) `control_recv` before delivering TransferResult.
  Assert `do_transfer` returns `TransferOutcome::Completed`.
- Symmetric negative test: close `control_recv` BEFORE delivering
  TransferCompleted on progress, assert the sender still returns
  Err (don't silently swallow real errors).

### Follow-up

- [x] Implement A (receiver heartbeat) — extracted as
      `run_with_progress_heartbeat` helper in
      `crates/core/src/transfer/receiver.rs`, called from the
      Phase-5 export window with `EXPORT_HEARTBEAT_INTERVAL = 2 s`.
- [x] Implement B (sender lenient on control EOF after success) —
      `do_transfer` in `crates/core/src/transfer/sender.rs` now
      tracks `seen_transfer_completed` and treats subsequent
      control-stream read errors as `control_done = true` rather
      than propagating.
- [x] Add the 3 regression tests:
      - `transfer::receiver::tests::
         run_with_progress_heartbeat_emits_periodic_frames_during_slow_future`
      - `transfer::sender::tests::
         control_stream_close_after_transfer_completed_yields_completed`
      - `transfer::sender::tests::
         control_stream_close_before_transfer_completed_still_fails`
- [ ] Consider widening `default_path_max_idle_timeout` to the iroh
      max (6_500 ms) and `default_path_keep_alive_interval` to 5_000
      ms — gives slightly more headroom without changing semantics.
      Deferred — heartbeat backup makes this less urgent.
