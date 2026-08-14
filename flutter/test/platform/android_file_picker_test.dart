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
    'decodes native file copy timing without exposing URI metadata',
    () async {
      TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
          .setMockMethodCallHandler(channel, (call) async {
            expect(call.method, 'pickFiles');
            return <String, Object>{
              'paths': <String>['/cache/one.bin', '/cache/two.bin'],
              'bytesCopied': 4096,
              'copyElapsedMicros': 125000,
            };
          });

      final result = await AndroidFilePicker.pickFiles();

      expect(result.paths, ['/cache/one.bin', '/cache/two.bin']);
      expect(result.bytesCopied, BigInt.from(4096));
      expect(result.copyElapsed, const Duration(milliseconds: 125));
    },
  );

  test('clamps malformed native counters to zero', () async {
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(channel, (_) async {
          return <String, Object>{
            'paths': <String>[],
            'bytesCopied': -1,
            'copyElapsedMicros': double.nan,
          };
        });

    final result = await AndroidFilePicker.pickFiles();

    expect(result.bytesCopied, BigInt.zero);
    expect(result.copyElapsed, Duration.zero);
  });
}
