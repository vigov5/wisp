import 'package:flutter/material.dart';

import '../../application/manifest.dart';
import '../../application/state.dart';
import 'active_transfer_file_list.dart';
import 'manifest_tree_card.dart';

enum TransferManifestPanelMode { previewTree, liveList }

class TransferManifestPanel extends StatelessWidget {
  const TransferManifestPanel({
    super.key,
    required this.mode,
    required this.items,
    this.progress,
    this.initiallyExpanded = false,
    this.allComplete = false,
    this.onOpenFile,
  });

  final TransferManifestPanelMode mode;
  final List<TransferManifestItem> items;
  final TransferTransferProgress? progress;
  final bool initiallyExpanded;

  /// Forwarded to [ActiveTransferFileList] (liveList mode) to show success
  /// ticks on every file when there's no live progress stream.
  final bool allComplete;

  /// Forwarded to [ActiveTransferFileList] (liveList mode): per-file "open"
  /// buttons on the receive finish screen. `null` (no buttons) in previewTree
  /// mode and everywhere off the receive finish screen.
  final void Function(TransferManifestItem item)? onOpenFile;

  @override
  Widget build(BuildContext context) {
    return switch (mode) {
      TransferManifestPanelMode.previewTree => ManifestTreeCard(
        items: items,
        initiallyExpanded: initiallyExpanded,
      ),
      TransferManifestPanelMode.liveList => ActiveTransferFileList(
        items: items,
        progress: progress,
        initiallyExpanded: initiallyExpanded,
        allComplete: allComplete,
        onOpenFile: onOpenFile,
      ),
    };
  }
}
