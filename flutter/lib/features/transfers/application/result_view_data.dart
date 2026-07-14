import 'package:flutter/foundation.dart';

import 'format_utils.dart';
import 'identity.dart';
import 'manifest.dart';
import 'state.dart';

enum TransferResultOutcome { success, cancelled, failed }

@immutable
class ResultMetric {
  const ResultMetric({required this.label, required this.value});

  final String label;
  final String value;
}

@immutable
class TransferResultViewData {
  const TransferResultViewData({
    required this.outcome,
    required this.title,
    required this.message,
    this.metrics,
    this.primaryLabel = 'Done',
    required this.deviceName,
    this.deviceType,
    this.web = false,
    this.manifestItems,
    this.durationLabel,
    this.averageSpeedLabel,
    this.totalSizeLabel,
    this.fileCountLabel,
  });

  final TransferResultOutcome outcome;
  final String title;
  final String message;
  final List<ResultMetric>? metrics;
  final String primaryLabel;
  final String deviceName;
  final DeviceType? deviceType;

  /// True when the sender is a browser (web) peer — the finish card shows a
  /// globe instead of the [deviceType] laptop/phone glyph.
  final bool web;
  final List<TransferManifestItem>? manifestItems;

  // High-level summary stats
  final String? durationLabel;
  final String? averageSpeedLabel;
  final String? totalSizeLabel;
  final String? fileCountLabel;
}

TransferResultViewData buildTransferResultViewData(TransferSessionState state) {
  final offer = state.incomingOffer;
  if (offer == null) {
    throw StateError('transfer result view data requires an incoming offer');
  }

  final deviceName = _displaySender(offer.sender.deviceName);
  final deviceType = offer.sender.deviceType;
  final web = offer.sender.web;
  final manifestItems = offer.manifest.items;
  final savedText = state.savedText;

  return switch (state.phase) {
    // Inline text never went through the blob pipeline, so the file-centric
    // stats (count, size, speed, manifest tree) would all read as noise. Show
    // a minimal "Text saved" card spelling out the exact name + folder so the
    // user knows where it landed (the "Open folder" button opens that place).
    TransferSessionPhase.completed when offer.isTextOffer =>
      TransferResultViewData(
        outcome: TransferResultOutcome.success,
        title: 'Text saved',
        message: savedText != null
            ? 'Saved ${savedText.fileName} to ${savedText.folderLabel}.'
            : 'Saved as a .txt file.',
        deviceName: deviceName,
        deviceType: deviceType,
        web: web,
        metrics: [
          ResultMetric(label: 'From', value: deviceName),
          if (savedText != null)
            ResultMetric(label: 'File', value: savedText.fileName),
          if (savedText != null)
            ResultMetric(label: 'Folder', value: savedText.folderLabel),
        ],
      ),
    TransferSessionPhase.completed => TransferResultViewData(
      outcome: TransferResultOutcome.success,
      title: 'Files saved',
      // The Rust side reports the cache-relative root which on Android always
      // looks like "Downloads", even when the user actually picked a SAF
      // folder of their own. Keep the success message generic — the "Open
      // folder" button shows the real location.
      message: 'Transfer complete.',
      deviceName: deviceName,
      deviceType: deviceType,
      web: web,
      manifestItems: manifestItems,
      durationLabel: _formatDuration(state.result?.duration),
      averageSpeedLabel: state.result?.averageSpeedLabel,
      totalSizeLabel: formatBytes(state.result?.totalBytes ?? BigInt.zero),
      fileCountLabel: '${state.result?.completedFiles ?? 0} files',
      metrics: [
        ResultMetric(label: 'From', value: deviceName),
        ResultMetric(label: 'Files', value: '${state.result!.completedFiles}'),
        ResultMetric(
          label: 'Size',
          value: formatBytes(state.result!.totalBytes),
        ),
      ],
    ),
    TransferSessionPhase.cancelled => TransferResultViewData(
      outcome: TransferResultOutcome.cancelled,
      title: 'Receive cancelled',
      message:
          state.errorMessage ??
          'Wisp stopped receiving before all files were saved.',
      deviceName: deviceName,
      deviceType: deviceType,
      web: web,
      manifestItems: manifestItems,
    ),
    TransferSessionPhase.failed => TransferResultViewData(
      outcome: TransferResultOutcome.failed,
      title: 'Couldn\'t finish receiving files',
      message: state.errorMessage ?? 'Couldn\'t finish receiving files.',
      deviceName: deviceName,
      deviceType: deviceType,
      web: web,
      manifestItems: manifestItems,
    ),
    _ => throw StateError(
      'transfer result view data requires a terminal state',
    ),
  };
}

String _displaySender(String value) {
  final trimmed = value.trim();
  return trimmed.isEmpty ? 'Unknown sender' : trimmed;
}

String? _formatDuration(Duration? duration) {
  if (duration == null) return null;
  if (duration.inSeconds < 60) {
    return '${duration.inSeconds}s';
  }
  return '${duration.inMinutes}m ${duration.inSeconds % 60}s';
}
