import 'package:app/theme/wisp_theme.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  test('light and dark themes register distinct WispColors', () {
    final light = buildWispTheme(Brightness.light).extension<WispColors>();
    final dark = buildWispTheme(Brightness.dark).extension<WispColors>();
    expect(light, isNotNull);
    expect(dark, isNotNull);
    // The neutrals that must flip actually differ between the two themes.
    expect(dark!.bg, isNot(equals(light!.bg)));
    expect(dark.surface, isNot(equals(light.surface)));
    expect(dark.ink, isNot(equals(light.ink)));
    // Dark ink should be light (near-white) and dark bg should be dark.
    expect(dark.ink.computeLuminance(), greaterThan(0.5));
    expect(dark.bg.computeLuminance(), lessThan(0.1));
    // accentFg brightens on dark.
    expect(dark.accentFg, equals(kAccentCyan));
    expect(light.accentFg, equals(kAccentCyanStrong));
  });

  testWidgets('context.wc resolves to the active theme brightness', (
    tester,
  ) async {
    late WispColors resolved;

    Future<void> pumpWith(ThemeMode mode) async {
      await tester.pumpWidget(
        MaterialApp(
          theme: buildWispTheme(Brightness.light),
          darkTheme: buildWispTheme(Brightness.dark),
          themeMode: mode,
          home: Builder(
            builder: (context) {
              resolved = context.wc;
              return const SizedBox();
            },
          ),
        ),
      );
      await tester.pumpAndSettle();
    }

    // Force the platform brightness so ThemeMode.system is deterministic.
    tester.platformDispatcher.platformBrightnessTestValue = Brightness.dark;
    addTearDown(tester.platformDispatcher.clearPlatformBrightnessTestValue);

    await pumpWith(ThemeMode.light);
    final lightInk = resolved.ink;
    expect(lightInk, equals(kInk));

    await pumpWith(ThemeMode.dark);
    final darkInk = resolved.ink;
    expect(darkInk, isNot(equals(kInk)));
    expect(darkInk.computeLuminance(), greaterThan(0.5));

    // System follows the (dark) platform brightness.
    await pumpWith(ThemeMode.system);
    expect(resolved.ink, equals(darkInk));
  });
}
