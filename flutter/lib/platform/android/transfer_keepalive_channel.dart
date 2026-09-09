import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';

/// Android-only foreground-service controller. No-op on every other platform.
class TransferKeepalive {
  static const _channel = MethodChannel('dev.vigov5.wisp/transfer_keepalive');

  static bool get _supported =>
      !kIsWeb && defaultTargetPlatform == TargetPlatform.android;

  /// Start the foreground service and post the ongoing notification. The
  /// service holds a partial wake lock + Wi-Fi low-latency lock for its
  /// lifetime. Idempotent: calling again with new title/body just updates the
  /// notification.
  ///
  /// [keepScreenOn] additionally holds the screen awake until [stop]. That is
  /// separate from the service's locks and cannot be folded into them: a sleeping
  /// screen puts Wi-Fi into power-save regardless of any lock an app holds, which
  /// costs ~4x throughput.
  static Future<void> start({
    required String title,
    required String body,
    bool keepScreenOn = false,
  }) async {
    if (!_supported) return;
    await _channel.invokeMethod<void>('start', {
      'title': title,
      'body': body,
      'keepScreenOn': keepScreenOn,
    });
  }

  /// Update the notification text without restarting the service or touching
  /// the wake locks. Title is required so the call is self-contained.
  static Future<void> update({
    required String title,
    required String body,
  }) async {
    if (!_supported) return;
    await _channel.invokeMethod<void>('update', {'title': title, 'body': body});
  }

  /// Stop the service, release its locks, and let the screen sleep again.
  static Future<void> stop() async {
    if (!_supported) return;
    await _channel.invokeMethod<void>('stop');
  }

  /// Open the system battery-optimisation prompt for this app. The user can
  /// then accept or dismiss; the result must be observed via
  /// [isIgnoringBatteryOptimizations] on resume.
  static Future<void> requestIgnoreBatteryOptimizations() async {
    if (!_supported) return;
    await _channel.invokeMethod<void>('requestIgnoreBatteryOptimizations');
  }

  /// Whether the user has granted the battery-optimisation exemption.
  /// Returns `false` on non-Android platforms or when the platform handler is
  /// missing (e.g., widget tests where the host plugin isn't registered —
  /// without this guard, the future never completes and `pumpAndSettle`
  /// hangs).
  static Future<bool> isIgnoringBatteryOptimizations() async {
    if (!_supported) return false;
    try {
      final ok = await _channel.invokeMethod<bool>(
        'isIgnoringBatteryOptimizations',
      );
      return ok ?? false;
    } on MissingPluginException {
      return false;
    } on PlatformException {
      return false;
    }
  }
}
