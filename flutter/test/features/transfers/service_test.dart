import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shared_preferences/shared_preferences.dart';

import 'package:app/features/saved_devices/application/saved_devices_controller.dart';
import 'package:app/features/saved_devices/application/saved_devices_repository.dart';
import 'package:app/features/transfers/feature.dart';
import 'package:app/platform/rust/receiver/fake_source.dart';

void main() {
  test('transfers service starts idle', () {
    final container = ProviderContainer();
    addTearDown(container.dispose);

    final state = container.read(transfersServiceProvider);

    expect(state.phase, TransferSessionPhase.idle);
    expect(state.offer, isNull);
  });

  test('transfers service tracks an incoming offer', () async {
    final source = FakeReceiverServiceSource();
    final container = ProviderContainer(
      overrides: [transfersServiceSourceProvider.overrideWithValue(source)],
    );
    addTearDown(container.dispose);

    expect(container.read(transfersServiceProvider).offer, isNull);

    source.emitIncomingOffer(senderName: 'Maya');
    await Future<void>.delayed(Duration.zero);

    final updated = container.read(transfersServiceProvider);
    expect(updated.phase, TransferSessionPhase.offerPending);
    expect(updated.offer?.displaySenderName, 'Maya');
    expect(updated.offer?.manifest.itemCount, 2);
    expect(updated.offer?.manifest.totalSizeBytes, BigInt.from(3072));
  });

  test('transfers service shows a connecting state before the offer', () async {
    final source = FakeReceiverServiceSource();
    final container = ProviderContainer(
      overrides: [transfersServiceSourceProvider.overrideWithValue(source)],
    );
    addTearDown(container.dispose);

    // Read first so the service subscribes before any event is emitted.
    expect(container.read(transfersServiceProvider).offer, isNull);

    // A pre-offer "connecting" event switches the UI to a connecting screen
    // built from the sender identity, with no manifest yet.
    source.emitConnecting(senderName: 'Maya');
    await Future<void>.delayed(Duration.zero);

    final connecting = container.read(transfersServiceProvider);
    expect(connecting.phase, TransferSessionPhase.connecting);
    expect(connecting.offer?.displaySenderName, 'Maya');
    expect(connecting.offer?.manifest.itemCount, 0);

    // The real offer then upgrades the same screen to the confirm (pending)
    // state with the full manifest.
    source.emitIncomingOffer(senderName: 'Maya');
    await Future<void>.delayed(Duration.zero);

    final pending = container.read(transfersServiceProvider);
    expect(pending.phase, TransferSessionPhase.offerPending);
    expect(pending.offer?.manifest.itemCount, 2);
  });

  test(
    'transfers service marks incoming offers with resume progress',
    () async {
      final source = FakeReceiverServiceSource();
      final container = ProviderContainer(
        overrides: [transfersServiceSourceProvider.overrideWithValue(source)],
      );
      addTearDown(container.dispose);

      expect(container.read(transfersServiceProvider).offer, isNull);

      source.emitIncomingOffer(
        senderName: 'Maya',
        bytesReceived: BigInt.from(1024),
      );
      await Future<void>.delayed(Duration.zero);

      final offer = container.read(transfersServiceProvider).offer;
      expect(offer?.bytesReceived, BigInt.from(1024));
      expect(offer?.willResume, isTrue);
    },
  );

  test('transfers service forwards offer decisions to the source', () async {
    final source = FakeReceiverServiceSource();
    final container = ProviderContainer(
      overrides: [transfersServiceSourceProvider.overrideWithValue(source)],
    );
    addTearDown(container.dispose);

    expect(
      container.read(transfersServiceProvider).phase,
      TransferSessionPhase.idle,
    );
    source.emitIncomingOffer(senderName: 'Maya');
    await Future<void>.delayed(Duration.zero);

    await container.read(transfersServiceProvider.notifier).acceptOffer();
    expect(source.lastRespondToOfferAccept, isTrue);
    expect(
      container.read(transfersServiceProvider).phase,
      TransferSessionPhase.receiving,
    );
    expect(container.read(transfersServiceProvider).progress?.totalFiles, 2);

    await container.read(transfersServiceProvider.notifier).declineOffer();
    expect(source.lastRespondToOfferAccept, isFalse);
    expect(
      container.read(transfersServiceProvider).phase,
      TransferSessionPhase.idle,
    );
  });

  test(
    'acceptOffer rolls back to pending offer when backend respond fails',
    () async {
      final source = _FailingOfferResponseSource(throwOnAccept: true);
      final container = ProviderContainer(
        overrides: [transfersServiceSourceProvider.overrideWithValue(source)],
      );
      addTearDown(container.dispose);

      source.emitIncomingOffer(senderName: 'Maya');
      await Future<void>.delayed(Duration.zero);

      await expectLater(
        container.read(transfersServiceProvider.notifier).acceptOffer(),
        throwsException,
      );

      final state = container.read(transfersServiceProvider);
      expect(state.phase, TransferSessionPhase.offerPending);
      expect(state.offer?.displaySenderName, 'Maya');
    },
  );

  test(
    'declineOffer restores pending offer when backend respond fails',
    () async {
      final source = _FailingOfferResponseSource(throwOnDecline: true);
      final container = ProviderContainer(
        overrides: [transfersServiceSourceProvider.overrideWithValue(source)],
      );
      addTearDown(container.dispose);

      source.emitIncomingOffer(senderName: 'Maya');
      await Future<void>.delayed(Duration.zero);

      await expectLater(
        container.read(transfersServiceProvider.notifier).declineOffer(),
        throwsException,
      );

      final state = container.read(transfersServiceProvider);
      expect(state.phase, TransferSessionPhase.offerPending);
      expect(state.offer?.displaySenderName, 'Maya');
    },
  );

  test('does not remember a browser (web) sender in Recent', () async {
    SharedPreferences.setMockInitialValues({});
    final prefs = await SharedPreferences.getInstance();
    final repo = SavedDevicesRepository(prefs: prefs);
    final source = FakeReceiverServiceSource();
    final container = ProviderContainer(
      overrides: [
        transfersServiceSourceProvider.overrideWithValue(source),
        savedDevicesRepositoryProvider.overrideWithValue(repo),
      ],
    );
    addTearDown(container.dispose);

    // Subscribe so the service processes the incoming events.
    container.read(transfersServiceProvider);
    expect(container.read(savedDevicesProvider), isEmpty);

    source.emitIncomingOffer(
      senderName: 'Browser',
      senderWeb: true,
      senderEphemeral: true,
    );
    await Future<void>.delayed(Duration.zero);
    source.emitCompletedTransfer(
      senderName: 'Browser',
      senderEndpointId: 'endpoint-web',
      senderWeb: true,
      senderEphemeral: true,
    );
    await Future<void>.delayed(const Duration(milliseconds: 20));

    // A browser peer's key is ephemeral, so it must not land in Recent even
    // though the transfer completed with a valid endpoint id.
    expect(container.read(savedDevicesProvider), isEmpty);
  });

  test('remembers a native (persistent) sender in Recent', () async {
    SharedPreferences.setMockInitialValues({});
    final prefs = await SharedPreferences.getInstance();
    final repo = SavedDevicesRepository(prefs: prefs);
    final source = FakeReceiverServiceSource();
    final container = ProviderContainer(
      overrides: [
        transfersServiceSourceProvider.overrideWithValue(source),
        savedDevicesRepositoryProvider.overrideWithValue(repo),
      ],
    );
    addTearDown(container.dispose);

    container.read(transfersServiceProvider);

    source.emitIncomingOffer(senderName: 'Maya');
    await Future<void>.delayed(Duration.zero);
    source.emitCompletedTransfer(
      senderName: 'Maya',
      senderEndpointId: 'endpoint-maya',
    );
    await Future<void>.delayed(const Duration(milliseconds: 20));

    // Control case: a normal peer with a persistent key is still remembered,
    // so the web/ephemeral guard isn't over-broad.
    final saved = container.read(savedDevicesProvider);
    expect(saved, hasLength(1));
    expect(saved.single.endpointId, 'endpoint-maya');
  });
}

class _FailingOfferResponseSource extends FakeReceiverServiceSource {
  _FailingOfferResponseSource({
    this.throwOnAccept = false,
    this.throwOnDecline = false,
  });

  final bool throwOnAccept;
  final bool throwOnDecline;

  @override
  Future<void> respondToOffer({required bool accept}) async {
    await super.respondToOffer(accept: accept);
    if ((accept && throwOnAccept) || (!accept && throwOnDecline)) {
      throw Exception('respond failed');
    }
  }
}
