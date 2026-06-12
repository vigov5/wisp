import 'dart:convert';
import 'dart:typed_data';

import 'package:app/features/identity/application/identity_backup_codec.dart';
import 'package:cryptography/cryptography.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  // A 32-byte sample key (0..31).
  final key = Uint8List.fromList(List<int>.generate(32, (i) => i));

  // Default codec for the plaintext / format-shape cases.
  final codec = IdentityBackupCodec();

  // Fast codec for the encrypted cases — low PBKDF2 iteration count keeps the
  // round-trip quick. encode and decode share the instance, so the (smaller)
  // iteration count is consistent across both halves.
  IdentityBackupCodec fastCodec() => IdentityBackupCodec(
    pbkdf2: Pbkdf2(
      macAlgorithm: Hmac.sha256(),
      iterations: 100,
      bits: 256,
    ),
  );

  group('plaintext', () {
    test('round-trips through the v1 prefix', () async {
      final payload = await codec.encode(key);
      expect(payload, startsWith('wisp-key:v1:'));
      expect(IdentityBackupCodec.isEncrypted(payload), isFalse);
      expect(IdentityBackupCodec.looksLikeBackup(payload), isTrue);

      final decoded = await codec.decode(payload);
      expect(decoded, equals(key));
    });

    test('empty password is treated as no password', () async {
      final payload = await codec.encode(key, password: '');
      expect(payload, startsWith('wisp-key:v1:'));
    });

    test('decodes a bare 32-byte base64 string leniently', () async {
      final bare = base64.encode(key);
      expect(IdentityBackupCodec.looksLikeBackup(bare), isTrue);
      final decoded = await codec.decode(bare);
      expect(decoded, equals(key));
    });
  });

  group('encrypted', () {
    test('round-trips with the correct password', () async {
      final c = fastCodec();
      final payload = await c.encode(key, password: 'hunter2!');
      expect(payload, startsWith('wisp-key:v1e:'));
      expect(IdentityBackupCodec.isEncrypted(payload), isTrue);
      expect(IdentityBackupCodec.looksLikeBackup(payload), isTrue);

      final decoded = await c.decode(payload, password: 'hunter2!');
      expect(decoded, equals(key));
    });

    test('different salt/nonce each call → different ciphertext', () async {
      final c = fastCodec();
      final a = await c.encode(key, password: 'pw');
      final b = await c.encode(key, password: 'pw');
      expect(a, isNot(equals(b)));
    });

    test('wrong password throws bad-password', () async {
      final c = fastCodec();
      final payload = await c.encode(key, password: 'right');
      expect(
        () => c.decode(payload, password: 'wrong'),
        throwsA(isA<IdentityBackupBadPasswordException>()),
      );
    });

    test('missing password throws', () async {
      final c = fastCodec();
      final payload = await c.encode(key, password: 'pw');
      expect(
        () => c.decode(payload),
        throwsA(isA<IdentityBackupException>()),
      );
    });
  });

  group('validation', () {
    test('encode rejects a non-32-byte key', () {
      expect(
        () => codec.encode(Uint8List.fromList([1, 2, 3])),
        throwsA(isA<IdentityBackupException>()),
      );
    });

    test('decode rejects a wrong-length plaintext payload', () {
      final shortB64 = base64.encode([1, 2, 3]);
      expect(
        () => codec.decode('wisp-key:v1:$shortB64'),
        throwsA(isA<IdentityBackupException>()),
      );
    });

    test('decode rejects garbage', () {
      expect(
        () => codec.decode('not a backup at all'),
        throwsA(isA<IdentityBackupException>()),
      );
    });

    test('looksLikeBackup is false for arbitrary text', () {
      expect(IdentityBackupCodec.looksLikeBackup('hello world'), isFalse);
    });
  });
}
