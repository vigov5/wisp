import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:flutter_test/flutter_test.dart';

import 'package:app/app/app.dart';
import 'package:app/features/settings/feature.dart';
import 'support/settings_test_overrides.dart';

Widget _app() => ProviderScope(
  overrides: [initialAppSettingsProvider.overrideWithValue(testAppSettings)],
  child: const WispApp(),
);

void main() {
  testWidgets('home screen shows drop-zone prompt', (
    WidgetTester tester,
  ) async {
    await tester.binding.setSurfaceSize(const Size(1024, 768));
    addTearDown(() => tester.binding.setSurfaceSize(null));

    await tester.pumpWidget(_app());
    await tester.pumpAndSettle();

    expect(find.text('Drop files to send'), findsOneWidget);
  });

  testWidgets('home screen shows file-picker button', (
    WidgetTester tester,
  ) async {
    await tester.binding.setSurfaceSize(const Size(1024, 768));
    addTearDown(() => tester.binding.setSurfaceSize(null));

    await tester.pumpWidget(_app());
    await tester.pumpAndSettle();

    expect(find.text('Share file'), findsOneWidget);
  });
}
