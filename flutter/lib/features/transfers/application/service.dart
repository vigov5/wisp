import 'dart:async';

import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../../platform/android/transfer_keepalive_channel.dart';
import '../../../platform/rust/receiver/fake_source.dart';
import '../../../platform/rust/receiver/source.dart';
import '../../../src/rust/api/receiver.dart' as rust_receiver;
import '../../saved_devices/application/saved_devices_controller.dart';
import 'connection_path.dart';
import 'format_utils.dart';
import 'identity.dart';
import 'manifest.dart';
import 'state.dart';

final transfersServiceSourceProvider = Provider<ReceiverServiceSource>(
  (ref) => FakeReceiverServiceSource(),
);

final transfersServiceProvider =
    NotifierProvider<TransfersServiceController, TransferSessionState>(
      TransfersServiceController.new,
    );

class TransfersServiceController extends Notifier<TransferSessionState> {
  StreamSubscription<rust_receiver.ReceiverTransferEvent>? _subscription;
  TransferIncomingOffer? _incomingOffer;
  DateTime? _transferStartTime;
  DateTime? _lastKeepaliveAt;

  /// How the in-flight inline-text offer was accepted (Copy/Save), or `null`
  /// for a file transfer. Set in [acceptOffer] and consumed when the `completed`
  /// event lands, to skip the progress screen and route the finish state.
  TransferTextDelivery? _textDelivery;

  /// Saved file name + folder for a "Save .txt" delivery, surfaced on the finish
  /// screen. Set alongside [_textDelivery]; `null` for Copy and file transfers.
  SavedTextLocation? _savedText;

  @override
  TransferSessionState build() {
    final source = ref.watch(transfersServiceSourceProvider);
    _subscription?.cancel();
    _subscription = source.watchIncomingTransfers().listen((event) {
      switch (event.phase) {
        case rust_receiver.ReceiverTransferPhase.offerReady:
          _incomingOffer = _mapIncomingOffer(event);
          state = TransferSessionState.offerPending(offer: _incomingOffer!);
          return;
        case rust_receiver.ReceiverTransferPhase.connecting:
          // A sender connected and identified itself, but its offer hasn't
          // arrived yet. Show a "connecting from <X>" screen built from the
          // sender identity (empty manifest). Ignore late/duplicate connecting
          // events once a real offer is in hand so we never regress a confirm
          // or receiving screen back to "connecting".
          if (_incomingOffer == null) {
            final connecting = _mapIncomingOffer(event);
            _incomingOffer = connecting;
            state = TransferSessionState.connecting(offer: connecting);
          }
          return;
        case rust_receiver.ReceiverTransferPhase.receiving:
          final freshPath = ConnectionPathInfo.fromReceiver(
            event.connectionPath,
          );
          if (_incomingOffer == null) {
            _incomingOffer = _mapIncomingOffer(event);
          } else if (freshPath != _incomingOffer!.connectionPath) {
            _incomingOffer = _incomingOffer!.copyWith(
              connectionPath: freshPath,
            );
          }
          if (_transferStartTime == null) {
            _transferStartTime = DateTime.now();
            _startKeepalive(senderName: _incomingOffer!.sender.displayName);
          }
          _maybeUpdateKeepalive(event: event);
          state = TransferSessionState.receiving(
            offer: _incomingOffer!,
            progress: _mapProgress(event),
          );
          return;
        case rust_receiver.ReceiverTransferPhase.completed:
          final offer = _mapIncomingOffer(event);
          final result = _mapResult(event);
          _stopKeepalive();
          final endpointId = offer.senderEndpointId;
          if (endpointId != null && endpointId.isNotEmpty) {
            unawaited(
              ref
                  .read(savedDevicesProvider.notifier)
                  .recordTransfer(
                    endpointId: endpointId,
                    label: offer.sender.displayName,
                    deviceType: offer.sender.deviceType.name,
                    bytesTransferred: result.bytesTransferred,
                    lastTicket: event.senderTicket,
                  ),
            );
          }
          final textDelivery = _textDelivery;
          final savedText = _savedText;
          _textDelivery = null;
          _savedText = null;
          if (textDelivery == TransferTextDelivery.copy) {
            // The snippet is already on the clipboard and the toast confirmed
            // it — dismiss straight back to idle, no finish screen.
            state = const TransferSessionState.idle();
            _incomingOffer = null;
            _transferStartTime = null;
            return;
          }
          if (textDelivery == TransferTextDelivery.save) {
            // Nothing actually streamed, so skip the 1s smoothing delay and
            // show the finish screen at once. Prefer the original offer, which
            // still carries `inlineText` so the result card renders the text
            // variant, and pass the saved name/folder for the message.
            state = TransferSessionState.completed(
              offer: _incomingOffer ?? offer,
              result: result,
              savedText: savedText,
            );
            _incomingOffer = null;
            _transferStartTime = null;
            return;
          }
          // The Rust `Completed` event ships an empty file list (see
          // completed_offer_event in crates/app/src/receiver/session.rs), so
          // the freshly mapped `offer` has no manifest items. Prefer the offer
          // retained from `offerReady`, which carries the full file list, so
          // the finish screen shows the same manifest the sender does. Capture
          // it now — the 1s delay below could see an intervening event mutate
          // `_incomingOffer`.
          final completedOffer = _incomingOffer ?? offer;
          unawaited(
            Future.delayed(const Duration(milliseconds: 1000)).then((_) {
              state = TransferSessionState.completed(
                offer: completedOffer,
                result: result,
              );
              _incomingOffer = null;
              _transferStartTime = null;
            }),
          );
          return;
        case rust_receiver.ReceiverTransferPhase.cancelled:
          final offer = _mapIncomingOffer(event);
          _stopKeepalive();
          // Persist a recent-device entry even though the transfer didn't
          // finish — the peer was reachable and the user may want to retry
          // later.  Uses bytesReceived (partial) for the totalBytes counter.
          final endpointId = offer.senderEndpointId;
          if (endpointId != null && endpointId.isNotEmpty) {
            unawaited(
              ref
                  .read(savedDevicesProvider.notifier)
                  .recordTransfer(
                    endpointId: endpointId,
                    label: offer.sender.displayName,
                    deviceType: offer.sender.deviceType.name,
                    bytesTransferred: offer.bytesReceived,
                    lastTicket: event.senderTicket,
                  ),
            );
          }
          // Like the completed event, the cancelled event carries no file
          // list, so prefer the offer retained from `offerReady` to keep the
          // manifest on the finish screen. recordTransfer above still uses the
          // fresh `offer` for its accurate partial bytesReceived / ticket.
          state = TransferSessionState.cancelled(
            offer: _incomingOffer ?? offer,
            errorMessage: event.error?.message ?? event.statusMessage,
          );
          _incomingOffer = null;
          _transferStartTime = null;
          return;
        case rust_receiver.ReceiverTransferPhase.failed:
          final offer = _mapIncomingOffer(event);
          _stopKeepalive();
          state = TransferSessionState.failed(
            offer: offer,
            errorMessage: event.error?.message ?? event.statusMessage,
          );
          _incomingOffer = null;
          _transferStartTime = null;
          return;
        case rust_receiver.ReceiverTransferPhase.declined:
          _stopKeepalive();
          state = const TransferSessionState.idle();
          _incomingOffer = null;
          _transferStartTime = null;
          return;
      }
    });
    ref.onDispose(() => _subscription?.cancel());
    return const TransferSessionState.idle();
  }

  Future<void> acceptOffer({
    TransferTextDelivery? textDelivery,
    SavedTextLocation? savedText,
  }) async {
    final source = ref.read(transfersServiceSourceProvider);
    final offer = state.offer ?? _incomingOffer ?? _offerFromFakeSource(source);
    _textDelivery = textDelivery;
    _savedText = savedText;
    // Inline text already arrived in the offer — there's nothing to transfer,
    // so we skip the progress screen and let the `completed` event route the
    // finish state (idle for Copy, result card for Save). File transfers still
    // show the optimistic `receiving` state while bytes stream in.
    if (offer != null && textDelivery == null) {
      state = TransferSessionState.receiving(
        offer: offer,
        progress: TransferTransferProgress(
          bytesTransferred: offer.bytesReceived,
          totalBytes: offer.manifest.totalSizeBytes,
          completedFiles: 0,
          totalFiles: offer.manifest.itemCount,
        ),
      );
    }
    try {
      await source.respondToOffer(accept: true);
    } catch (_) {
      _textDelivery = null;
      _savedText = null;
      if (offer != null) {
        _incomingOffer = offer;
        state = TransferSessionState.offerPending(offer: offer);
      } else {
        _incomingOffer = null;
        state = const TransferSessionState.idle();
      }
      rethrow;
    }
  }

  Future<void> declineOffer() async {
    final source = ref.read(transfersServiceSourceProvider);
    final offer = state.offer ?? _incomingOffer ?? _offerFromFakeSource(source);
    state = const TransferSessionState.idle();
    _incomingOffer = null;
    try {
      await source.respondToOffer(accept: false);
    } catch (_) {
      if (offer != null) {
        _incomingOffer = offer;
        state = TransferSessionState.offerPending(offer: offer);
      }
      rethrow;
    }
  }

  Future<void> cancelTransfer() {
    final source = ref.read(transfersServiceSourceProvider);
    return source.cancelTransfer();
  }

  void dismissTransferResult() {
    state = const TransferSessionState.idle();
    _incomingOffer = null;
  }

  TransferIncomingOffer _mapIncomingOffer(
    rust_receiver.ReceiverTransferEvent event,
  ) {
    return TransferIncomingOffer(
      sender: TransferIdentity(
        role: TransferRole.sender,
        // The current Flutter bridge does not surface endpoint IDs on transfer events yet.
        endpointId: '',
        deviceName: event.senderName,
        deviceType: _mapDeviceType(event.senderDeviceType),
      ),
      manifest: TransferManifest(
        items: event.files
            .map(
              (file) =>
                  TransferManifestItem(path: file.path, sizeBytes: file.size),
            )
            .toList(growable: false),
      ),
      destinationLabel: event.destinationLabel,
      saveRootLabel: event.saveRootLabel,
      statusMessage: event.statusMessage,
      bytesReceived: event.bytesReceived,
      connectionPath: ConnectionPathInfo.fromReceiver(event.connectionPath),
      senderEndpointId: event.senderEndpointId,
      inlineText: event.inlineText,
    );
  }

  TransferTransferProgress _mapProgress(
    rust_receiver.ReceiverTransferEvent event,
  ) {
    final snapshot = event.snapshot;
    return TransferTransferProgress(
      bytesTransferred: snapshot == null
          ? event.bytesReceived
          : snapshot.bytesTransferred,
      totalBytes: snapshot == null ? event.totalSizeBytes : snapshot.totalBytes,
      completedFiles: snapshot == null ? 0 : snapshot.completedFiles,
      totalFiles: snapshot == null
          ? event.itemCount.toInt()
          : snapshot.totalFiles,
      activeFileIndex: snapshot?.activeFileId,
      activeFileBytesTransferred: snapshot?.activeFileBytes,
      speedLabel: snapshot == null ? null : _formatRate(snapshot.bytesPerSec),
      etaLabel: snapshot == null ? null : _formatEta(snapshot.etaSeconds),
      connectionPath: ConnectionPathInfo.fromReceiver(event.connectionPath),
    );
  }

  TransferTransferResult _mapResult(rust_receiver.ReceiverTransferEvent event) {
    final snapshot = event.snapshot;
    final totalBytes = snapshot == null
        ? event.totalSizeBytes
        : snapshot.totalBytes;
    final bytesTransferred = snapshot == null
        ? event.bytesReceived
        : snapshot.bytesTransferred;

    Duration? duration;
    String? avgSpeedLabel;

    if (_transferStartTime != null) {
      duration = DateTime.now().difference(_transferStartTime!);
      if (duration.inMilliseconds > 0) {
        final avgSpeed =
            (bytesTransferred.toDouble() / (duration.inMilliseconds / 1000.0))
                .round();
        avgSpeedLabel = '${formatBytes(BigInt.from(avgSpeed))}/s';
      }
    }

    return TransferTransferResult(
      bytesTransferred: bytesTransferred,
      totalBytes: totalBytes,
      completedFiles: snapshot == null
          ? event.itemCount.toInt()
          : snapshot.completedFiles,
      totalFiles: snapshot == null
          ? event.itemCount.toInt()
          : snapshot.totalFiles,
      duration: duration,
      averageSpeedLabel: avgSpeedLabel,
    );
  }

  DeviceType _mapDeviceType(String value) {
    switch (value.trim().toLowerCase()) {
      case 'phone':
        return DeviceType.phone;
      case 'laptop':
      default:
        return DeviceType.laptop;
    }
  }

  String? _formatRate(BigInt? bytesPerSec) {
    if (bytesPerSec == null) {
      return null;
    }
    return '${formatBytes(bytesPerSec)}/s';
  }

  String? _formatEta(BigInt? etaSeconds) {
    if (etaSeconds == null) {
      return null;
    }
    return formatEta(etaSeconds);
  }

  void _startKeepalive({required String senderName}) {
    _lastKeepaliveAt = DateTime.now();
    TransferKeepalive.start(
      title: 'Wisp receiving',
      body: senderName.isEmpty ? 'Incoming files' : 'from $senderName',
    ).ignore();
  }

  void _maybeUpdateKeepalive({
    required rust_receiver.ReceiverTransferEvent event,
  }) {
    final now = DateTime.now();
    final last = _lastKeepaliveAt;
    if (last != null && now.difference(last) < const Duration(seconds: 1)) {
      return;
    }
    _lastKeepaliveAt = now;
    final snapshot = event.snapshot;
    final transferred = snapshot?.bytesTransferred ?? event.bytesReceived;
    final total = snapshot?.totalBytes ?? event.totalSizeBytes;
    final body = total > BigInt.zero
        ? '${formatBytes(transferred)} / ${formatBytes(total)}'
        : event.senderName;
    TransferKeepalive.update(title: 'Wisp receiving', body: body).ignore();
  }

  void _stopKeepalive() {
    if (_lastKeepaliveAt == null) return;
    _lastKeepaliveAt = null;
    TransferKeepalive.stop().ignore();
  }

  TransferIncomingOffer? _offerFromFakeSource(ReceiverServiceSource source) {
    if (source is! FakeReceiverServiceSource) {
      return null;
    }

    final senderName = source.lastIncomingSenderName;
    if (senderName == null) {
      return null;
    }

    return TransferIncomingOffer(
      sender: TransferIdentity(
        role: TransferRole.sender,
        endpointId: source.lastIncomingSenderEndpointId ?? '',
        deviceName: senderName,
        deviceType: DeviceType.laptop,
      ),
      manifest: TransferManifest(
        items: (source.lastIncomingFiles ?? const [])
            .map(
              (file) =>
                  TransferManifestItem(path: file.path, sizeBytes: file.size),
            )
            .toList(growable: false),
      ),
      destinationLabel: 'Downloads',
      saveRootLabel: 'Downloads',
      statusMessage: 'Incoming offer',
      bytesReceived: source.lastIncomingBytesReceived ?? BigInt.zero,
    );
  }
}
