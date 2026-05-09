import 'package:app/features/settings/feature.dart';
import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shared_preferences/shared_preferences.dart';

/// Finite-duration replacement for [WidgetTester.pumpAndSettle].  Settings
/// page contains [TextField]s; once one is focused (auto-focus on route
/// activation, or after [WidgetTester.enterText]) Flutter's cursor blink
/// runs an `AnimationController.repeat()` which never settles.  We pump for
/// a fixed window long enough to flush every finite animation in the page
/// (route push, dialog open, AnimatedSwitcher, etc.).
Future<void> _pumpSettle(WidgetTester tester) async {
  await tester.pump();
  await tester.pump(const Duration(milliseconds: 500));
}

void main() {
  setUp(() => SharedPreferences.setMockInitialValues({}));

  testWidgets('asks before discarding unsaved changes', (tester) async {
    final prefs = await SharedPreferences.getInstance();
    final repo = SettingsRepository(
      prefs: prefs,
      randomDeviceName: () => 'Rusty Ridge',
      defaultDownloadRoot: '/tmp/Drift',
    );
    final initialSettings = await repo.loadOrCreate();

    await tester.pumpWidget(
      ProviderScope(
        overrides: [
          settingsRepositoryProvider.overrideWithValue(repo),
          initialAppSettingsProvider.overrideWithValue(initialSettings),
        ],
        child: const MaterialApp(home: SettingsFeature()),
      ),
    );
    await _pumpSettle(tester);

    expect(find.text('Settings'), findsOneWidget);
    expect(find.text('Device name'), findsOneWidget);
    expect(find.text('Save received files to'), findsOneWidget);
    expect(find.text('Nearby discoverability'), findsOneWidget);
    expect(find.text('Advanced'), findsOneWidget);
    expect(find.text('Discovery Server'), findsOneWidget);
    expect(find.text('Save Changes'), findsOneWidget);

    await tester.enterText(find.byType(TextField).first, 'Maya MacBook');
    await tester.tap(find.byTooltip('Back'));
    await _pumpSettle(tester);

    expect(find.text('Discard changes?'), findsOneWidget);
    expect(find.text('Stay'), findsOneWidget);
    expect(find.text('Discard'), findsOneWidget);
  });

  testWidgets('enables save when a field changes', (tester) async {
    final prefs = await SharedPreferences.getInstance();
    final repo = SettingsRepository(
      prefs: prefs,
      randomDeviceName: () => 'Rusty Ridge',
      defaultDownloadRoot: '/tmp/Drift',
    );
    final initialSettings = await repo.loadOrCreate();

    await tester.pumpWidget(
      ProviderScope(
        overrides: [
          settingsRepositoryProvider.overrideWithValue(repo),
          initialAppSettingsProvider.overrideWithValue(initialSettings),
        ],
        child: const MaterialApp(home: SettingsFeature()),
      ),
    );
    await _pumpSettle(tester);

    expect(
      tester
          .widget<FilledButton>(
            find.widgetWithText(FilledButton, 'Save Changes'),
          )
          .onPressed,
      isNull,
    );

    await tester.enterText(find.byType(TextField).first, 'Maya MacBook');
    await tester.pump();

    expect(
      tester
          .widget<FilledButton>(
            find.widgetWithText(FilledButton, 'Save Changes'),
          )
          .onPressed,
      isNotNull,
    );
  });

  testWidgets('shows a read-only friendly download path', (tester) async {
    final prefs = await SharedPreferences.getInstance();
    final repo = SettingsRepository(
      prefs: prefs,
      randomDeviceName: () => 'Rusty Ridge',
      defaultDownloadRoot: '/tmp/Drift',
    );
    final initialSettings = await repo.loadOrCreate();
    final internalDownloadRoot =
        '/Users/samarh/Library/Containers/com.example.app/Data/Downloads/Drift';
    final customSettings = initialSettings.copyWith(
      downloadRoot: internalDownloadRoot,
    );

    await tester.pumpWidget(
      ProviderScope(
        overrides: [
          settingsRepositoryProvider.overrideWithValue(repo),
          initialAppSettingsProvider.overrideWithValue(customSettings),
        ],
        child: const MaterialApp(home: SettingsFeature()),
      ),
    );
    await _pumpSettle(tester);

    final downloadField = tester.widget<TextField>(
      find.byKey(const ValueKey<String>('settings-download-root-field')),
    );

    expect(downloadField.readOnly, isTrue);
    expect(downloadField.controller?.text, '/Users/samarh/Downloads/Drift');
  });
}
