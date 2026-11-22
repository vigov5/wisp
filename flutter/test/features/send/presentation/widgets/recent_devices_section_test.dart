import 'dart:convert';

import 'package:app/app/app_router.dart';
import 'package:app/features/receive/application/service.dart';
import 'package:app/features/receive/application/state.dart';
import 'package:app/features/saved_devices/application/saved_devices_controller.dart';
import 'package:app/features/saved_devices/presentation/saved_devices_page.dart';
import 'package:app/features/send/application/controller.dart';
import 'package:app/features/send/application/model.dart';
import 'package:app/features/send/application/state.dart';
import 'package:app/features/settings/settings_providers.dart';
import 'package:app/platform/rust/receiver/fake_source.dart';
import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shared_preferences/shared_preferences.dart';

import '../../../../support/settings_test_overrides.dart';
import '../../../../support/test_overrides.dart';

Map<String, Object?> _record(
  String id,
  int minutesAgo, {
  bool favorite = false,
}) => {
  'endpointId': id,
  'label': id,
  'deviceType': 'laptop',
  'lastSeenAt': DateTime.now()
      .toUtc()
      .subtract(Duration(minutes: minutesAgo))
      .toIso8601String(),
  'transferCount': 1,
  'totalBytes': '0',
  'lastTicket': 'ticket-$id',
  'favorite': favorite,
};

Future<ProviderContainer> _pumpSendDraft(
  WidgetTester tester, {
  List<NearbyReceiver> nearby = const [],
}) async {
  final router = buildAppRouter();
  final container = ProviderContainer(
    overrides: [
      initialAppSettingsProvider.overrideWithValue(testAppSettings),
      receiverServiceSourceProvider.overrideWithValue(
        FakeReceiverServiceSource(nearbyResults: nearby),
      ),
      savedDevicesRepositoryProvider.overrideWithValue(
        await mockSavedDevicesRepo(),
      ),
      ...usbTestOverrides,
    ],
  );
  addTearDown(container.dispose);
  await tester.pumpWidget(
    UncontrolledProviderScope(
      container: container,
      child: MaterialApp.router(routerConfig: router),
    ),
  );
  router.go(
    AppRoutePaths.sendDraft,
    extra: [
      SendPickedFile(
        path: '/tmp/report.pdf',
        name: 'report.pdf',
        sizeBytes: BigInt.from(1024),
      ),
    ],
  );
  await pumpFinite(tester);
  return container;
}

void main() {
  setUp(() {
    SharedPreferences.setMockInitialValues({
      'saved_devices.v1': jsonEncode([
        _record('Alpha', 1),
        _record('Gamma', 30),
        _record('Beta', 90, favorite: true),
      ]),
    });
  });

  testWidgets('a favorite is pinned first in Recent and wears a star', (
    tester,
  ) async {
    await _pumpSendDraft(tester);

    final beta = tester.getTopLeft(find.text('Beta')).dx;
    final alpha = tester.getTopLeft(find.text('Alpha')).dx;
    final gamma = tester.getTopLeft(find.text('Gamma')).dx;
    expect(beta, lessThan(alpha));
    expect(alpha, lessThan(gamma));

    expect(find.byIcon(Icons.star_rounded), findsOneWidget);
    expect(
      find.ancestor(of: find.text('Beta'), matching: find.byType(Scrollbar)),
      findsOneWidget,
    );
  });

  testWidgets('a nearby device that is a favorite wears a star', (
    tester,
  ) async {
    await _pumpSendDraft(
      tester,
      nearby: const [
        NearbyReceiver(
          fullname: 'beta._wisp',
          label: 'Beta on LAN',
          deviceType: 'laptop',
          code: '',
          ticket: 'lan-ticket-beta',
          endpointId: 'Beta',
        ),
        NearbyReceiver(
          fullname: 'stranger._wisp',
          label: 'Stranger',
          deviceType: 'phone',
          code: '',
          ticket: 'lan-ticket-stranger',
          endpointId: 'Stranger',
        ),
      ],
    );
    await pumpFinite(tester, duration: const Duration(seconds: 2));

    Finder starIn(String label) => find.descendant(
      of: find
          .ancestor(of: find.text(label), matching: find.byType(Stack))
          .first,
      matching: find.byIcon(Icons.star_rounded),
    );
    expect(starIn('Beta on LAN'), findsOneWidget);
    expect(starIn('Stranger'), findsNothing);
  });

  testWidgets('Pick lists every saved device and selects the tapped one', (
    tester,
  ) async {
    final container = await _pumpSendDraft(tester);

    final pick = find.text('Pick');
    await tester.ensureVisible(pick);
    await tester.tap(pick);
    await pumpFinite(tester);

    final sheet = find.byKey(const ValueKey('saved-device-picker'));
    expect(sheet, findsOneWidget);
    double rowY(String label) => tester
        .getTopLeft(find.descendant(of: sheet, matching: find.text(label)))
        .dy;
    expect(rowY('Beta'), lessThan(rowY('Alpha')));
    expect(rowY('Alpha'), lessThan(rowY('Gamma')));

    await tester.tap(find.descendant(of: sheet, matching: find.text('Gamma')));
    await pumpFinite(tester);

    expect(sheet, findsNothing);
    final state = container.read(sendControllerProvider);
    expect(state, isA<SendStateDrafting>());
    expect((state as SendStateDrafting).destination.ticket, 'ticket-Gamma');
  });

  testWidgets('Manage opens Saved devices, where the star toggles favorite', (
    tester,
  ) async {
    final container = await _pumpSendDraft(tester);

    final manage = find.text('Manage');
    await tester.ensureVisible(manage);
    await tester.tap(manage);
    await pumpFinite(tester);
    expect(find.byType(SavedDevicesPage), findsOneWidget);
    expect(
      tester.getTopLeft(find.text('Beta')).dy,
      lessThan(tester.getTopLeft(find.text('Alpha')).dy),
    );

    final alphaRow = find.ancestor(
      of: find.text('Alpha'),
      matching: find.byType(Dismissible),
    );
    await tester.tap(
      find.descendant(
        of: alphaRow,
        matching: find.byTooltip('Add to favorites'),
      ),
    );
    await pumpFinite(tester);

    final alpha = container
        .read(savedDevicesProvider)
        .firstWhere((d) => d.endpointId == 'Alpha');
    expect(alpha.favorite, isTrue);
    expect(
      find.descendant(
        of: alphaRow,
        matching: find.byTooltip('Remove from favorites'),
      ),
      findsOneWidget,
    );
  });
}
