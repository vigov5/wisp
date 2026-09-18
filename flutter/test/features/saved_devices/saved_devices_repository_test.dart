import 'package:app/features/saved_devices/application/saved_device.dart';
import 'package:app/features/saved_devices/application/saved_devices_repository.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shared_preferences/shared_preferences.dart';

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  setUp(() {
    SharedPreferences.setMockInitialValues({});
  });

  Future<SavedDevicesRepository> makeRepo() async {
    final prefs = await SharedPreferences.getInstance();
    return SavedDevicesRepository(prefs: prefs);
  }

  test('starts empty', () async {
    final repo = await makeRepo();
    expect(repo.loadAll(), isEmpty);
  });

  test('recordTransfer creates a new entry', () async {
    final repo = await makeRepo();
    await repo.recordTransfer(
      endpointId: 'ABC123',
      label: 'Maya MacBook',
      deviceType: 'laptop',
      bytesTransferred: BigInt.from(1024),
      lastTicket: 'tkt-1',
    );
    final all = repo.loadAll();
    expect(all, hasLength(1));
    expect(all.single.endpointId, 'ABC123');
    expect(all.single.transferCount, 1);
    expect(all.single.totalBytes, BigInt.from(1024));
    expect(all.single.lastTicket, 'tkt-1');
  });

  test('recordTransfer increments existing entry', () async {
    final repo = await makeRepo();
    await repo.recordTransfer(
      endpointId: 'ABC123',
      label: 'Maya MacBook',
      deviceType: 'laptop',
      bytesTransferred: BigInt.from(1024),
    );
    await repo.recordTransfer(
      endpointId: 'ABC123',
      label: 'Maya MacBook',
      deviceType: 'laptop',
      bytesTransferred: BigInt.from(2048),
    );
    final all = repo.loadAll();
    expect(all, hasLength(1));
    expect(all.single.transferCount, 2);
    expect(all.single.totalBytes, BigInt.from(3072));
  });

  test(
    'recordTransfer updates label/type when same pubkey gets a new name',
    () async {
      final repo = await makeRepo();
      await repo.recordTransfer(
        endpointId: 'ABC123',
        label: 'Old Name',
        deviceType: 'laptop',
        bytesTransferred: BigInt.zero,
      );
      await repo.recordTransfer(
        endpointId: 'ABC123',
        label: 'New Name',
        deviceType: 'phone',
        bytesTransferred: BigInt.zero,
      );
      final saved = repo.loadAll().single;
      expect(saved.label, 'New Name');
      expect(saved.deviceType, 'phone');
    },
  );

  test(
    'recordTransfer keeps existing label when update carries placeholder',
    () async {
      final repo = await makeRepo();
      await repo.recordTransfer(
        endpointId: 'ABC123',
        label: 'Maya MacBook',
        deviceType: 'laptop',
        bytesTransferred: BigInt.zero,
      );
      // Subsequent transfer where the peer didn't surface a real name.
      await repo.recordTransfer(
        endpointId: 'ABC123',
        label: 'Sender',
        deviceType: '',
        bytesTransferred: BigInt.zero,
      );
      await repo.recordTransfer(
        endpointId: 'ABC123',
        label: 'Recipient device',
        deviceType: '',
        bytesTransferred: BigInt.zero,
      );
      await repo.recordTransfer(
        endpointId: 'ABC123',
        label: 'Code AB1 2CD',
        deviceType: '',
        bytesTransferred: BigInt.zero,
      );
      final saved = repo.loadAll().single;
      expect(saved.label, 'Maya MacBook');
      expect(saved.deviceType, 'laptop');
    },
  );

  test('remove deletes one entry', () async {
    final repo = await makeRepo();
    await repo.recordTransfer(
      endpointId: 'A',
      label: 'A',
      deviceType: 'phone',
      bytesTransferred: BigInt.zero,
    );
    await repo.recordTransfer(
      endpointId: 'B',
      label: 'B',
      deviceType: 'laptop',
      bytesTransferred: BigInt.zero,
    );
    await repo.remove('A');
    expect(repo.loadAll().map((d) => d.endpointId), ['B']);
  });

  test('clear removes everything', () async {
    final repo = await makeRepo();
    await repo.recordTransfer(
      endpointId: 'A',
      label: 'A',
      deviceType: 'phone',
      bytesTransferred: BigInt.zero,
    );
    await repo.clear();
    expect(repo.loadAll(), isEmpty);
  });

  test('persists across repository instances', () async {
    final r1 = await makeRepo();
    await r1.recordTransfer(
      endpointId: 'X',
      label: 'X',
      deviceType: 'phone',
      bytesTransferred: BigInt.from(50),
    );
    final r2 = await makeRepo();
    expect(r2.loadAll(), hasLength(1));
    expect(r2.loadAll().single.endpointId, 'X');
  });

  test('SavedDevice round-trip JSON', () {
    final original = SavedDevice(
      endpointId: 'ABC',
      label: 'Maya',
      deviceType: 'phone',
      lastSeenAt: DateTime.utc(2026, 5, 8, 12),
      transferCount: 5,
      totalBytes: BigInt.from(1234567890),
      lastTicket: 'tkt',
    );
    final json = original.toJson();
    final restored = SavedDevice.fromJson(json);
    expect(restored, equals(original));
  });

  test('SavedDevice defaults autoAccept to false and round-trips it', () {
    final trusted = SavedDevice(
      endpointId: 'ABC',
      label: 'Maya',
      deviceType: 'phone',
      lastSeenAt: DateTime.utc(2026, 5, 8, 12),
      transferCount: 5,
      totalBytes: BigInt.from(10),
      autoAccept: true,
    );
    expect(SavedDevice.fromJson(trusted.toJson()), equals(trusted));
    // A record written before the feature (no key) reads back as not-trusted.
    final legacy = {
      'endpointId': 'ABC',
      'label': 'Maya',
      'deviceType': 'phone',
      'lastSeenAt': DateTime.utc(2026, 5, 8, 12).toIso8601String(),
      'transferCount': 5,
      'totalBytes': '10',
    };
    expect(SavedDevice.fromJson(legacy)!.autoAccept, isFalse);
  });

  test('setAutoAccept flips the flag on an existing device', () async {
    final repo = await makeRepo();
    await repo.recordTransfer(
      endpointId: 'ABC123',
      label: 'Maya MacBook',
      deviceType: 'laptop',
      bytesTransferred: BigInt.zero,
    );
    expect(repo.loadAll().single.autoAccept, isFalse);
    await repo.setAutoAccept('ABC123', true);
    expect(repo.loadAll().single.autoAccept, isTrue);
    await repo.setAutoAccept('ABC123', false);
    expect(repo.loadAll().single.autoAccept, isFalse);
  });

  test('setAutoAccept no-ops for an unknown device', () async {
    final repo = await makeRepo();
    await repo.setAutoAccept('nope', true);
    expect(repo.loadAll(), isEmpty);
  });

  test('recordTransfer preserves an existing autoAccept flag', () async {
    final repo = await makeRepo();
    await repo.recordTransfer(
      endpointId: 'ABC123',
      label: 'Maya MacBook',
      deviceType: 'laptop',
      bytesTransferred: BigInt.zero,
    );
    await repo.setAutoAccept('ABC123', true);
    await repo.recordTransfer(
      endpointId: 'ABC123',
      label: 'Maya MacBook',
      deviceType: 'laptop',
      bytesTransferred: BigInt.from(2048),
    );
    final saved = repo.loadAll().single;
    expect(saved.autoAccept, isTrue);
    expect(saved.transferCount, 2);
  });

  test('trustFromOffer creates a trusted record when none exists', () async {
    final repo = await makeRepo();
    await repo.trustFromOffer(
      endpointId: 'NEW1',
      label: 'Maya MacBook',
      deviceType: 'laptop',
    );
    final saved = repo.loadAll().single;
    expect(saved.endpointId, 'NEW1');
    expect(saved.autoAccept, isTrue);
    expect(saved.label, 'Maya MacBook');
    expect(saved.transferCount, 0);
  });

  test('trustFromOffer trusts an existing record in place', () async {
    final repo = await makeRepo();
    await repo.recordTransfer(
      endpointId: 'ABC123',
      label: 'Maya MacBook',
      deviceType: 'laptop',
      bytesTransferred: BigInt.from(99),
    );
    await repo.trustFromOffer(
      endpointId: 'ABC123',
      label: 'Sender',
      deviceType: '',
    );
    final saved = repo.loadAll().single;
    expect(saved.autoAccept, isTrue);
    // Existing real name/stats are kept, not clobbered by the placeholder.
    expect(saved.label, 'Maya MacBook');
    expect(saved.totalBytes, BigInt.from(99));
  });

  test('trustFromOffer falls back to a neutral label when none is given', () async {
    final repo = await makeRepo();
    await repo.trustFromOffer(
      endpointId: 'NEW1',
      label: '',
      deviceType: 'laptop',
    );
    // With nothing to show, the tile still needs a label. (A raw placeholder
    // like "Sender" is kept as-is on first save, mirroring recordTransfer, and
    // replaced by a real name on the next transfer.)
    expect(repo.loadAll().single.label, 'Saved device');
  });

  test('eviction caps the list at 30', () async {
    final repo = await makeRepo();
    for (var i = 0; i < 35; i++) {
      await repo.recordTransfer(
        endpointId: 'id-$i',
        label: 'Device $i',
        deviceType: 'laptop',
        bytesTransferred: BigInt.zero,
      );
      // Tiny pause so lastSeenAt differs.
      await Future<void>.delayed(const Duration(milliseconds: 1));
    }
    expect(repo.loadAll(), hasLength(30));
    // Newest entries survive: last inserted should be present.
    expect(repo.loadAll().any((d) => d.endpointId == 'id-34'), isTrue);
  });
}
