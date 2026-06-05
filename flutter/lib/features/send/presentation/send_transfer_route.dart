import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../../theme/wisp_theme.dart';
import '../../../app/app_router.dart';
import '../../transfers/application/manifest.dart';
import '../../transfers/application/state.dart' as transfer_state;
import '../../transfers/presentation/widgets/sending_connection_strip.dart';
import '../../transfers/presentation/widgets/transfer_flow_layout.dart';
import '../../transfers/presentation/widgets/transfer_presentation_helpers.dart';
import '../../transfers/presentation/widgets/transfer_manifest_panel.dart';
import '../application/controller.dart';
import '../application/model.dart';
import '../application/state.dart';
import '../application/transfer_state.dart';
import 'send_transfer_view_data.dart';
import 'package:app/features/send/presentation/widgets/recipient_avatar.dart';

class SendTransferRoutePage extends ConsumerStatefulWidget {
  const SendTransferRoutePage({super.key, required this.request});

  final SendRequestData request;

  @override
  ConsumerState<SendTransferRoutePage> createState() =>
      _SendTransferRoutePageState();
}

class _SendTransferRoutePageState extends ConsumerState<SendTransferRoutePage> {
  final bool _allowPop = false;

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted) {
        ref.read(sendControllerProvider.notifier).startTransfer(widget.request);
      }
    });
  }

  @override
  Widget build(BuildContext context) {
    final state = ref.watch(sendControllerProvider);
    final controller = ref.read(sendControllerProvider.notifier);
    final viewData = buildSendTransferPageData(
      state: state,
      request: widget.request,
    );

    // "Done" path — clear the draft for terminal results, then home.  Used
    // for both success (only button shown) and failure (left button next to
    // "Retry") so the user always has an explicit way back to home from the
    // result card.
    void exitRoute() {
      if (!mounted) {
        return;
      }

      final currentState = ref.read(sendControllerProvider);
      if (currentState is SendStateResult) {
        ref.read(sendControllerProvider.notifier).clearDraft();
      }

      context.goHome();
    }

    // "Retry" path — only offered for failed/cancelled/declined results.
    // Restores the SendStateResult back to SendStateDrafting (same files,
    // same destination, same resolved sizes) and navigates to the draft
    // preview so the user can adjust destination + re-tap Send without
    // re-picking files.  No-op on a state that isn't a failure result.
    void retryFromResult() {
      if (!mounted) {
        return;
      }
      final currentState = ref.read(sendControllerProvider);
      if (currentState is! SendStateResult) {
        return;
      }
      if (currentState.result.outcome == SendTransferOutcome.success) {
        // Defensive: Retry shouldn't be reachable for success, but if a
        // race surfaces it, fall back to the success exit so we don't
        // strand the user on the result screen.
        exitRoute();
        return;
      }
      ref.read(sendControllerProvider.notifier).restoreDraftFromResult();
      // Pass empty `files` so the draft route builder uses the
      // controller state we just restored instead of seeding from
      // `extra` (which would replace the items via `beginDraft`).
      context.goSendDraft(files: const []);
    }

    return PopScope(
      canPop: _allowPop,
      onPopInvokedWithResult: (didPop, _) {
        if (!didPop) {
          return;
        }

        final currentState = ref.read(sendControllerProvider);
        switch (currentState) {
          case SendStateTransferring():
            controller.cancelTransfer();
          case SendStateResult():
            controller.clearDraft();
          case SendStateIdle() || SendStateDrafting():
            break;
        }
      },
      child: Scaffold(
        backgroundColor: kBg,
        body: SafeArea(
          child: SizedBox.expand(
            child: AnimatedSwitcher(
              duration: const Duration(milliseconds: 400),
              switchInCurve: Curves.easeOut,
              switchOutCurve: Curves.easeIn,
              child: _TransferStateCard(
                key: ValueKey(state.runtimeType),
                state: state,
                viewData: viewData,
                onExit: exitRoute,
                onRetry: retryFromResult,
              ),
            ),
          ),
        ),
      ),
    );
  }
}

class _TransferStateCard extends StatelessWidget {
  const _TransferStateCard({
    super.key,
    required this.state,
    required this.viewData,
    required this.onExit,
    required this.onRetry,
  });

  final SendState state;
  final SendTransferPageData viewData;
  final VoidCallback onExit;
  final VoidCallback onRetry;

  @override
  Widget build(BuildContext context) {
    final accent = viewData.visual.accentColor;
    final transfer = switch (state) {
      SendStateTransferring(:final transfer) => transfer,
      SendStateResult(:final transfer) => transfer,
      _ => null,
    };
    final primary = Theme.of(context).colorScheme.primary;
    final isSuccessResult =
        state is SendStateResult &&
        viewData.visual.statusLabel.toLowerCase().trim() == 'success';

    final progress = _buildSharedTransferProgress(
      transfer,
      viewData.files.length,
    );

    final showFooterButton =
        (state is SendStateTransferring &&
            (progress?.progressFraction ?? 0.0) < 1.0) ||
        state is SendStateResult;
    final manifestItems = viewData.files
        .map(
          (file) =>
              TransferManifestItem(path: file.path, sizeBytes: file.sizeBytes),
        )
        .toList(growable: false);
    final manifestMode = switch (state) {
      SendStateTransferring(:final transfer)
          when transfer.phase == SendTransferPhase.sending =>
        TransferManifestPanelMode.liveList,
      SendStateResult() => TransferManifestPanelMode.liveList,
      SendStateTransferring() => TransferManifestPanelMode.previewTree,
      _ => TransferManifestPanelMode.previewTree,
    };
    final stripMode =
        viewData.stripMode ??
        (isSuccessResult
            ? SendingStripMode.transferring
            : SendingStripMode.waitingOnRecipient);

    final Widget subtitle;
    if (progress != null && state is SendStateTransferring) {
      subtitle = buildSpeedLine(
        speedLabel: progress.speedLabel ?? '',
        etaLabel: progress.etaLabel,
      );
    } else {
      subtitle = buildSubtitleText(viewData.visual.subtitle);
    }

    return TransferFlowLayout(
      statusLabel: viewData.visual.statusLabel,
      statusColor: accent,
      subtitle: subtitle,
      explainer: isSuccessResult ? _SendStatsGrid(viewData: viewData) : null,
      illustration: RecipientAvatar(
        deviceName: viewData.remoteLabel,
        deviceType: viewData.remoteDeviceType ?? 'phone',
        mode: stripMode,
        progress: (viewData.progressFraction ?? 0.0).clamp(0.0, 1.0),
        animate: viewData.visual.showSpinner,
        connectionPath: viewData.connectionPath,
        countdownDuration: viewData.countdownDuration,
      ),
      manifest: manifestItems.isEmpty
          ? null
          : TransferManifestPanel(
              mode: manifestMode,
              items: manifestItems,
              progress: progress,
              initiallyExpanded: state is SendStateResult,
            ),
      footer: _buildFooter(
        state: state,
        showFooterButton: showFooterButton,
        isSuccessResult: isSuccessResult,
        primary: primary,
        accent: accent,
        onExit: onExit,
        onRetry: onRetry,
      ),
    );
  }
}

/// Footer button layout for the transfer result / in-flight card.
///
/// Three branches in priority order:
/// 1. **Active transfer not yet at 100%** → one red "Cancel transfer" button
///    (TextButton with red tint).
/// 2. **Success result** → one "Done" button (primary color) → home.
/// 3. **Failed / cancelled / declined result** → two buttons side-by-side:
///    left "Done" (outlined / secondary, back to home, clears draft) and
///    right "Retry" (filled, accent color, restores the draft and pushes
///    /send/draft so the user can immediately re-send the same files).
/// 4. **Anything else** (e.g. early connecting phase) → no footer button.
Widget _buildFooter({
  required SendState state,
  required bool showFooterButton,
  required bool isSuccessResult,
  required Color primary,
  required Color accent,
  required VoidCallback onExit,
  required VoidCallback onRetry,
}) {
  if (!showFooterButton) {
    return const SizedBox.shrink();
  }

  if (state is SendStateTransferring) {
    return Row(
      children: [
        Expanded(
          child: TextButton(
            onPressed: onExit,
            style: TextButton.styleFrom(
              foregroundColor: const Color(0xFFB34A4A),
              backgroundColor: const Color(0xFFB34A4A).withValues(alpha: 0.08),
              minimumSize: const Size(0, 48),
              shape: RoundedRectangleBorder(
                borderRadius: BorderRadius.circular(12),
                side: BorderSide(
                  color: const Color(0xFFB34A4A).withValues(alpha: 0.15),
                ),
              ),
            ),
            child: const Text('Cancel transfer'),
          ),
        ),
      ],
    );
  }

  if (state is! SendStateResult) {
    return const SizedBox.shrink();
  }

  if (isSuccessResult) {
    return Row(
      children: [
        Expanded(
          child: FilledButton(
            onPressed: onExit,
            style: FilledButton.styleFrom(
              backgroundColor: primary,
              foregroundColor: Colors.white,
              minimumSize: const Size(0, 48),
              shape: RoundedRectangleBorder(
                borderRadius: BorderRadius.circular(12),
              ),
            ),
            child: const Text('Done'),
          ),
        ),
      ],
    );
  }

  // Failure result: side-by-side "Done" + "Retry".  "Retry" is the visually
  // dominant action (FilledButton in the accent color) because re-sending
  // the same files is the more likely user intent after a failure; "Done"
  // (OutlinedButton) is the escape hatch back to home.
  return Row(
    children: [
      Expanded(
        child: OutlinedButton(
          onPressed: onExit,
          style: OutlinedButton.styleFrom(
            foregroundColor: kInk,
            minimumSize: const Size(0, 48),
            shape: RoundedRectangleBorder(
              borderRadius: BorderRadius.circular(12),
            ),
            side: BorderSide(color: kBorder),
          ),
          child: const Text('Done'),
        ),
      ),
      const SizedBox(width: 12),
      Expanded(
        child: FilledButton(
          onPressed: onRetry,
          style: FilledButton.styleFrom(
            backgroundColor: accent,
            foregroundColor: Colors.white,
            minimumSize: const Size(0, 48),
            shape: RoundedRectangleBorder(
              borderRadius: BorderRadius.circular(12),
            ),
          ),
          child: const Text('Retry'),
        ),
      ),
    ],
  );
}

class _SendStatsGrid extends StatelessWidget {
  const _SendStatsGrid({required this.viewData});

  final SendTransferPageData viewData;

  @override
  Widget build(BuildContext context) {
    // Only show stats if we have at least one valid metric
    final displayStats = <_StatItem>[
      if (viewData.totalSizeLabel != null)
        _StatItem(label: 'SIZE', value: viewData.totalSizeLabel!),
      if (viewData.durationLabel != null)
        _StatItem(label: 'TIME', value: viewData.durationLabel!),
      if (viewData.averageSpeedLabel != null)
        _StatItem(label: 'SPEED', value: viewData.averageSpeedLabel!),
    ];

    if (displayStats.isEmpty) return const SizedBox.shrink();

    return Container(
      margin: const EdgeInsets.only(top: 8),
      padding: const EdgeInsets.symmetric(vertical: 12, horizontal: 16),
      decoration: BoxDecoration(
        color: kFill.withValues(alpha: 0.4),
        borderRadius: BorderRadius.circular(12),
      ),
      child: TweenAnimationBuilder<double>(
        tween: Tween(begin: 0.0, end: 1.0),
        duration: const Duration(milliseconds: 600),
        curve: Curves.easeOutCubic,
        builder: (context, value, child) {
          return Opacity(
            opacity: value,
            child: Transform.translate(
              offset: Offset(0, 10 * (1 - value)),
              child: child,
            ),
          );
        },
        child: Row(
          mainAxisAlignment: MainAxisAlignment.spaceAround,
          children: [
            for (int i = 0; i < displayStats.length; i++) ...[
              if (i > 0)
                Container(
                  width: 1,
                  height: 24,
                  color: kBorder.withValues(alpha: 0.5),
                ),
              Expanded(
                child: Column(
                  children: [
                    Text(
                      displayStats[i].label,
                      style: wispSans(
                        fontSize: 9,
                        fontWeight: FontWeight.w800,
                        color: kMuted,
                        letterSpacing: 0.5,
                      ),
                    ),
                    const SizedBox(height: 2),
                    Text(
                      displayStats[i].value,
                      style: wispSans(
                        fontSize: 13,
                        fontWeight: FontWeight.w700,
                        color: kInk,
                      ),
                    ),
                  ],
                ),
              ),
            ],
          ],
        ),
      ),
    );
  }
}

class _StatItem {
  const _StatItem({required this.label, required this.value});
  final String label;
  final String value;
}

transfer_state.TransferTransferProgress? _buildSharedTransferProgress(
  SendTransferState? transfer,
  int fallbackFileCount,
) {
  if (transfer == null || transfer.totalBytes == BigInt.zero) {
    return null;
  }

  final snapshot = transfer.snapshot;
  return transfer_state.TransferTransferProgress(
    bytesTransferred: transfer.bytesSent,
    totalBytes: transfer.totalBytes,
    completedFiles: snapshot?.completedFiles ?? 0,
    totalFiles: snapshot?.totalFiles ?? fallbackFileCount,
    activeFileIndex: snapshot?.activeFileId,
    activeFileBytesTransferred: snapshot?.activeFileBytes,
    speedLabel: viewSpeedLabel(transfer),
    etaLabel: viewEtaLabel(transfer),
  );
}

String? viewSpeedLabel(SendTransferState transfer) {
  final speed = transfer.snapshot?.bytesPerSec;
  if (speed == null || speed <= BigInt.zero) {
    return null;
  }
  return '${formatBytes(speed)}/s';
}

String? viewEtaLabel(SendTransferState transfer) {
  final eta = transfer.snapshot?.etaSeconds;
  if (eta == null || eta <= BigInt.zero) {
    return null;
  }

  return formatEta(eta);
}
