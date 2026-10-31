import 'package:app/features/receive/application/resume_poll.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  final now = DateTime.utc(2026, 1, 1, 12);

  test('a confirmed code is not polled', () {
    expect(shouldPollOnResume(isStale: false, now: now), isFalse);
  });

  test('a stale code is polled on the first resume', () {
    expect(shouldPollOnResume(isStale: true, now: now), isTrue);
  });

  test('a second resume inside the interval is skipped', () {
    expect(
      shouldPollOnResume(
        isStale: true,
        now: now,
        lastPoll: now.subtract(resumePollInterval - const Duration(seconds: 1)),
      ),
      isFalse,
      reason: 'app switching must not stack polls on a dead network',
    );
  });

  test('a resume once the interval has elapsed is polled again', () {
    expect(
      shouldPollOnResume(
        isStale: true,
        now: now,
        lastPoll: now.subtract(resumePollInterval),
      ),
      isTrue,
    );
  });

  test('the interval gates even a stale code, never the other way round', () {
    // Not stale and long past the interval: still nothing to confirm.
    expect(
      shouldPollOnResume(
        isStale: false,
        now: now,
        lastPoll: now.subtract(const Duration(hours: 1)),
      ),
      isFalse,
    );
  });
}
