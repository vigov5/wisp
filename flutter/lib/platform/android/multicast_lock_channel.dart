import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';

/// Android-only Wi-Fi multicast lock, held while the app is foreground.
///
/// Android's Wi-Fi driver filters *inbound* multicast when no app holds this
/// lock, to save power. Sending is unaffected, which is what made LAN discovery
/// fail in a confusing way: this device's mDNS announcements reached a desktop
/// on the same subnet, but it never saw the desktop's queries or announcements,
/// so neither side could list the other. `CHANGE_WIFI_MULTICAST_STATE` was
/// already in the manifest for this; nothing had ever taken the lock.
///
/// The native side is idempotent, so repeated calls with the same value are
/// safe and cannot leak or over-release the lock. No-op on every other
/// platform.
class MulticastLock {
  static const _channel = MethodChannel('dev.vigov5.wisp/multicast_lock');

  static bool get _supported =>
      !kIsWeb && defaultTargetPlatform == TargetPlatform.android;

  /// Acquire (`held: true`) or release the lock. Returns whether it is held
  /// afterwards — `false` off-Android, and `false` if the platform refused,
  /// in which case discovery degrades to short codes and nothing else breaks.
  static Future<bool> setHeld({required bool held}) async {
    if (!_supported) return false;
    try {
      final ok = await _channel.invokeMethod<bool>('setHeld', {'held': held});
      return ok ?? false;
    } on MissingPluginException {
      return false;
    } on PlatformException {
      return false;
    }
  }
}
