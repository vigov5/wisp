import 'package:app/platform/android/transfer_keepalive_channel.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  const channel = MethodChannel('dev.vigov5.wisp/transfer_keepalive');
  final invocations = <MethodCall>[];

  TargetPlatform? originalPlatform;

  setUp(() {
    invocations.clear();
    originalPlatform = debugDefaultTargetPlatformOverride;
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(channel, (call) async {
          invocations.add(call);
          if (call.method == 'isIgnoringBatteryOptimizations') return true;
          return null;
        });
  });

  tearDown(() {
    debugDefaultTargetPlatformOverride = originalPlatform;
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(channel, null);
  });

  group('on Android', () {
    setUp(() {
      debugDefaultTargetPlatformOverride = TargetPlatform.android;
    });

    test('start forwards title + body to channel', () async {
      await TransferKeepalive.start(title: 'Wisp sending', body: 'to Maya');
      expect(invocations, hasLength(1));
      expect(invocations.first.method, 'start');
      expect(invocations.first.arguments, {
        'title': 'Wisp sending',
        'body': 'to Maya',
      });
    });

    test('update forwards title + body', () async {
      await TransferKeepalive.update(
        title: 'Wisp sending',
        body: '5 MB / 10 MB',
      );
      expect(invocations.first.method, 'update');
      expect(invocations.first.arguments, {
        'title': 'Wisp sending',
        'body': '5 MB / 10 MB',
      });
    });

    test('stop invokes stop method', () async {
      await TransferKeepalive.stop();
      expect(invocations.single.method, 'stop');
    });

    test(
      'requestIgnoreBatteryOptimizations invokes the right method',
      () async {
        await TransferKeepalive.requestIgnoreBatteryOptimizations();
        expect(invocations.single.method, 'requestIgnoreBatteryOptimizations');
      },
    );

    test('isIgnoringBatteryOptimizations returns the platform reply', () async {
      final ignoring = await TransferKeepalive.isIgnoringBatteryOptimizations();
      expect(ignoring, isTrue);
      expect(invocations.single.method, 'isIgnoringBatteryOptimizations');
    });
  });

  group('on non-Android', () {
    setUp(() {
      debugDefaultTargetPlatformOverride = TargetPlatform.macOS;
    });

    test('start is a no-op', () async {
      await TransferKeepalive.start(title: 't', body: 'b');
      expect(invocations, isEmpty);
    });

    test('update is a no-op', () async {
      await TransferKeepalive.update(title: 't', body: 'b');
      expect(invocations, isEmpty);
    });

    test('stop is a no-op', () async {
      await TransferKeepalive.stop();
      expect(invocations, isEmpty);
    });

    test('isIgnoringBatteryOptimizations returns false', () async {
      final ignoring = await TransferKeepalive.isIgnoringBatteryOptimizations();
      expect(ignoring, isFalse);
      expect(invocations, isEmpty);
    });
  });
}
