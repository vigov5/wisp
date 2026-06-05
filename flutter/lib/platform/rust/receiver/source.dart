import '../../../features/receive/application/state.dart';
import '../../../features/transfers/application/state.dart';
import '../../../src/rust/api/receiver.dart' as rust_receiver;

abstract class ReceiverServiceSource {
  ReceiverServiceState get currentState;

  Stream<ReceiverServiceState> watchState();

  Stream<rust_receiver.ReceiverTransferEvent> watchIncomingTransfers();

  Future<void> setup({String? serverUrl});

  Future<void> ensureRegistered({String? serverUrl});

  Future<void> updateIdentity({
    required String deviceName,
    required String downloadRoot,
    String? serverUrl,
  });

  Future<void> setDiscoverable({required bool enabled});

  Future<void> respondToOffer({required bool accept});

  /// Saves received inline text as a `.txt` to the user-visible destination and
  /// returns the saved name + folder. On desktop the Rust download root is
  /// already the real folder; on Android the text is written to the receive
  /// cache then exported to the chosen SAF folder / Downloads via MediaStore —
  /// mirroring how files land after a transfer, so text doesn't get stranded in
  /// the cache.
  Future<SavedTextLocation> saveInlineText({
    required String suggestedName,
    required String contents,
  });

  Future<void> cancelTransfer();

  Future<List<NearbyReceiver>> scanNearby({required Duration timeout});

  Future<void> shutdown();
}
