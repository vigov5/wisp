import 'dart:convert';
import 'dart:math';
import 'dart:typed_data';

import 'package:flutter_secure_storage/flutter_secure_storage.dart';
import 'package:shared_preferences/shared_preferences.dart';

const String _secureKey = 'app.secret_key.b64';
const String _legacyPrefsKey = 'app.secret_key.b64';
const int _secretKeyLength = 32;

/// Loads (and lazily generates) the persistent 32-byte iroh secret key for
/// this install. The value is stored via `flutter_secure_storage` 10.x:
///   - iOS / macOS: Keychain
///   - Android: RSA-OAEP-wrapped AES-GCM with the key stored in AndroidKeystore
///   - Linux: libsecret
///   - Windows: Windows Credential Locker
///
/// On first launch with this version we migrate any plaintext key found in
/// the regular `shared_preferences` (written by the previous implementation)
/// into secure storage and delete the plaintext copy.
class IdentityStorage {
  IdentityStorage({required this.prefs, FlutterSecureStorage? secureStorage})
    : _secure =
          secureStorage ??
          const FlutterSecureStorage(
            aOptions: AndroidOptions(),
            iOptions: IOSOptions(
              accessibility: KeychainAccessibility.first_unlock,
            ),
          );

  final SharedPreferences prefs;
  final FlutterSecureStorage _secure;

  /// Returns the persisted 32-byte secret key, generating one if missing.
  Future<Uint8List> loadOrCreate() async {
    // 1. Try secure storage first.
    final secureValue = await _safeRead();
    if (secureValue != null) {
      final bytes = _tryDecode(secureValue);
      if (bytes != null) return bytes;
    }

    // 2. Migrate any legacy key from plaintext prefs (older app versions).
    final legacy = prefs.getString(_legacyPrefsKey);
    if (legacy != null && legacy.isNotEmpty) {
      final migrated = _tryDecode(legacy);
      if (migrated != null) {
        await _safeWrite(legacy);
        await prefs.remove(_legacyPrefsKey);
        return migrated;
      }
      // Malformed legacy entry — drop it and fall through to regenerate.
      await prefs.remove(_legacyPrefsKey);
    }

    // 3. Generate fresh.
    final bytes = _randomBytes();
    await _safeWrite(base64.encode(bytes));
    return bytes;
  }

  Uint8List? _tryDecode(String b64) {
    try {
      final bytes = base64.decode(b64);
      if (bytes.length == _secretKeyLength) {
        return Uint8List.fromList(bytes);
      }
    } catch (_) {
      // fall through
    }
    return null;
  }

  Uint8List _randomBytes() {
    final rng = Random.secure();
    final bytes = Uint8List(_secretKeyLength);
    for (var i = 0; i < _secretKeyLength; i++) {
      bytes[i] = rng.nextInt(256);
    }
    return bytes;
  }

  Future<String?> _safeRead() async {
    try {
      return await _secure.read(key: _secureKey);
    } catch (_) {
      // Some Linux/desktop environments don't have a secret service; treat
      // any failure as "no value" and let the migration / regen path run.
      return null;
    }
  }

  Future<void> _safeWrite(String value) async {
    try {
      await _secure.write(key: _secureKey, value: value);
    } catch (_) {
      // If secure storage is unavailable on the host, fall back to plaintext
      // prefs so the app still works (iroh identity is the only thing at
      // stake — the threat model accepts this on dev environments).
      await prefs.setString(_legacyPrefsKey, value);
    }
  }
}
