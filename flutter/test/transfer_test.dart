import 'dart:io';
import 'package:app/app/app.dart';
import 'package:app/features/settings/feature.dart';
import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  Widget app() => ProviderScope(
        overrides: [
          initialAppSettingsProvider.overrideWithValue(
            const AppSettings(
              deviceName: 'Wisp',
              downloadRoot: '/tmp/Wisp',
              discoverableByDefault: true,
              discoveryServerUrl: null,
            ),
          ),
        ],
        child: const WispApp(),
      );

  testWidgets('launches the Wisp shell', (WidgetTester tester) async {
    await tester.binding.setSurfaceSize(const Size(1024, 768));
    addTearDown(() => tester.binding.setSurfaceSize(null));

    await tester.pumpWidget(app());
    await tester.pumpAndSettle();

    final isMobile = Platform.isAndroid || Platform.isIOS;
    if (isMobile) {
      expect(find.text('Select files'), findsOneWidget);
    } else {
      expect(find.text('Drop files to send'), findsOneWidget);
    }
  });
}
