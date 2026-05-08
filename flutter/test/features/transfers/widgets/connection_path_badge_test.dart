import 'package:app/features/transfers/application/connection_path.dart';
import 'package:app/features/transfers/presentation/widgets/connection_path_badge.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

Widget _wrap(Widget child) {
  return MaterialApp(home: Scaffold(body: Center(child: child)));
}

void main() {
  testWidgets('renders P2P direct label without IP when no directAddr', (
    tester,
  ) async {
    await tester.pumpWidget(
      _wrap(
        const ConnectionPathBadge(
          path: ConnectionPathInfo(kind: ConnectionPathKind.direct),
        ),
      ),
    );
    expect(find.text('P2P direct'), findsOneWidget);
  });

  testWidgets(
    'renders P2P direct via <ip> label when directAddr provided',
    (tester) async {
      await tester.pumpWidget(
        _wrap(
          const ConnectionPathBadge(
            path: ConnectionPathInfo(
              kind: ConnectionPathKind.direct,
              directAddr: '192.168.1.5:5000',
            ),
          ),
        ),
      );
      expect(find.text('P2P direct via 192.168.1.5'), findsOneWidget);
    },
  );

  testWidgets(
    'renders Via relay: <host> label when kind is relay and URL parseable',
    (tester) async {
      await tester.pumpWidget(
        _wrap(
          const ConnectionPathBadge(
            path: ConnectionPathInfo(
              kind: ConnectionPathKind.relay,
              relayUrl: 'https://relay.example.com:443/',
            ),
          ),
        ),
      );
      expect(find.text('Via relay: relay.example.com'), findsOneWidget);
    },
  );

  testWidgets(
    'falls back to "Via relay" when relay URL has no host',
    (tester) async {
      await tester.pumpWidget(
        _wrap(
          const ConnectionPathBadge(
            path: ConnectionPathInfo(kind: ConnectionPathKind.relay),
          ),
        ),
      );
      expect(find.text('Via relay'), findsOneWidget);
    },
  );

  testWidgets('renders nothing when path is null', (tester) async {
    await tester.pumpWidget(_wrap(const ConnectionPathBadge(path: null)));
    expect(find.byType(Container), findsNothing);
    expect(find.textContaining('relay'), findsNothing);
    expect(find.textContaining('P2P'), findsNothing);
  });

  testWidgets('renders nothing when kind is unknown', (tester) async {
    await tester.pumpWidget(
      _wrap(
        const ConnectionPathBadge(
          path: ConnectionPathInfo(kind: ConnectionPathKind.unknown),
        ),
      ),
    );
    expect(find.textContaining('relay'), findsNothing);
    expect(find.textContaining('P2P'), findsNothing);
  });

  testWidgets(
    'animates between states via AnimatedSwitcher when path changes',
    (tester) async {
      ConnectionPathInfo info = const ConnectionPathInfo(
        kind: ConnectionPathKind.relay,
        relayUrl: 'https://r.example/',
      );

      Widget builder() => _wrap(ConnectionPathBadge(path: info));
      await tester.pumpWidget(builder());
      expect(find.text('Via relay: r.example'), findsOneWidget);

      info = const ConnectionPathInfo(kind: ConnectionPathKind.direct);
      await tester.pumpWidget(builder());
      // mid-animation: AnimatedSwitcher cross-fades; allow both labels possible
      await tester.pump(const Duration(milliseconds: 100));
      // settle to final state
      await tester.pumpAndSettle();
      expect(find.text('P2P direct'), findsOneWidget);
    },
  );
}
