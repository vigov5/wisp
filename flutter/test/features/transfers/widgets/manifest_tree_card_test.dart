import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import 'package:app/features/transfers/application/manifest.dart';
import 'package:app/features/transfers/presentation/widgets/manifest_tree.dart';
import 'package:app/features/transfers/presentation/widgets/manifest_tree_card.dart';

void main() {
  List<TransferManifestItem> folderOf(int count) => List.generate(
    count,
    (index) => TransferManifestItem(
      path: 'photos/file${index.toString().padLeft(4, '0')}.bin',
      sizeBytes: BigInt.from(1024 + index),
    ),
    growable: false,
  );

  Future<void> pumpCard(
    WidgetTester tester,
    List<TransferManifestItem> items,
  ) async {
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: Align(
            alignment: Alignment.topCenter,
            child: ManifestTreeCard(items: items),
          ),
        ),
      ),
    );
    await tester.pump();
  }

  // Every tree row carries a Tooltip with its full path, so counting those
  // counts the rows the framework actually built.
  Finder rows() => find.byType(Tooltip);

  // The flat branch has no tooltips; its rows are the path texts.
  Finder paths() => find.textContaining(RegExp(r'^photos/file'));

  testWidgets('a large folder lists flat and builds only what fits', (
    tester,
  ) async {
    // Regression: expanding a 1911-file offer used to lay out every row —
    // the tree sat in a SingleChildScrollView, which offers an unbounded
    // height, so the shrink-wrapping list measured itself against all of
    // them. On the screen that holds the Accept button.
    await pumpCard(tester, folderOf(2000));
    await tester.tap(find.text('Contents'));
    await tester.pumpAndSettle();

    // No tree above the threshold: its per-child animated insertion is the
    // cost that culling cannot fix.
    expect(find.byType(ManifestTree), findsNothing);
    final built = paths().evaluate().length;
    expect(built, greaterThan(0), reason: 'the visible rows must render');
    expect(
      built,
      lessThan(100),
      reason: 'a 200px viewport must not build 2000 rows ($built built)',
    );
  });

  testWidgets('a small folder keeps the tree and still culls', (tester) async {
    await pumpCard(tester, folderOf(120));
    await tester.tap(find.text('Contents'));
    await tester.pumpAndSettle();

    expect(find.byType(ManifestTree), findsOneWidget);
    final built = rows().evaluate().length;
    expect(built, greaterThan(0));
    expect(
      built,
      lessThan(60),
      reason: 'a 200px viewport must not build 120 rows ($built built)',
    );
  });

  testWidgets('the tree survives a rebuild with an equal manifest', (
    tester,
  ) async {
    // The parents rebuild their item list from scratch on every transfer
    // event, so identity never matches. Rebuilding the tree on each of those
    // threw away the expansion state the user had set, and re-walked every
    // path to do it.
    await pumpCard(tester, folderOf(120));
    await tester.tap(find.text('Contents'));
    await tester.pumpAndSettle();
    final before = rows().evaluate().length;

    // A different list object with identical contents.
    await pumpCard(tester, folderOf(120));
    await tester.pumpAndSettle();

    expect(find.text('Contents'), findsOneWidget);
    expect(rows().evaluate().length, before);
  });
}
