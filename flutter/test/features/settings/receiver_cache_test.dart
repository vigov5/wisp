import 'dart:io';

import 'package:app/features/settings/application/receiver_cache.dart';
import 'package:app/features/transfers/application/state.dart';
import 'package:flutter_test/flutter_test.dart';

String _join(String a, String b) => '$a${Platform.pathSeparator}$b';

/// The desktop shape: the root is the user's save folder, so only `.wisp/`
/// is ours.
ReceiverCacheLocation _wispOnly(String root) =>
    ReceiverCacheLocation(root: root, wholeRoot: false);

/// The Android shape: the root is our own staging folder, so all of it is.
ReceiverCacheLocation _wholeRoot(String root) =>
    ReceiverCacheLocation(root: root, wholeRoot: true);

void main() {
  late Directory root;

  setUp(() async {
    root = await Directory.systemTemp.createTemp('wisp_receiver_cache');
  });

  tearDown(() async {
    if (await root.exists()) {
      await root.delete(recursive: true);
    }
  });

  group('size', () {
    test('counts every file under .wisp, however deep', () async {
      final nested = Directory(_join(_join(root.path, '.wisp'), 'transfers'));
      await nested.create(recursive: true);
      await File(_join(nested.path, 'blob')).writeAsBytes(List.filled(2048, 7));
      await File(
        _join(_join(root.path, '.wisp'), 'record.json'),
      ).writeAsString('x' * 100);
      // Anything outside .wisp belongs to the user, not to us.
      await File(
        _join(root.path, 'holiday.jpg'),
      ).writeAsBytes(List.filled(5000, 1));

      expect(await receiverCacheSizeBytes(_wispOnly(root.path)), 2148);
    });

    test('counts the whole staging folder where that is the cache', () async {
      // Android stages received files in the root itself and only moves them
      // into MediaStore at the end, so a file sitting next to `.wisp` is ours
      // too — and it is the part that gets big.
      await Directory(_join(root.path, '.wisp')).create(recursive: true);
      await File(
        _join(_join(root.path, '.wisp'), 'record.json'),
      ).writeAsString('x' * 100);
      await File(
        _join(root.path, 'holiday.jpg'),
      ).writeAsBytes(List.filled(5000, 1));

      expect(await receiverCacheSizeBytes(_wholeRoot(root.path)), 5100);
      expect(await receiverCacheSizeBytes(_wispOnly(root.path)), 100);
    });

    test('a cache that was never created is zero, not an error', () async {
      expect(await receiverCacheSizeBytes(_wispOnly(root.path)), 0);
    });
  });

  group('delete', () {
    test('removes .wisp and leaves the save folder alone', () async {
      final cache = Directory(_join(root.path, '.wisp'));
      await cache.create(recursive: true);
      await File(_join(cache.path, 'blob')).writeAsBytes(List.filled(32, 0));
      final keep = File(_join(root.path, 'holiday.jpg'));
      await keep.writeAsBytes(List.filled(16, 1));

      await deleteReceiverCache(_wispOnly(root.path));

      expect(await cache.exists(), isFalse);
      expect(await keep.exists(), isTrue);
    });

    test('deleting a cache that is not there is not an error', () async {
      await deleteReceiverCache(_wispOnly(root.path));
      expect(await Directory(_join(root.path, '.wisp')).exists(), isFalse);
    });
  });

  group('root resolution', () {
    test('a SAF uri cannot be walked', () async {
      expect(
        await resolveReceiverCacheLocation('content://downloads/tree'),
        isNull,
      );
    });

    test('an empty root cannot be walked', () async {
      expect(await resolveReceiverCacheLocation('   '), isNull);
    });

    test('a plain path is used as given, trimmed', () async {
      final location = await resolveReceiverCacheLocation('  /srv/wisp ');
      expect(location?.root, '/srv/wisp');
      expect(location?.wholeRoot, isFalse);
    });
  });

  group('busy', () {
    test('idle and finished transfers leave the cache free to clear', () {
      expect(receiverCacheIsBusy(TransferSessionPhase.idle), isFalse);
      expect(receiverCacheIsBusy(TransferSessionPhase.cancelled), isFalse);
      expect(receiverCacheIsBusy(TransferSessionPhase.failed), isFalse);
    });

    test('a live transfer holds it, and so does the finished screen', () {
      // `completed` counts as busy: on Android the background save is still
      // moving files out of the cache while that screen is up.
      expect(receiverCacheIsBusy(TransferSessionPhase.connecting), isTrue);
      expect(receiverCacheIsBusy(TransferSessionPhase.offerPending), isTrue);
      expect(receiverCacheIsBusy(TransferSessionPhase.receiving), isTrue);
      expect(receiverCacheIsBusy(TransferSessionPhase.completed), isTrue);
    });
  });

  group('warning threshold', () {
    ReceiverCacheState sized(int bytes) => ReceiverCacheState(
      location: _wispOnly('/x'),
      sizeBytes: bytes,
      measured: true,
    );

    test('stays quiet until the cache is actually measured', () {
      expect(
        ReceiverCacheState(
          location: _wispOnly('/x'),
          sizeBytes: kReceiverCacheWarningBytes * 2,
        ).shouldWarn,
        isFalse,
      );
    });

    test('stays quiet just under the threshold', () {
      expect(sized(kReceiverCacheWarningBytes - 1).shouldWarn, isFalse);
    });

    test('warns at the threshold', () {
      expect(sized(kReceiverCacheWarningBytes).shouldWarn, isTrue);
    });

    test('a dismissed warning stays dismissed', () {
      expect(
        sized(
          kReceiverCacheWarningBytes * 3,
        ).copyWith(dismissed: true).shouldWarn,
        isFalse,
      );
    });

    test('an unknown size never warns', () {
      expect(const ReceiverCacheState(measured: true).shouldWarn, isFalse);
    });
  });

  group('formatting', () {
    test('rounds to something a person can read', () {
      expect(formatCacheBytes(512), '512 B');
      expect(formatCacheBytes(2048), '2.0 KB');
      expect(formatCacheBytes(kReceiverCacheWarningBytes), '500 MB');
      expect(formatCacheBytes(3 * 1024 * 1024 * 1024), '3.0 GB');
    });
  });
}
