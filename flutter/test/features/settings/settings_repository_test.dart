import 'package:app/features/settings/application/repository.dart';
import 'package:app/platform/rust/rendezvous_defaults.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shared_preferences/shared_preferences.dart';

void main() {
  setUp(() => SharedPreferences.setMockInitialValues({}));

  test('loadOrCreate seeds defaults when preferences are empty', () async {
    final prefs = await SharedPreferences.getInstance();
    final repo = SettingsRepository(
      prefs: prefs,
      randomDeviceName: () => 'Rusty Ridge',
      defaultDownloadRoot: '/tmp/Wisp',
    );

    final settings = await repo.loadOrCreate();

    expect(settings.deviceName, 'Rusty Ridge');
    expect(settings.downloadRoot, '/tmp/Wisp');
    expect(settings.discoverableByDefault, isTrue);
    expect(settings.discoveryServerUrl, defaultRendezvousUrl);
    expect(settings.keepScreenOnDuringTransfer, isTrue);
  });

  test(
    'keepScreenOnDuringTransfer defaults on for installs without the key',
    () async {
      // An upgrade reads through _readExisting, not the seed path, so the
      // default has to be set in both places. Letting it read false there would
      // silently leave every existing install in the slow configuration: a
      // sleeping screen drops Wi-Fi into power-save and costs ~4x throughput.
      SharedPreferences.setMockInitialValues({
        'settings.device_name': 'Rusty Ridge',
        'settings.download_root': '/tmp/Wisp',
      });
      final prefs = await SharedPreferences.getInstance();
      final repo = SettingsRepository(
        prefs: prefs,
        randomDeviceName: () => 'unused',
        defaultDownloadRoot: '/tmp/Wisp',
      );

      final settings = await repo.loadOrCreate();

      expect(settings.keepScreenOnDuringTransfer, isTrue);
    },
  );

  test('save round-trips keepScreenOnDuringTransfer when turned off', () async {
    final prefs = await SharedPreferences.getInstance();
    final repo = SettingsRepository(
      prefs: prefs,
      randomDeviceName: () => 'Rusty Ridge',
      defaultDownloadRoot: '/tmp/Wisp',
    );
    final seeded = await repo.loadOrCreate();

    await repo.save(seeded.copyWith(keepScreenOnDuringTransfer: false));

    // Re-read rather than trusting the returned object: a default of true in
    // the reader would mask a missing write and make "off" un-persistable.
    expect((await repo.loadOrCreate()).keepScreenOnDuringTransfer, isFalse);
  });
}
