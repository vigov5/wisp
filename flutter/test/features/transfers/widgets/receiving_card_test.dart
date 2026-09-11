import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shared_preferences/shared_preferences.dart';

import 'package:app/features/saved_devices/application/saved_devices_controller.dart';
import 'package:app/features/transfers/application/identity.dart';
import 'package:app/features/transfers/application/manifest.dart';
import 'package:app/features/transfers/application/state.dart';
import 'package:app/features/transfers/presentation/widgets/receiving_card.dart';
import 'package:app/platform/android_media_store.dart';

import '../../../support/test_overrides.dart';

void main() {
  setUp(() => SharedPreferences.setMockInitialValues({}));

  TransferIncomingOffer offerOf(BigInt totalBytes) => TransferIncomingOffer(
    sender: TransferIdentity(
      role: TransferRole.sender,
      endpointId: 'endpoint-1',
      deviceName: 'Maya',
      deviceType: DeviceType.phone,
    ),
    manifest: TransferManifest(
      items: [TransferManifestItem(path: 'a.bin', sizeBytes: totalBytes)],
    ),
    destinationLabel: 'Maya',
    saveRootLabel: 'Downloads',
    statusMessage: 'Receiving files...',
    bytesReceived: BigInt.zero,
    senderEndpointId: 'endpoint-1',
  );

  Future<void> pumpCard(
    WidgetTester tester,
    TransferTransferProgress progress,
  ) async {
    final savedDevicesRepo = await mockSavedDevicesRepo();
    await tester.pumpWidget(
      ProviderScope(
        overrides: [
          savedDevicesRepositoryProvider.overrideWithValue(savedDevicesRepo),
        ],
        child: MaterialApp(
          home: Scaffold(
            body: ReceivingCard(
              offer: offerOf(progress.totalBytes),
              progress: progress,
              animate: false,
              onCancel: () {},
            ),
          ),
        ),
      ),
    );
    await tester.pump();
  }

  testWidgets('before the first byte the card says what it is preparing', (
    tester,
  ) async {
    // The mirror of the finishing notice, at the other end of the transfer.
    // Accepting a folder creates one destination per file *before* the answer
    // goes back to the sender — 7-15 s for 1911 files — so this card is
    // already up, with no bytes to show, while the sender still says
    // "Waiting". Users read the stillness as a missed tap and tapped Save
    // again.
    addTearDown(() => AndroidReceiveDestinations.prepareProgress.value = null);
    AndroidReceiveDestinations.prepareProgress.value =
        const AndroidReceivePrepareProgress(created: 320, total: 1911);

    await pumpCard(
      tester,
      TransferTransferProgress(
        bytesTransferred: BigInt.zero,
        totalBytes: BigInt.from(2048),
        completedFiles: 0,
        totalFiles: 1911,
      ),
    );

    expect(find.textContaining('Preparing to receive… (320/1911)'), findsOne);
    expect(find.textContaining('keep Wisp open'), findsOne);
    // The label above it is deliberately untouched: the phase is still the
    // receive, and a second status word for a sub-step reads as a new state.
    expect(find.text('RECEIVING'), findsWidgets);
  });

  testWidgets('with nothing preparing the card keeps its plain subtitle', (
    tester,
  ) async {
    // Zero bytes is not by itself a preparing state — a desktop receiver, or
    // the moment after preparation finishes, has no count to show.
    AndroidReceiveDestinations.prepareProgress.value = null;

    await pumpCard(
      tester,
      TransferTransferProgress(
        bytesTransferred: BigInt.zero,
        totalBytes: BigInt.from(2048),
        completedFiles: 0,
        totalFiles: 1,
      ),
    );

    expect(find.textContaining('Preparing to receive'), findsNothing);
    expect(find.textContaining('keep Wisp open'), findsNothing);
    expect(find.text('Receiving files...'), findsOne);
  });

  testWidgets('mid-transfer shows the speed, not a finishing notice', (
    tester,
  ) async {
    await pumpCard(
      tester,
      TransferTransferProgress(
        bytesTransferred: BigInt.from(512),
        totalBytes: BigInt.from(2048),
        completedFiles: 0,
        totalFiles: 1,
        speedLabel: '12.0 MB/s',
      ),
    );

    expect(find.text('12.0 MB/s'), findsOneWidget);
    expect(find.textContaining('keep Wisp open'), findsNothing);
    expect(find.text('RECEIVING'), findsWidgets);
  });

  testWidgets('the last byte switches the card to a finishing notice', (
    tester,
  ) async {
    // The window this covers is real work and takes real time: the receiver
    // writes each file to its destination and clears its pending flag, 23-26 s
    // for 1911 files. The speed reading vanishes with the last byte, and
    // before this the card fell back to a bare "Receiving files..." — which
    // reads as a stalled transfer and invites closing the app mid-write.
    await pumpCard(
      tester,
      TransferTransferProgress(
        bytesTransferred: BigInt.from(2048),
        totalBytes: BigInt.from(2048),
        completedFiles: 1,
        totalFiles: 1,
        // Deliberately still set: a stale speed must not win over the fact
        // that the bytes are all in.
        speedLabel: '12.0 MB/s',
      ),
    );

    expect(find.text('FINALIZING'), findsWidgets);
    expect(find.textContaining('Saving files to this device'), findsOneWidget);
    expect(find.textContaining('keep Wisp open'), findsOneWidget);
    expect(find.text('12.0 MB/s'), findsNothing);
  });

  testWidgets('the core\'s own finalizing phase is enough on its own', (
    tester,
  ) async {
    // Authoritative where it exists: the receiver's tracker marks Finalizing
    // at the export step, which can begin before the byte counter is rounded
    // off. Byte-complete is the fallback for the sender, which never marks it.
    await pumpCard(
      tester,
      TransferTransferProgress(
        bytesTransferred: BigInt.from(2040),
        totalBytes: BigInt.from(2048),
        completedFiles: 0,
        totalFiles: 1,
        isFinalizing: true,
      ),
    );

    expect(find.text('FINALIZING'), findsWidgets);
    expect(find.textContaining('keep Wisp open'), findsOneWidget);
  });
}
