import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';

/// Android-only helper to deep-link into the system Tethering settings.
///
/// Android exposes no public API to toggle USB tethering programmatically, so
/// the USB-cable flow guides the user to the settings screen and asks them to
/// flip it on. No-op on every other platform.
class UsbTether {
  static const _channel = MethodChannel('dev.vigov5.wisp/usb_tether');

  static bool get _supported =>
      !kIsWeb && defaultTargetPlatform == TargetPlatform.android;

  /// Whether this platform can deep-link into USB-tethering settings (Android
  /// only). The UI uses this to hide "turn on tethering" / "open tether
  /// settings" affordances where they'd be no-ops — e.g. on Windows/desktop the
  /// cable's *host* is the phone, so tethering is enabled there, not here.
  static bool get isSupported => _supported;

  /// Open the system Tethering settings screen. Returns `true` when a settings
  /// screen was launched, `false` when the platform couldn't surface one (or
  /// when called off-Android / without the host plugin, e.g. widget tests).
  static Future<bool> openTetherSettings() async {
    if (!_supported) return false;
    try {
      final ok = await _channel.invokeMethod<bool>('openTetherSettings');
      return ok ?? false;
    } on MissingPluginException {
      return false;
    } on PlatformException {
      return false;
    }
  }

  /// Whether a USB cable is physically attached, regardless of whether
  /// tethering is on yet. Reads the platform's sticky USB-state broadcast so
  /// the tether checklist can tick "cable connected" before the user enables
  /// tethering (which is what actually brings the network link up).
  static Future<bool> isCableConnected() async {
    if (!_supported) return false;
    try {
      final on = await _channel.invokeMethod<bool>('isCableConnected');
      return on ?? false;
    } on MissingPluginException {
      return false;
    } on PlatformException {
      return false;
    }
  }
}
