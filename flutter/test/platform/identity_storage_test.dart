import 'dart:convert';

import 'package:app/platform/identity_storage.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shared_preferences/shared_preferences.dart';

const _secureChannelName = 'plugins.it_nomads.com/flutter_secure_storage';

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  late Map<String, String> secureStore;

  setUp(() {
    SharedPreferences.setMockInitialValues({});
    secureStore = <String, String>{};
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(const MethodChannel(_secureChannelName), (
          call,
        ) async {
          final args = call.arguments as Map<Object?, Object?>? ?? const {};
          final key = args['key'] as String?;
          switch (call.method) {
            case 'read':
              return secureStore[key];
            case 'write':
              final value = args['value'] as String?;
              if (key != null && value != null) secureStore[key] = value;
              return null;
            case 'delete':
              if (key != null) secureStore.remove(key);
              return null;
            case 'deleteAll':
              secureStore.clear();
              return null;
            case 'readAll':
              return Map<String, String>.from(secureStore);
            case 'containsKey':
              return secureStore.containsKey(key);
          }
          return null;
        });
  });

  tearDown(() {
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(
          const MethodChannel(_secureChannelName),
          null,
        );
  });

  test('generates a 32-byte secret on first run', () async {
    final prefs = await SharedPreferences.getInstance();
    final identity = IdentityStorage(prefs: prefs);
    final bytes = await identity.loadOrCreate();
    expect(bytes, hasLength(32));
    final stored = secureStore['app.secret_key.b64'];
    expect(stored, isNotNull);
    expect(base64.decode(stored!), hasLength(32));
  });

  test('returns the same secret on subsequent calls', () async {
    final prefs = await SharedPreferences.getInstance();
    final identity = IdentityStorage(prefs: prefs);
    final first = await identity.loadOrCreate();
    final second = await identity.loadOrCreate();
    expect(second, first);
  });

  test('migrates a legacy plaintext key from shared_preferences', () async {
    final legacy = base64.encode(List<int>.generate(32, (i) => i + 1));
    SharedPreferences.setMockInitialValues({'app.secret_key.b64': legacy});
    final prefs = await SharedPreferences.getInstance();
    final identity = IdentityStorage(prefs: prefs);
    final bytes = await identity.loadOrCreate();
    expect(bytes, hasLength(32));
    expect(bytes.first, 1);
    // Migrated to secure store.
    expect(secureStore['app.secret_key.b64'], legacy);
    // Legacy plaintext copy removed.
    expect(prefs.getString('app.secret_key.b64'), isNull);
  });

  test('regenerates if stored value is malformed', () async {
    secureStore['app.secret_key.b64'] = 'not-base64-!!!';
    final prefs = await SharedPreferences.getInstance();
    final identity = IdentityStorage(prefs: prefs);
    final bytes = await identity.loadOrCreate();
    expect(bytes, hasLength(32));
  });

  test('regenerates if stored value has wrong length', () async {
    secureStore['app.secret_key.b64'] = base64.encode([1, 2, 3]);
    final prefs = await SharedPreferences.getInstance();
    final identity = IdentityStorage(prefs: prefs);
    final bytes = await identity.loadOrCreate();
    expect(bytes, hasLength(32));
  });

  test('read returns null when nothing is stored', () async {
    final prefs = await SharedPreferences.getInstance();
    final identity = IdentityStorage(prefs: prefs);
    expect(await identity.read(), isNull);
  });

  test('read returns the stored key without generating one', () async {
    final stored = base64.encode(List<int>.generate(32, (i) => i + 1));
    secureStore['app.secret_key.b64'] = stored;
    final prefs = await SharedPreferences.getInstance();
    final identity = IdentityStorage(prefs: prefs);
    final bytes = await identity.read();
    expect(bytes, isNotNull);
    expect(base64.encode(bytes!), stored);
    // No new key was minted as a side effect.
    expect(secureStore['app.secret_key.b64'], stored);
  });

  test('replace overwrites the stored key', () async {
    final prefs = await SharedPreferences.getInstance();
    final identity = IdentityStorage(prefs: prefs);
    await identity.loadOrCreate();
    final imported = Uint8List.fromList(List<int>.generate(32, (i) => 255 - i));
    await identity.replace(imported);
    expect(secureStore['app.secret_key.b64'], base64.encode(imported));
    expect(await identity.read(), imported);
  });

  test('replace rejects a wrong-length key', () async {
    final prefs = await SharedPreferences.getInstance();
    final identity = IdentityStorage(prefs: prefs);
    expect(
      () => identity.replace(Uint8List.fromList([1, 2, 3])),
      throwsA(isA<ArgumentError>()),
    );
  });
}
