import 'dart:convert';
import 'dart:math';
import 'dart:typed_data';

import 'package:cryptography/cryptography.dart';

/// Number of raw bytes in an iroh secret key. The backup payload always
/// round-trips to exactly this length.
const int kSecretKeyLength = 32;

/// v1: key only. `wisp-key:v1:<base64 of 32 bytes>`.
const String _plainPrefixV1 = 'wisp-key:v1:';

/// v1 encrypted: key only. `wisp-key:v1e:<base64 of salt|nonce|mac|cipher>`.
const String _encryptedPrefixV1 = 'wisp-key:v1e:';

/// v2: a JSON object `{"k":<b64 key>,"n":<device name>}`.
/// `wisp-key:v2:<base64 of the UTF-8 JSON>`.
const String _plainPrefixV2 = 'wisp-key:v2:';

/// v2 encrypted: the same JSON object, sealed as one so the name is protected.
/// `wisp-key:v2e:<base64 of salt|nonce|mac|cipher(UTF-8 JSON)>`.
const String _encryptedPrefixV2 = 'wisp-key:v2e:';

/// JSON keys inside a v2 payload. New fields can be added over time: a decoder
/// reads the ones it knows and ignores the rest, so additive changes don't need
/// a v3 prefix — that's the whole reason v2 is JSON rather than a fixed frame.
const String _jsonKeyKey = 'k';
const String _jsonKeyName = 'n';

const int _saltLength = 16;
const int _nonceLength = 12;
const int _macLength = 16; // AES-GCM tag.
const int _pbkdf2Iterations = 210000;

/// Cap on the backed-up device name (in UTF-8 bytes). Names are short; this
/// keeps the QR comfortably dense to scan.
const int _maxDeviceNameBytes = 128;

/// The result of decoding a backup payload: always a 32-byte [key], plus the
/// [deviceName] the backup carried when it was created (null for v1 backups
/// that predate device-name capture, or when the name was blank).
class IdentityBackup {
  const IdentityBackup({required this.key, this.deviceName});

  final Uint8List key;
  final String? deviceName;
}

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
/// Shapes (the version bumps to v2 once a device name is included):
///   - `wisp-key:v1:<b64(key)>` — the key as-is, just re-encoded.
///   - `wisp-key:v1e:<b64(salt|nonce|mac|cipher)>` — AES-256-GCM with a key
///     derived from the user's password via PBKDF2-HMAC-SHA256.
///   - `wisp-key:v2:<b64(json)>` — a JSON object `{"k":<b64 key>,"n":<name>}`.
///   - `wisp-key:v2e:<b64(salt|nonce|mac|cipher(json))>` — the same JSON,
///     encrypted as one blob so the name is protected too.
///
/// v2 is JSON so the payload can grow: extra keys can be added later and older
/// decoders simply ignore the ones they don't recognise, no v3 needed. v1
/// payloads still decode (with a null device name), so older backups keep
/// restoring after this format bump.
///
/// Why not age/PGP: the payload must fit in a QR a phone camera reads reliably
/// and decode with zero external tooling on the receiving device.
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

  /// Produces a backup payload for [keyBytes]. When [deviceName] is non-null
  /// and non-blank it is carried in the payload (v2) so a restore can bring the
  /// name back; otherwise the older key-only shape (v1) is emitted. When
  /// [password] is non-null and non-empty the payload is encrypted.
  ///
  /// [salt] / [nonce] are injectable for deterministic tests; production calls
  /// omit them and fresh random values are generated.
  Future<String> encode(
    Uint8List keyBytes, {
    String? deviceName,
    String? password,
    List<int>? salt,
    List<int>? nonce,
  }) async {
    if (keyBytes.length != kSecretKeyLength) {
      throw IdentityBackupException(
        'secret key must be $kSecretKeyLength bytes, got ${keyBytes.length}',
      );
    }

    final name = _sanitizeName(deviceName);
    final hasName = name.isNotEmpty;
    // The bytes that get encrypted (or base64'd as-is): the v2 JSON object when
    // there's a name, or the bare key for v1.
    final payloadBytes = hasName ? _encodeJson(name, keyBytes) : keyBytes;

    if (password == null || password.isEmpty) {
      final prefix = hasName ? _plainPrefixV2 : _plainPrefixV1;
      return '$prefix${base64.encode(payloadBytes)}';
    }

    final usedSalt = salt ?? _randomBytes(_saltLength);
    final usedNonce = nonce ?? _aesGcm.newNonce();
    final derived = await _deriveKey(password, usedSalt);
    final box = await _aesGcm.encrypt(
      payloadBytes,
      secretKey: derived,
      nonce: usedNonce,
    );

    final blob = Uint8List.fromList([
      ...usedSalt,
      ...box.nonce,
      ...box.mac.bytes,
      ...box.cipherText,
    ]);
    final prefix = hasName ? _encryptedPrefixV2 : _encryptedPrefixV1;
    return '$prefix${base64.encode(blob)}';
  }

  /// Turns a backup [payload] back into its 32-byte key (and device name, when
  /// the payload is a v2 form that carries one).
  ///
  /// Accepts every prefixed form, and — to be forgiving about pasted input — a
  /// bare base64 string of exactly 32 bytes (treated as a v1 key). For an
  /// encrypted payload, [password] is required.
  ///
  /// Throws [IdentityBackupBadPasswordException] on a decryption failure and
  /// [IdentityBackupException] for any other malformed input.
  Future<IdentityBackup> decode(String payload, {String? password}) async {
    final trimmed = payload.trim();

    if (trimmed.startsWith(_encryptedPrefixV1) ||
        trimmed.startsWith(_encryptedPrefixV2)) {
      if (password == null || password.isEmpty) {
        throw const IdentityBackupException('this backup needs a password');
      }
      final hasName = trimmed.startsWith(_encryptedPrefixV2);
      final prefix = hasName ? _encryptedPrefixV2 : _encryptedPrefixV1;
      final clear = await _decryptBlob(
        trimmed.substring(prefix.length),
        password,
      );
      return hasName
          ? _decodeJson(clear)
          : IdentityBackup(key: _requireKey(clear));
    }

    if (trimmed.startsWith(_plainPrefixV2)) {
      final bytes = _decodeBase64(trimmed.substring(_plainPrefixV2.length));
      return _decodeJson(bytes);
    }

    final raw = trimmed.startsWith(_plainPrefixV1)
        ? trimmed.substring(_plainPrefixV1.length)
        : trimmed;
    return IdentityBackup(key: _requireKey(_decodeBase64(raw)));
  }

  /// Whether [payload] is a password-protected backup (so the UI can ask for a
  /// password before calling [decode]).
  static bool isEncrypted(String payload) {
    final trimmed = payload.trim();
    return trimmed.startsWith(_encryptedPrefixV1) ||
        trimmed.startsWith(_encryptedPrefixV2);
  }

  /// Whether [payload] is shaped like any recognised backup. Used to validate
  /// pasted/scanned input before attempting a full decode.
  static bool looksLikeBackup(String payload) {
    final trimmed = payload.trim();
    if (trimmed.startsWith(_encryptedPrefixV1) ||
        trimmed.startsWith(_encryptedPrefixV2) ||
        trimmed.startsWith(_plainPrefixV1) ||
        trimmed.startsWith(_plainPrefixV2)) {
      return true;
    }
    try {
      return base64.decode(trimmed).length == kSecretKeyLength;
    } catch (_) {
      return false;
    }
  }

  /// Decrypts a `salt|nonce|mac|cipher` blob, returning the cleartext payload
  /// bytes (a bare key for v1e, or a framed name+key for v2e). The caller
  /// interprets the length.
  Future<Uint8List> _decryptBlob(String b64, String password) async {
    final blob = _decodeBase64(b64);
    const headerLength = _saltLength + _nonceLength + _macLength;
    if (blob.length <= headerLength) {
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
      return Uint8List.fromList(clear);
    } on SecretBoxAuthenticationError {
      throw const IdentityBackupBadPasswordException();
    }
  }

  /// Serialises a device [name] and 32-byte [key] as the v2 JSON object.
  Uint8List _encodeJson(String name, Uint8List key) {
    final json = jsonEncode(<String, String>{
      _jsonKeyKey: base64.encode(key),
      _jsonKeyName: name,
    });
    return Uint8List.fromList(utf8.encode(json));
  }

  /// Parses v2 JSON bytes back into a name + key. Required field: [_jsonKeyKey]
  /// (base64 of the 32-byte key). Unknown keys are ignored, so a future writer
  /// can add fields without breaking this decoder.
  IdentityBackup _decodeJson(Uint8List bytes) {
    final Map<String, dynamic> map;
    try {
      final decoded = jsonDecode(utf8.decode(bytes));
      if (decoded is! Map<String, dynamic>) {
        throw const IdentityBackupException('backup is malformed');
      }
      map = decoded;
    } on FormatException {
      throw const IdentityBackupException('backup is malformed');
    }

    final rawKey = map[_jsonKeyKey];
    if (rawKey is! String) {
      throw const IdentityBackupException('backup is missing its key');
    }
    final key = _requireKey(_decodeBase64(rawKey));

    final rawName = map[_jsonKeyName];
    final name = (rawName is String && rawName.trim().isNotEmpty)
        ? rawName
        : null;
    return IdentityBackup(key: key, deviceName: name);
  }

  /// Validates that [bytes] is exactly a 32-byte key, returning it.
  Uint8List _requireKey(Uint8List bytes) {
    if (bytes.length != kSecretKeyLength) {
      throw IdentityBackupException(
        'expected a $kSecretKeyLength-byte key, got ${bytes.length} bytes',
      );
    }
    return bytes;
  }

  /// Trims a device name and clamps it to [_maxDeviceNameBytes] UTF-8 bytes.
  /// Truncation stops on a rune boundary so the result is always valid UTF-8
  /// (never a split surrogate pair). Returns '' when there's no usable name.
  String _sanitizeName(String? name) {
    final trimmed = name?.trim() ?? '';
    if (trimmed.isEmpty) return '';
    if (utf8.encode(trimmed).length <= _maxDeviceNameBytes) return trimmed;
    final buffer = StringBuffer();
    var bytes = 0;
    for (final rune in trimmed.runes) {
      final runeBytes = utf8.encode(String.fromCharCode(rune)).length;
      if (bytes + runeBytes > _maxDeviceNameBytes) break;
      buffer.writeCharCode(rune);
      bytes += runeBytes;
    }
    return buffer.toString();
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
