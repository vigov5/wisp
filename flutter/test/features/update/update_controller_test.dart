import 'dart:convert';

import 'package:app/features/update/application/github_release_api.dart';
import 'package:app/features/update/application/update_providers.dart';
import 'package:app/features/update/domain/update_status.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:http/testing.dart';
import 'package:http/http.dart' as http;
import 'package:package_info_plus/package_info_plus.dart';
import 'package:shared_preferences/shared_preferences.dart';

import '../../support/test_overrides.dart';

// A `releases/latest` payload advertising a newer build than the running app.
String _latestJson(String tag) => jsonEncode({
  'tag_name': tag,
  'html_url': 'https://github.com/vigov5/wisp/releases/tag/$tag',
  'body': 'Notes',
  'assets': const [],
});

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

  GithubReleaseApi apiReturning(String tag) {
    return GithubReleaseApi(
      client: MockClient((_) async => http.Response(_latestJson(tag), 200)),
    );
  }

  Future<ProviderContainer> container({
    required String tag,
    Map<String, Object> prefs = const {},
  }) async {
    SharedPreferences.setMockInitialValues(prefs);
    final repo = await mockUpdateRepo();
    return ProviderContainer(
      overrides: [
        updateRepositoryProvider.overrideWithValue(repo),
        githubReleaseApiProvider.overrideWithValue(apiReturning(tag)),
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
}
