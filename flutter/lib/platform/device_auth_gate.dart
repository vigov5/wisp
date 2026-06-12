import 'dart:io';

import 'package:flutter/services.dart';
import 'package:local_auth/local_auth.dart';

/// Outcome of a device-auth challenge.
enum DeviceAuthResult {
  /// The user passed biometric / device-credential authentication.
  success,

  /// The user failed or cancelled the prompt.
  failed,

  /// Auth can't be enforced here: a desktop platform (no `local_auth`
  /// support), or a mobile device with no screen lock / enrolled biometric.
  /// Callers proceed, but should nudge mobile users to set up a lock.
  unsupported,
}

/// Wraps `local_auth` so the secret-key export screen can require the user to
/// prove they're the device owner before the key is read into memory.
///
/// Only Android and iOS are gated — `local_auth` has no Linux/Windows backend
/// in this app's setup, and the desktop story (keychain-at-rest) differs. When
/// a mobile device has no secured lock at all, auth can't be enforced, so the
/// gate reports [DeviceAuthResult.unsupported] rather than blocking the user
/// out of their own backup.
class DeviceAuthGate {
  DeviceAuthGate({LocalAuthentication? auth})
    : _auth = auth ?? LocalAuthentication();

  final LocalAuthentication _auth;

  /// Whether this platform participates in the auth gate at all. Desktop
  /// returns false (callers treat that as "proceed").
  static bool get isGatedPlatform => Platform.isAndroid || Platform.isIOS;

  /// Prompts for biometric / device-credential auth.
  ///
  /// Returns [DeviceAuthResult.unsupported] without prompting on desktop or
  /// when the device has no lock configured; otherwise the prompt's outcome.
  Future<DeviceAuthResult> authenticate(String reason) async {
    if (!isGatedPlatform) return DeviceAuthResult.unsupported;
    try {
      // `isDeviceSupported()` is true when the OS has *some* enrolled auth
      // (biometric or device passcode). If false, there's nothing to prompt.
      final supported = await _auth.isDeviceSupported();
      if (!supported) return DeviceAuthResult.unsupported;

      final ok = await _auth.authenticate(
        localizedReason: reason,
        options: const AuthenticationOptions(
          // Allow the device PIN/passcode as a fallback to biometrics — the
          // goal is "prove device ownership", not strictly a fingerprint.
          biometricOnly: false,
          stickyAuth: true,
        ),
      );
      return ok ? DeviceAuthResult.success : DeviceAuthResult.failed;
    } on PlatformException {
      // Lockout, hardware unavailable mid-call, no activity, etc. Treat as a
      // failed challenge — the caller keeps the key hidden.
      return DeviceAuthResult.failed;
    }
  }
}
