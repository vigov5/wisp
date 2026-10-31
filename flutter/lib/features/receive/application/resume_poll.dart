/// Minimum gap between resume-triggered registration polls.
///
/// Matches the receiver's own registration timeout, so rapid app switching
/// cannot queue a second poll behind one still waiting out a dead network.
const resumePollInterval = Duration(seconds: 30);

/// Whether returning to the foreground should ask the receiver to re-confirm
/// its pairing code.
///
/// Two rules, both load-bearing:
///
/// * Only when the code is already flagged stale. A confirmed code needs no
///   poll, and polling on every app switch is traffic nobody asked for.
/// * Not twice inside [resumePollInterval]. On a network where every attempt
///   hangs for the full timeout, an unthrottled trigger would stack them.
///
/// [lastPoll] is `null` before the first one.
bool shouldPollOnResume({
  required bool isStale,
  required DateTime now,
  DateTime? lastPoll,
  Duration minInterval = resumePollInterval,
}) {
  if (!isStale) return false;
  if (lastPoll == null) return true;
  return now.difference(lastPoll) >= minInterval;
}
