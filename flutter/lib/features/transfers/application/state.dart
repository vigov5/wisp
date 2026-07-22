import 'package:flutter/foundation.dart';

import 'connection_path.dart';
import 'identity.dart';
import 'manifest.dart';

enum TransferSessionPhase {
  idle,

  /// A sender has connected and identified itself (Hello exchanged) but its
  /// offer hasn't arrived yet. The UI shows a "connecting from sender" screen
  /// with no accept/decline — there's no manifest to act on until the offer
  /// lands.
  connecting,
  offerPending,
  receiving,
  completed,
  cancelled,
  failed,
}

/// How the receiver handled an inline-text offer. The text already arrived in
/// the offer, so there's nothing to transfer: [copy] lands it on the clipboard
/// and dismisses straight back to idle (the toast is the confirmation), while
/// [save] writes a .txt and shows the finish screen so the user can open the
/// folder. `null` everywhere else means an ordinary file transfer.
enum TransferTextDelivery { copy, save }

/// Where a received text snippet was written, in human-readable form — surfaced
/// in the save toast and on the finish screen so the user knows the exact name
/// and folder. [folderLabel] is a friendly path (e.g. `Download/Wisp` or a
/// desktop absolute path), never a raw `content://` SAF URI.
@immutable
class SavedTextLocation {
  const SavedTextLocation({required this.fileName, required this.folderLabel});

  final String fileName;
  final String folderLabel;

  /// `<folder>/<file>` for display, joined with the folder's own separator.
  String get fullPath {
    final sep = folderLabel.contains('\\') ? '\\' : '/';
    final trimmed = folderLabel.endsWith(sep)
        ? folderLabel.substring(0, folderLabel.length - 1)
        : folderLabel;
    return '$trimmed$sep$fileName';
  }
}

@immutable
class TransferTransferProgress {
  const TransferTransferProgress({
    required this.bytesTransferred,
    required this.totalBytes,
    required this.completedFiles,
    required this.totalFiles,
    this.activeFileIndex,
    this.activeFileBytesTransferred,
    this.speedLabel,
    this.etaLabel,
    this.connectionPath,
  });

  final BigInt bytesTransferred;
  final BigInt totalBytes;
  final int completedFiles;
  final int totalFiles;
  final int? activeFileIndex;
  final BigInt? activeFileBytesTransferred;
  final String? speedLabel;
  final String? etaLabel;
  final ConnectionPathInfo? connectionPath;

  double get progressFraction {
    if (totalBytes == BigInt.zero) {
      return 0;
    }

    final transferred = bytesTransferred.toDouble();
    final total = totalBytes.toDouble();
    return transferred / total;
  }
}

@immutable
class TransferTransferResult {
  const TransferTransferResult({
    required this.bytesTransferred,
    required this.totalBytes,
    required this.completedFiles,
    required this.totalFiles,
    this.duration,
    this.averageSpeedLabel,
  });

  final BigInt bytesTransferred;
  final BigInt totalBytes;
  final int completedFiles;
  final int totalFiles;
  final Duration? duration;
  final String? averageSpeedLabel;
}

@immutable
class TransferIncomingOffer {
  const TransferIncomingOffer({
    required this.sender,
    required this.manifest,
    required this.destinationLabel,
    required this.saveRootLabel,
    required this.statusMessage,
    required this.bytesReceived,
    this.connectionPath,
    this.senderEndpointId,
    this.inlineText,
  });

  final TransferIdentity sender;
  final TransferManifest manifest;
  final String destinationLabel;
  final String saveRootLabel;
  final String statusMessage;
  final BigInt bytesReceived;
  final ConnectionPathInfo? connectionPath;
  final String? senderEndpointId;

  /// Plain text for a text-only offer — rendered with Copy / Save-as-.txt
  /// actions instead of a file manifest. `null` for ordinary file offers.
  final String? inlineText;

  String get displaySenderName => sender.displayName;
  bool get willResume => bytesReceived > BigInt.zero;
  bool get isTextOffer => inlineText != null;

  TransferIncomingOffer copyWith({
    ConnectionPathInfo? connectionPath,
    String? senderEndpointId,
  }) {
    return TransferIncomingOffer(
      sender: sender,
      manifest: manifest,
      destinationLabel: destinationLabel,
      saveRootLabel: saveRootLabel,
      statusMessage: statusMessage,
      bytesReceived: bytesReceived,
      connectionPath: connectionPath ?? this.connectionPath,
      senderEndpointId: senderEndpointId ?? this.senderEndpointId,
      inlineText: inlineText,
    );
  }
}

@immutable
class TransferSessionState {
  const TransferSessionState._({
    required this.phase,
    required this.offer,
    required this.progress,
    required this.result,
    required this.errorMessage,
    this.errorTitle,
    this.errorRecovery,
    this.savedText,
  });

  const TransferSessionState.idle()
    : this._(
        phase: TransferSessionPhase.idle,
        offer: null,
        progress: null,
        result: null,
        errorMessage: null,
      );

  const TransferSessionState.connecting({required TransferIncomingOffer offer})
    : this._(
        phase: TransferSessionPhase.connecting,
        offer: offer,
        progress: null,
        result: null,
        errorMessage: null,
      );

  const TransferSessionState.offerPending({
    required TransferIncomingOffer offer,
  }) : this._(
         phase: TransferSessionPhase.offerPending,
         offer: offer,
         progress: null,
         result: null,
         errorMessage: null,
       );

  const TransferSessionState.receiving({
    required TransferIncomingOffer offer,
    required TransferTransferProgress progress,
  }) : this._(
         phase: TransferSessionPhase.receiving,
         offer: offer,
         progress: progress,
         result: null,
         errorMessage: null,
       );

  const TransferSessionState.completed({
    required TransferIncomingOffer offer,
    required TransferTransferResult result,
    SavedTextLocation? savedText,
  }) : this._(
         phase: TransferSessionPhase.completed,
         offer: offer,
         progress: null,
         result: result,
         errorMessage: null,
         savedText: savedText,
       );

  const TransferSessionState.cancelled({
    required TransferIncomingOffer offer,
    required String errorMessage,
  }) : this._(
         phase: TransferSessionPhase.cancelled,
         offer: offer,
         progress: null,
         result: null,
         errorMessage: errorMessage,
       );

  const TransferSessionState.failed({
    required TransferIncomingOffer offer,
    required String errorMessage,
    String? errorTitle,
    String? errorRecovery,
  }) : this._(
         phase: TransferSessionPhase.failed,
         offer: offer,
         progress: null,
         result: null,
         errorMessage: errorMessage,
         errorTitle: errorTitle,
         errorRecovery: errorRecovery,
       );

  final TransferSessionPhase phase;
  final TransferIncomingOffer? offer;
  final TransferTransferProgress? progress;
  final TransferTransferResult? result;
  final String? errorMessage;

  /// Failure title (e.g. "Incompatible version") and an optional actionable
  /// recovery hint, carried alongside [errorMessage] so the finish screen can
  /// show the same title + message + guide layout the sender uses.
  final String? errorTitle;
  final String? errorRecovery;

  /// Set only on the completed state of a "Save .txt" inline-text receive.
  final SavedTextLocation? savedText;

  bool get hasOffer => offer != null;
  bool get hasIncomingOffer => hasOffer;

  TransferIncomingOffer? get incomingOffer => offer;
}
