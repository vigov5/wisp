import 'dart:convert';
import 'dart:io';

import 'package:app/features/update/application/github_release_api.dart';
import 'package:app/features/update/application/update_installer.dart';
import 'package:app/features/update/application/update_providers.dart';
import 'package:app/features/update/domain/update_release.dart';
import 'package:app/features/update/domain/update_status.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:http/testing.dart';
import 'package:http/http.dart' as http;
import 'package:package_info_plus/package_info_plus.dart';
import 'package:shared_preferences/shared_preferences.dart';

import '../../support/test_overrides.dart';

// A `releases/latest` payload advertising a newer build than the running app.
// [withWindowsAsset] attaches the installer asset the Windows install path
// looks for (see UpdateRelease.assetForCurrentPlatform).
String _latestJson(String tag, {bool withWindowsAsset = false}) => jsonEncode({
  'tag_name': tag,
  'html_url': 'https://github.com/vigov5/wisp/releases/tag/$tag',
  'body': 'Notes',
  'assets': [
    if (withWindowsAsset)
      {
        'name': 'wisp-windows-setup-${tag.replaceAll('v', '')}.exe',
        'browser_download_url':
            'https://github.com/vigov5/wisp/releases/download/$tag/setup.exe',
        'size': 1024,
      },
  ],
});

/// A stand-in installer whose launch outcome is scripted, so the controller's
/// post-launch branch can be tested without spawning a real installer.
class _FakeInstaller extends UpdateInstaller {
  _FakeInstaller({required this.launchSucceeds});

  final bool launchSucceeds;
  int revealCount = 0;

  @override
  Future<File> download(
    ReleaseAsset asset, {
    void Function(double? progress)? onProgress,
  }) async {
    onProgress?.call(1.0);
    return File('${Directory.systemTemp.path}${Platform.pathSeparator}'
        '${asset.name}');
  }

  @override
  Future<bool> runWindowsInstaller(File installer) async => launchSucceeds;

  @override
  Future<void> revealInstaller(File installer) async {
    revealCount++;
  }
}

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  // Pretend the running build is v1.6.0 so a v1.7.0 release counts as newer.
  setUp(() {
    PackageInfo.setMockInitialValues(
      appName: 'Wisp',
      packageName: 'dev.vigov5.wisp',
      version: '1.6.0',
      buildNumber: '9',
      buildSignature: '',
    );
  });

  GithubReleaseApi apiReturning(String tag, {bool withWindowsAsset = false}) {
    return GithubReleaseApi(
      client: MockClient(
        (_) async => http.Response(
          _latestJson(tag, withWindowsAsset: withWindowsAsset),
          200,
        ),
      ),
    );
  }

  Future<ProviderContainer> container({
    required String tag,
    Map<String, Object> prefs = const {},
    bool withWindowsAsset = false,
    UpdateInstaller? installer,
  }) async {
    SharedPreferences.setMockInitialValues(prefs);
    final repo = await mockUpdateRepo();
    return ProviderContainer(
      overrides: [
        updateRepositoryProvider.overrideWithValue(repo),
        githubReleaseApiProvider.overrideWithValue(
          apiReturning(tag, withWindowsAsset: withWindowsAsset),
        ),
        if (installer != null)
          updateInstallerProvider.overrideWithValue(installer),
      ],
    );
  }

  test('automatic check surfaces a newer release on every launch', () async {
    final c = await container(tag: 'v1.7.0');
    addTearDown(c.dispose);

    await c.read(updateControllerProvider.notifier).checkForUpdates();

    final state = c.read(updateControllerProvider);
    expect(state.phase, UpdatePhase.available);
    expect(state.release?.tagName, 'v1.7.0');
  });

  test('automatic check is skipped when the startup toggle is off', () async {
    final c = await container(
      tag: 'v1.7.0',
      prefs: {'update.check_on_startup': false},
    );
    addTearDown(c.dispose);

    await c.read(updateControllerProvider.notifier).checkForUpdates();

    // No check ran: phase never left idle.
    expect(c.read(updateControllerProvider).phase, UpdatePhase.idle);
  });

  test('automatic check honours a skipped version', () async {
    final c = await container(
      tag: 'v1.7.0',
      prefs: {'update.skipped_version': 'v1.7.0'},
    );
    addTearDown(c.dispose);

    await c.read(updateControllerProvider.notifier).checkForUpdates();

    expect(c.read(updateControllerProvider).phase, UpdatePhase.upToDate);
  });

  test('manual check ignores both the toggle and the skip list', () async {
    final c = await container(
      tag: 'v1.7.0',
      prefs: {
        'update.check_on_startup': false,
        'update.skipped_version': 'v1.7.0',
      },
    );
    addTearDown(c.dispose);

    await c
        .read(updateControllerProvider.notifier)
        .checkForUpdates(manual: true);

    final state = c.read(updateControllerProvider);
    expect(state.phase, UpdatePhase.available);
    expect(state.release?.tagName, 'v1.7.0');
  });

  test(
    'a failed installer launch falls back to manual install + reveal',
    () async {
      final installer = _FakeInstaller(launchSucceeds: false);
      final c = await container(
        tag: 'v1.7.0',
        withWindowsAsset: true,
        installer: installer,
      );
      addTearDown(c.dispose);

      final notifier = c.read(updateControllerProvider.notifier);
      await notifier.checkForUpdates();
      await notifier.downloadAndInstall();

      final state = c.read(updateControllerProvider);
      expect(state.phase, UpdatePhase.manualInstall);
      expect(state.installerPath, isNotNull);
      expect(installer.revealCount, 1);
    },
    // The Windows install path only engages when assetForCurrentPlatform()
    // resolves, which it does on Windows only.
    skip: !Platform.isWindows,
  );
}
