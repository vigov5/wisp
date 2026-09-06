import 'package:app/features/send/application/source_rejections.dart';
import 'package:app/platform/native_source.dart';
import 'package:flutter_test/flutter_test.dart';

SourceRejection _noSpace(String name, int required, int available) {
  return SourceRejection(
    name: name,
    reason: SourceRejectionReason.noSpace,
    requiredBytes: BigInt.from(required),
    availableBytes: BigInt.from(available),
  );
}

void main() {
  test('a single refused file names itself and the space it needed', () {
    // The case this exists for: one 6.2 GB share into a phone that cannot
    // hold a copy of it.
    final message = describeSourceRejections([
      _noSpace('demo.rar', 6218218542, 5100000000),
    ]);

    expect(message, contains('demo.rar'));
    expect(message, contains('5.8 GB'));
    expect(message, contains('4.7 GB'));
  });

  test('several refused files are counted, and the tightest headroom wins', () {
    // Free space shrinks as each copy lands, so the smallest figure is the
    // one that explains the refusal — quoting the first would overstate it.
    final message = describeSourceRejections([
      _noSpace('one.bin', 1000, 900000),
      _noSpace('two.bin', 2000, 400000),
    ]);

    expect(message, contains('2 files'));
    expect(message, contains('2.9 KB'));
    expect(message, contains('391 KB'));
  });

  test('a provider that gave no size still produces a usable sentence', () {
    final message = describeSourceRejections([
      const SourceRejection(
        name: 'mystery.bin',
        reason: SourceRejectionReason.noSpace,
      ),
    ]);

    expect(message, 'Not enough space to add mystery.bin.');
  });

  test('unreadable reads differently from out of space', () {
    final message = describeSourceRejections([
      const SourceRejection(
        name: 'locked.bin',
        reason: SourceRejectionReason.unreadable,
      ),
    ]);

    expect(message, "Couldn't read locked.bin.");
  });

  test('a mix of reasons stays vague rather than claiming one of them', () {
    final message = describeSourceRejections([
      _noSpace('big.bin', 10, 1),
      const SourceRejection(
        name: 'locked.bin',
        reason: SourceRejectionReason.unreadable,
      ),
    ]);

    expect(message, "Couldn't add 2 files.");
  });

  test('nothing refused says nothing', () {
    expect(describeSourceRejections(const []), isEmpty);
  });
}
