import 'package:app/platform/android_file_picker.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  const channel = MethodChannel('dev.vigov5.wisp/file_picker');

  tearDown(() {
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(channel, null);
  });

  test(
    'decodes native sources and copy timing without exposing URI metadata',
    () async {
      TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
          .setMockMethodCallHandler(channel, (call) async {
            expect(call.method, 'pickFiles');
            return <String, Object>{
              'sources': <Map<String, Object>>[
                // Sent straight from its descriptor: the path cannot name the
                // file, so the name has to come from the map.
                {
                  'path': '/proc/self/fd/42',
                  'name': 'holiday.mp4',
                  'size': 6000000000,
                  'copied': false,
                },
                // Fell back to a cache copy, and names itself.
                {
                  'path': '/cache/two.bin',
                  'name': 'two.bin',
                  'size': 4096,
                  'copied': true,
                },
              ],
              'bytesCopied': 4096,
              'copyElapsedMicros': 125000,
            };
          });

      final result = await AndroidFilePicker.pickFiles();

      expect(result.sources.map((source) => source.path), [
        '/proc/self/fd/42',
        '/cache/two.bin',
      ]);
      expect(result.sources.map((source) => source.name), [
        'holiday.mp4',
        'two.bin',
      ]);
      expect(result.sources.map((source) => source.fromDescriptor), [
        true,
        false,
      ]);
      expect(result.sources.first.sizeBytes, BigInt.from(6000000000));
      expect(result.bytesCopied, BigInt.from(4096));
      expect(result.copyElapsed, const Duration(milliseconds: 125));
    },
  );

  test('pick progress falls back to a file count when there are no bytes', () {
    // The descriptor phase copies nothing, so it reports files. Before this,
    // `fraction` keyed off totalBytes alone and every folder pick — including
    // the 1911-file one that spends ~30 s opening descriptors — sat on an
    // indeterminate bar with no numbers at all.
    const files = AndroidPickProgress(
      bytesCopied: 0,
      totalBytes: 0,
      index: 480,
      count: 1911,
    );
    expect(files.countsFiles, isTrue);
    expect(files.fraction, closeTo(480 / 1911, 1e-9));

    // A copy still reports bytes, and must not be described as files.
    const bytes = AndroidPickProgress(
      bytesCopied: 512,
      totalBytes: 2048,
      index: 1,
      count: 4,
    );
    expect(bytes.countsFiles, isFalse);
    expect(bytes.fraction, closeTo(0.25, 1e-9));

    // A copy whose total is unknown stays indeterminate rather than being
    // mistaken for a file count.
    const unknown = AndroidPickProgress(
      bytesCopied: 4096,
      totalBytes: 0,
      index: 0,
      count: 1,
    );
    expect(unknown.countsFiles, isFalse);
    expect(unknown.fraction, isNull);

    // Nothing known at all.
    const empty = AndroidPickProgress(
      bytesCopied: 0,
      totalBytes: 0,
      index: 0,
      count: 0,
    );
    expect(empty.fraction, isNull);
  });

  test('clamps malformed native counters to zero', () async {
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(channel, (_) async {
          return <String, Object>{
            'sources': <Object>[],
            'bytesCopied': -1,
            'copyElapsedMicros': double.nan,
          };
        });

    final result = await AndroidFilePicker.pickFiles();

    expect(result.sources, isEmpty);
    expect(result.bytesCopied, BigInt.zero);
    expect(result.copyElapsed, Duration.zero);
  });

  test('drops native source entries that carry no usable path', () async {
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(channel, (_) async {
          return <String, Object>{
            'sources': <Object>[
              <String, Object?>{'path': '  ', 'name': 'blank'},
              <String, Object?>{'name': 'no-path'},
              <String, Object?>{'path': '/cache/ok.bin'},
            ],
            'bytesCopied': 0,
            'copyElapsedMicros': 0,
          };
        });

    final result = await AndroidFilePicker.pickFiles();

    expect(result.sources.map((source) => source.path), ['/cache/ok.bin']);
    // No `name` in the map, so it falls back to the path's own final segment.
    expect(result.sources.single.name, 'ok.bin');
    expect(result.sources.single.fromDescriptor, isFalse);
  });
}
