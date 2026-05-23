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

### Follow-up

- [ ] Implement the fixes above
- [ ] Add the two regression tests
- [ ] Reply on upstream issue with the fix link once merged

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
