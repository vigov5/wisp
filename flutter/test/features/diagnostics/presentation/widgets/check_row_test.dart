import 'package:app/features/diagnostics/domain/check_result.dart';
import 'package:app/features/diagnostics/presentation/widgets/check_row.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

Widget _pumpRow(CheckResult result, {double width = 320}) {
  return MaterialApp(
    home: Scaffold(
      body: Center(
        child: SizedBox(
          width: width,
          child: CheckRow(result: result),
        ),
      ),
    ),
  );
}

void main() {
  testWidgets('renders label, detail, and status icon for a pass result', (
    tester,
  ) async {
    await tester.pumpWidget(
      _pumpRow(
        const CheckResult(
          id: 'network.internet',
          group: CheckGroup.network,
          status: CheckStatus.pass,
          label: 'Internet reachable',
          detail: '32 ms',
        ),
      ),
    );

    expect(find.text('Internet reachable'), findsOneWidget);
    expect(find.text('32 ms'), findsOneWidget);
    expect(find.byIcon(Icons.check_circle_rounded), findsOneWidget);
  });

  testWidgets('shows a spinner instead of an icon while running', (
    tester,
  ) async {
    await tester.pumpWidget(
      _pumpRow(
        const CheckResult(
          id: 'rendezvous.health',
          group: CheckGroup.rendezvous,
          status: CheckStatus.running,
          label: 'Server /healthz',
        ),
      ),
    );

    expect(find.byType(CircularProgressIndicator), findsOneWidget);
    expect(find.text('Server /healthz'), findsOneWidget);
  });

  testWidgets(
    'long detail text wraps to second line without horizontal overflow',
    (tester) async {
      // Regression for the bug where the relay URL + long Android download path
      // caused a 146-pixel right-overflow on phones. The detail must wrap onto
      // its own line below the label rather than spilling out of the row.
      const longRelayUrl =
          '8 sockets bound · 7 LAN IPs · relay aps1-1.relay.n0.iroh-canary.iroh.link';
      await tester.pumpWidget(
        _pumpRow(
          const CheckResult(
            id: 'p2p.transport',
            group: CheckGroup.p2p,
            status: CheckStatus.pass,
            label: 'iroh transport ready',
            detail: longRelayUrl,
          ),
          // Tight phone width to expose any overflow.
          width: 320,
        ),
      );

      expect(tester.takeException(), isNull);
      expect(find.text('iroh transport ready'), findsOneWidget);
      expect(find.textContaining('aps1-1.relay.n0'), findsOneWidget);
    },
  );

  testWidgets('row is expandable when a hint is present', (tester) async {
    await tester.pumpWidget(
      _pumpRow(
        const CheckResult(
          id: 'lan.self_scan',
          group: CheckGroup.lan,
          status: CheckStatus.fail,
          label: 'mDNS self-scan failed',
          detail: 'Did not see own advertisement within 3s.',
          hint: 'Wi-Fi may be in client isolation mode.',
        ),
      ),
    );

    // Collapsed: hint is not visible yet, but the chevron is.
    expect(find.text('Wi-Fi may be in client isolation mode.'), findsNothing);
    expect(find.byIcon(Icons.keyboard_arrow_down_rounded), findsOneWidget);

    await tester.tap(find.byType(InkWell));
    await tester.pumpAndSettle();

    expect(
      find.text('Wi-Fi may be in client isolation mode.'),
      findsOneWidget,
    );
    expect(find.byIcon(Icons.keyboard_arrow_up_rounded), findsOneWidget);
  });

  testWidgets('row without hint or action is not expandable', (tester) async {
    await tester.pumpWidget(
      _pumpRow(
        const CheckResult(
          id: 'network.internet',
          group: CheckGroup.network,
          status: CheckStatus.pass,
          label: 'Internet reachable',
          detail: '32 ms',
        ),
      ),
    );

    expect(find.byIcon(Icons.keyboard_arrow_down_rounded), findsNothing);
    expect(find.byIcon(Icons.keyboard_arrow_up_rounded), findsNothing);
  });

  testWidgets('action button taps invoke the onAction callback', (
    tester,
  ) async {
    CheckAction? captured;
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: SizedBox(
            width: 320,
            child: CheckRow(
              result: const CheckResult(
                id: 'permissions.notifications',
                group: CheckGroup.permissions,
                status: CheckStatus.warn,
                label: 'Notifications denied',
                detail: 'You won\'t get alerts.',
                hint: 'Enable in system settings.',
                action: CheckAction(
                  label: 'Open app settings',
                  kind: CheckActionKind.openAppSettings,
                ),
              ),
              onAction: (action) => captured = action,
            ),
          ),
        ),
      ),
    );

    // Expand the row to reveal the action button.
    await tester.tap(find.byType(InkWell).first);
    await tester.pumpAndSettle();

    await tester.tap(find.text('Open app settings'));
    await tester.pump();

    expect(captured, isNotNull);
    expect(captured!.kind, CheckActionKind.openAppSettings);
  });
}
