import 'dart:io';

import 'package:app/features/settings/application/receiver_cache.dart';
import 'package:flutter_test/flutter_test.dart';

String _join(String a, String b) => '$a${Platform.pathSeparator}$b';

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

      expect(await receiverCacheSizeBytes(root.path), 2148);
    });

    test('a cache that was never created is zero, not an error', () async {
      expect(await receiverCacheSizeBytes(root.path), 0);
    });
  });

  group('delete', () {
    test('removes .wisp and leaves the save folder alone', () async {
      final cache = Directory(_join(root.path, '.wisp'));
      await cache.create(recursive: true);
      await File(_join(cache.path, 'blob')).writeAsBytes(List.filled(32, 0));
      final keep = File(_join(root.path, 'holiday.jpg'));
      await keep.writeAsBytes(List.filled(16, 1));

      await deleteReceiverCache(root.path);

      expect(await cache.exists(), isFalse);
      expect(await keep.exists(), isTrue);
    });

    test('deleting a cache that is not there is not an error', () async {
      await deleteReceiverCache(root.path);
      expect(await Directory(_join(root.path, '.wisp')).exists(), isFalse);
    });
  });

  group('root resolution', () {
    test('a SAF uri cannot be walked', () async {
      expect(
        await resolveReceiverCacheRoot('content://downloads/tree'),
        isNull,
      );
    });

    test('an empty root cannot be walked', () async {
      expect(await resolveReceiverCacheRoot('   '), isNull);
    });

    test('a plain path is used as given, trimmed', () async {
      expect(await resolveReceiverCacheRoot('  /srv/wisp '), '/srv/wisp');
    });
  });

  group('warning threshold', () {
    ReceiverCacheState sized(int bytes) =>
        ReceiverCacheState(cacheRoot: '/x', sizeBytes: bytes, measured: true);

    test('stays quiet until the cache is actually measured', () {
      expect(
        const ReceiverCacheState(
          cacheRoot: '/x',
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
