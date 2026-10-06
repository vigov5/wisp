import 'package:app/features/settings/application/receiver_cache.dart';
import 'package:app/shell/widgets/receiver_cache_banner.dart';
import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:flutter_test/flutter_test.dart';

/// Stands in for the real notifier so the test never touches a filesystem or
/// the settings providers — the banner's job is only to render what it is
/// handed and to call back.
class _FakeReceiverCache extends ReceiverCacheNotifier {
  _FakeReceiverCache(this._initial);

  final ReceiverCacheState _initial;
  int clears = 0;

  @override
  ReceiverCacheState build() => _initial;

  @override
  Future<String?> clear() async {
    clears++;
    return null;
  }
}

Future<void> _pump(WidgetTester tester, _FakeReceiverCache fake) {
  return tester.pumpWidget(
    ProviderScope(
      overrides: [receiverCacheProvider.overrideWith(() => fake)],
      child: const MaterialApp(
        home: Scaffold(body: Align(child: ReceiverCacheBanner())),
      ),
    ),
  );
}

ReceiverCacheState _measured(int bytes) =>
    ReceiverCacheState(cacheRoot: '/wisp', sizeBytes: bytes, measured: true);

void main() {
  testWidgets('says how big the cache is and offers to clean it', (
    tester,
  ) async {
    final fake = _FakeReceiverCache(_measured(600 * 1024 * 1024));
    await _pump(tester, fake);

    expect(find.text('Receiver cache is using 600 MB'), findsOneWidget);
    expect(find.byIcon(Icons.warning_amber_rounded), findsOneWidget);

    await tester.tap(find.text('Clean now'));
    await tester.pump();

    expect(fake.clears, 1);
  });

  testWidgets('stays out of the way below the threshold', (tester) async {
    await _pump(tester, _FakeReceiverCache(_measured(10 * 1024 * 1024)));

    expect(find.text('Clean now'), findsNothing);
    expect(find.byIcon(Icons.warning_amber_rounded), findsNothing);
  });

  testWidgets('dismissing it is final for the session', (tester) async {
    await _pump(tester, _FakeReceiverCache(_measured(900 * 1024 * 1024)));
    expect(find.text('Clean now'), findsOneWidget);

    await tester.tap(find.byIcon(Icons.close_rounded));
    await tester.pump();

    expect(find.text('Clean now'), findsNothing);
  });
}
