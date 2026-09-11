import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shared_preferences/shared_preferences.dart';

import 'package:app/features/saved_devices/application/saved_devices_controller.dart';
import 'package:app/features/saved_devices/application/saved_devices_repository.dart';
import 'package:app/features/transfers/feature.dart';
import 'package:app/platform/rust/receiver/fake_source.dart';
import 'package:app/src/rust/api/error.dart' as rust_error;

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

  test('a re-emitted offer event does not reopen a decided offer', () async {
    final source = FakeReceiverServiceSource();
    final container = ProviderContainer(
      overrides: [transfersServiceSourceProvider.overrideWithValue(source)],
    );
    addTearDown(container.dispose);

    // Read first so the service subscribes before any event is emitted.
    expect(container.read(transfersServiceProvider).offer, isNull);

    source.emitIncomingOffer(senderName: 'Maya');
    await Future<void>.delayed(Duration.zero);
    expect(
      container.read(transfersServiceProvider).phase,
      TransferSessionPhase.offerPending,
    );

    await container.read(transfersServiceProvider.notifier).acceptOffer();
    await Future<void>.delayed(Duration.zero);
    expect(
      container.read(transfersServiceProvider).phase,
      TransferSessionPhase.receiving,
    );

    // What the UI actually receives seconds later: the bridge answers a
    // connection-path change by cloning the cached offer event, which until the
    // transfer starts is still the OfferReady one. Accepting a large folder
    // spends seconds creating destinations before Rust hears the answer, and
    // the path watcher polls throughout, so this arrived while the user was
    // looking at the progress card — and put the Save button back in front of
    // them.
    source.emitIncomingOffer(senderName: 'Maya');
    await Future<void>.delayed(Duration.zero);

    expect(
      container.read(transfersServiceProvider).phase,
      TransferSessionPhase.receiving,
      reason: 'a late offer event must not reopen a decision already made',
    );
    expect(
      source.respondToOfferCalls,
      1,
      reason: 'and the user must not be able to answer the same offer twice',
    );
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
    'a second acceptOffer while the first is in flight is dropped',
    () async {
      // Regression, measured on a device: tap 2 waited out tap 1's 18.5 s of
      // destination creation, got the platform lock 1 ms later, and began by
      // releasing the 1911 descriptors tap 1's live transfer was writing into.
      // The transfer died 140 ms in, on the first file, and tap 2 then failed
      // with "no pending offer". Two taps, not an intermittent fetch bug.
      final source = _SlowAcceptSource();
      final container = ProviderContainer(
        overrides: [transfersServiceSourceProvider.overrideWithValue(source)],
      );
      addTearDown(container.dispose);

      source.emitIncomingOffer(senderName: 'Maya');
      await Future<void>.delayed(Duration.zero);

      final notifier = container.read(transfersServiceProvider.notifier);
      // Both taps before the first has been answered, which is exactly what a
      // double tap on a screen that takes seconds to respond produces.
      final first = notifier.acceptOffer();
      final second = notifier.acceptOffer();
      await Future.wait([first, second]);

      expect(
        source.acceptCount,
        1,
        reason: 'the platform must see one accept, whatever the user taps',
      );
    },
  );

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

      // Deliberately does not rethrow. Both call sites drop the future — an
      // expression-bodied VoidCallback in the offer card, `unawaited` in the
      // notification handler — so a rethrow could only land as an unhandled
      // zone exception, which is exactly what a device log showed:
      // `Unhandled Exception: Instance of 'UserFacingErrorData'`, with the
      // reason reaching neither the user nor the log.
      await container.read(transfersServiceProvider.notifier).acceptOffer();

      final state = container.read(transfersServiceProvider);
      expect(state.phase, TransferSessionPhase.offerPending);
      expect(state.offer?.displaySenderName, 'Maya');
    },
  );

  test('acceptOffer surfaces a non-retryable backend failure instead of '
      'bouncing back silently', () async {
    // A plain Exception (above) says nothing about whether retrying could
    // work, so the offer stays put. A UserFacingErrorData that declares
    // itself non-retryable has to reach the user: the failed phase renders
    // its title, message and recovery.
    final source = _FailingOfferResponseSource(
      throwOnAccept: true,
      acceptError: const rust_error.UserFacingErrorData(
        kind: rust_error.UserFacingErrorKindData.permissionDenied,
        title: 'Cannot save here',
        message: 'Wisp has no permission to write to Downloads.',
        recovery: 'Pick a different folder in Settings.',
        retryable: false,
      ),
    );
    final container = ProviderContainer(
      overrides: [transfersServiceSourceProvider.overrideWithValue(source)],
    );
    addTearDown(container.dispose);

    source.emitIncomingOffer(senderName: 'Maya');
    await Future<void>.delayed(Duration.zero);
    await container.read(transfersServiceProvider.notifier).acceptOffer();

    final state = container.read(transfersServiceProvider);
    expect(state.phase, TransferSessionPhase.failed);
    expect(state.errorTitle, 'Cannot save here');
    expect(state.errorMessage, 'Wisp has no permission to write to Downloads.');
    expect(state.errorRecovery, 'Pick a different folder in Settings.');
  });

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

/// Counts accepts and answers them only after a turn of the event loop, so a
/// second call can arrive while the first is still in flight.
class _SlowAcceptSource extends FakeReceiverServiceSource {
  int acceptCount = 0;

  @override
  Future<void> respondToOffer({
    required bool accept,
    List<String> transferPaths = const [],
  }) async {
    if (accept) acceptCount++;
    await Future<void>.delayed(const Duration(milliseconds: 20));
    await super.respondToOffer(accept: accept);
  }
}

class _FailingOfferResponseSource extends FakeReceiverServiceSource {
  _FailingOfferResponseSource({
    this.throwOnAccept = false,
    this.throwOnDecline = false,
    this.acceptError,
  });

  final bool throwOnAccept;
  final bool throwOnDecline;

  /// What an accept throws. `null` throws a plain [Exception], which carries
  /// no claim about whether retrying could work.
  final Object? acceptError;

  @override
  Future<void> respondToOffer({
    required bool accept,
    List<String> transferPaths = const [],
  }) async {
    await super.respondToOffer(accept: accept);
    if (accept && throwOnAccept) {
      throw acceptError ?? Exception('respond failed');
    }
    if (!accept && throwOnDecline) {
      throw Exception('respond failed');
    }
  }
}
