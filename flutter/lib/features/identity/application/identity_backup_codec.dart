import 'dart:convert';
import 'dart:math';
import 'dart:typed_data';

import 'package:cryptography/cryptography.dart';

/// Number of raw bytes in an iroh secret key. The backup payload always
/// round-trips to exactly this length.
const int kSecretKeyLength = 32;

/// Prefix for an unencrypted backup payload: `wisp-key:v1:<base64 of 32 bytes>`.
const String _plainPrefix = 'wisp-key:v1:';

/// Prefix for a password-encrypted backup payload:
/// `wisp-key:v1e:<base64 of salt|nonce|mac|ciphertext>`.
const String _encryptedPrefix = 'wisp-key:v1e:';

const int _saltLength = 16;
const int _nonceLength = 12;
const int _macLength = 16; // AES-GCM tag.
const int _pbkdf2Iterations = 210000;

/// Raised when a backup payload can't be turned back into a 32-byte key.
class IdentityBackupException implements Exception {
  const IdentityBackupException(this.message);

  final String message;

  @override
  String toString() => 'IdentityBackupException: $message';
}

/// Raised specifically when decryption fails — almost always a wrong password,
/// but also a corrupted/tampered ciphertext. Separated from
/// [IdentityBackupException] so the UI can show "wrong password" rather than a
/// generic "invalid backup" message.
class IdentityBackupBadPasswordException extends IdentityBackupException {
  const IdentityBackupBadPasswordException()
    : super('wrong password or corrupted backup');
}

/// Encodes/decodes the secret-key backup payload shared by the QR code, the
/// copyable text code, and the `.wispkey` file — they all carry the same
/// string, so import only needs to recognise the prefix.
///
/// Two shapes:
///   - plaintext: `wisp-key:v1:<b64(key)>` — the key as-is, just re-encoded.
///   - encrypted: `wisp-key:v1e:<b64(salt|nonce|mac|cipher)>` — AES-256-GCM
///     with a key derived from the user's password via PBKDF2-HMAC-SHA256.
///
/// Why a custom format and not, say, age/PGP: the payload must fit in a QR a
/// phone camera reads reliably (76 bytes encrypted → ~104 base64 chars, well
/// within a comfortable QR density) and decode with zero external tooling on
/// the receiving device.
class IdentityBackupCodec {
  IdentityBackupCodec({AesGcm? aesGcm, Pbkdf2? pbkdf2})
    : _aesGcm = aesGcm ?? AesGcm.with256bits(),
      _pbkdf2 =
          pbkdf2 ??
          Pbkdf2(
            macAlgorithm: Hmac.sha256(),
            iterations: _pbkdf2Iterations,
            bits: 256,
          );

  final AesGcm _aesGcm;
  final Pbkdf2 _pbkdf2;
  final Random _rng = Random.secure();

  /// Produces a backup payload for [keyBytes]. When [password] is non-null and
  /// non-empty the payload is encrypted; otherwise it's the plaintext form.
  ///
  /// [salt] / [nonce] are injectable for deterministic tests; production calls
  /// omit them and fresh random values are generated.
  Future<String> encode(
    Uint8List keyBytes, {
    String? password,
    List<int>? salt,
    List<int>? nonce,
  }) async {
    if (keyBytes.length != kSecretKeyLength) {
      throw IdentityBackupException(
        'secret key must be $kSecretKeyLength bytes, got ${keyBytes.length}',
      );
    }
    if (password == null || password.isEmpty) {
      return '$_plainPrefix${base64.encode(keyBytes)}';
    }

    final usedSalt = salt ?? _randomBytes(_saltLength);
    final usedNonce = nonce ?? _aesGcm.newNonce();
    final derived = await _deriveKey(password, usedSalt);
    final box = await _aesGcm.encrypt(
      keyBytes,
      secretKey: derived,
      nonce: usedNonce,
    );

    final blob = Uint8List.fromList([
      ...usedSalt,
      ...box.nonce,
      ...box.mac.bytes,
      ...box.cipherText,
    ]);
    return '$_encryptedPrefix${base64.encode(blob)}';
  }

  /// Turns a backup [payload] back into the raw 32-byte key.
  ///
  /// Accepts the two prefixed forms, and — to be forgiving about pasted input —
  /// a bare base64 string of exactly 32 bytes (treated as plaintext). For an
  /// encrypted payload, [password] is required.
  ///
  /// Throws [IdentityBackupBadPasswordException] on a decryption failure and
  /// [IdentityBackupException] for any other malformed input.
  Future<Uint8List> decode(String payload, {String? password}) async {
    final trimmed = payload.trim();

    if (trimmed.startsWith(_encryptedPrefix)) {
      if (password == null || password.isEmpty) {
        throw const IdentityBackupException('this backup needs a password');
      }
      return _decodeEncrypted(
        trimmed.substring(_encryptedPrefix.length),
        password,
      );
    }

    final raw = trimmed.startsWith(_plainPrefix)
        ? trimmed.substring(_plainPrefix.length)
        : trimmed;
    final bytes = _decodeBase64(raw);
    if (bytes.length != kSecretKeyLength) {
      throw IdentityBackupException(
        'expected a $kSecretKeyLength-byte key, got ${bytes.length} bytes',
      );
    }
    return bytes;
  }

  /// Whether [payload] is a password-protected backup (so the UI can ask for a
  /// password before calling [decode]).
  static bool isEncrypted(String payload) =>
      payload.trim().startsWith(_encryptedPrefix);

  /// Whether [payload] is shaped like any recognised backup. Used to validate
  /// pasted/scanned input before attempting a full decode.
  static bool looksLikeBackup(String payload) {
    final trimmed = payload.trim();
    if (trimmed.startsWith(_encryptedPrefix) ||
        trimmed.startsWith(_plainPrefix)) {
      return true;
    }
    try {
      return base64.decode(trimmed).length == kSecretKeyLength;
    } catch (_) {
      return false;
    }
  }

  Future<Uint8List> _decodeEncrypted(String b64, String password) async {
    final blob = _decodeBase64(b64);
    const headerLength = _saltLength + _nonceLength + _macLength;
    if (blob.length != headerLength + kSecretKeyLength) {
      throw const IdentityBackupException('encrypted backup is malformed');
    }
    final salt = blob.sublist(0, _saltLength);
    final nonce = blob.sublist(_saltLength, _saltLength + _nonceLength);
    final mac = blob.sublist(
      _saltLength + _nonceLength,
      _saltLength + _nonceLength + _macLength,
    );
    final cipher = blob.sublist(headerLength);

    final derived = await _deriveKey(password, salt);
    try {
      final clear = await _aesGcm.decrypt(
        SecretBox(cipher, nonce: nonce, mac: Mac(mac)),
        secretKey: derived,
      );
      if (clear.length != kSecretKeyLength) {
        throw const IdentityBackupException('decrypted key has wrong length');
      }
      return Uint8List.fromList(clear);
    } on SecretBoxAuthenticationError {
      throw const IdentityBackupBadPasswordException();
    }
  }

  Uint8List _randomBytes(int length) {
    final bytes = Uint8List(length);
    for (var i = 0; i < length; i++) {
      bytes[i] = _rng.nextInt(256);
    }
    return bytes;
  }

  Future<SecretKey> _deriveKey(String password, List<int> salt) {
    return _pbkdf2.deriveKey(
      secretKey: SecretKey(utf8.encode(password)),
      nonce: salt,
    );
  }

  Uint8List _decodeBase64(String value) {
    try {
      return Uint8List.fromList(base64.decode(value.trim()));
    } catch (_) {
      throw const IdentityBackupException('not a valid backup code');
    }
  }
}
