import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../../theme/wisp_theme.dart';
import '../../../app/app_router.dart';
import '../../transfers/application/manifest.dart';
import '../../transfers/application/state.dart' as transfer_state;
import '../../transfers/presentation/widgets/relay_tip_note.dart';
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

  /// Auto-return countdown for a *successful* send. The result card has no
  /// follow-up action on the sender side (nothing to open locally), so it
  /// returns home on its own after [_autoCloseSeconds] rather than stranding
  /// the user on a terminal screen. Sender success only — failures keep a
  /// manual Done so the reason stays put, and the receiver side never
  /// auto-closes (its files are still waiting to be opened). Any pointer-down
  /// on the card cancels it (see the Listener in build), so it only fires when
  /// the user has actually walked away. `null` timer ⇒ not counting.
  static const int _autoCloseSeconds = 10;
  Timer? _autoCloseTimer;
  int _autoCloseRemaining = 0;

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
  void dispose() {
    _autoCloseTimer?.cancel();
    super.dispose();
  }

  // "Done" path — clear the draft for terminal results, then home.  Used for
  // both success (only button shown) and failure (left button next to "Retry")
  // so the user always has an explicit way back to home from the result card.
  // Also the target the success auto-close timer fires into.
  void _exitRoute() {
    if (!mounted) {
      return;
    }
    _autoCloseTimer?.cancel();
    _autoCloseTimer = null;

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
  void _retryFromResult() {
    if (!mounted) {
      return;
    }
    _cancelAutoClose();
    final currentState = ref.read(sendControllerProvider);
    if (currentState is! SendStateResult) {
      return;
    }
    if (currentState.result.outcome == SendTransferOutcome.success) {
      // Defensive: Retry shouldn't be reachable for success, but if a
      // race surfaces it, fall back to the success exit so we don't
      // strand the user on the result screen.
      _exitRoute();
      return;
    }
    ref.read(sendControllerProvider.notifier).restoreDraftFromResult();
    // Pass empty `files` so the draft route builder uses the
    // controller state we just restored instead of seeding from
    // `extra` (which would replace the items via `beginDraft`).
    context.goSendDraft(files: const []);
  }

  // "Back" path — shown alongside Cancel only while still connecting (no data
  // sent yet). Aborts the in-flight connect and rolls the transfer back into
  // the draft (cancelTransfer already does that state transition), then
  // returns to the draft screen so the user can pick a different connect
  // method or change the files. Empty `files` => reuse the restored draft.
  void _backToDraft() {
    if (!mounted) {
      return;
    }
    _cancelAutoClose();
    ref.read(sendControllerProvider.notifier).cancelTransfer();
    context.goSendDraft(files: const []);
  }

  // Start the success auto-return. Idempotent: a repeat success signal while
  // already counting is ignored so the clock never restarts mid-countdown.
  void _startAutoClose() {
    if (_autoCloseTimer != null) {
      return;
    }
    setState(() => _autoCloseRemaining = _autoCloseSeconds);
    _autoCloseTimer = Timer.periodic(const Duration(seconds: 1), (timer) {
      if (!mounted) {
        timer.cancel();
        return;
      }
      if (_autoCloseRemaining <= 1) {
        timer.cancel();
        _autoCloseTimer = null;
        _exitRoute();
      } else {
        setState(() => _autoCloseRemaining -= 1);
      }
    });
  }

  // Stop the countdown and revert the button to a plain "Done". Called on any
  // pointer interaction with the card so the screen never vanishes out from
  // under someone who's still reading it.
  void _cancelAutoClose() {
    if (_autoCloseTimer == null) {
      return;
    }
    _autoCloseTimer!.cancel();
    _autoCloseTimer = null;
    setState(() => _autoCloseRemaining = 0);
  }

  @override
  Widget build(BuildContext context) {
    final state = ref.watch(sendControllerProvider);
    final controller = ref.read(sendControllerProvider.notifier);
    final viewData = buildSendTransferPageData(
      state: state,
      request: widget.request,
    );

    // Arm the auto-return when the send reaches a *successful* terminal state;
    // any other outcome cancels it. Registered every build so the
    // transferring→result transition (which triggers this rebuild) fires it.
    ref.listen<SendState>(sendControllerProvider, (prev, next) {
      final reachedSuccess =
          next is SendStateResult &&
          next.result.outcome == SendTransferOutcome.success;
      if (reachedSuccess) {
        _startAutoClose();
      } else {
        _cancelAutoClose();
      }
    });

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
        backgroundColor: context.wc.bg,
        body: SafeArea(
          // A pointer-down anywhere on the card cancels the success auto-close
          // (translucent so blank areas count too). Listener never consumes the
          // event, so buttons and scrolling keep working.
          child: Listener(
            behavior: HitTestBehavior.translucent,
            onPointerDown: (_) => _cancelAutoClose(),
            child: SizedBox.expand(
              child: AnimatedSwitcher(
                duration: const Duration(milliseconds: 400),
                switchInCurve: Curves.easeOut,
                switchOutCurve: Curves.easeIn,
                child: _TransferStateCard(
                  key: ValueKey(state.runtimeType),
                  state: state,
                  viewData: viewData,
                  onExit: _exitRoute,
                  onRetry: _retryFromResult,
                  onBack: _backToDraft,
                  autoCloseRemaining: _autoCloseTimer != null
                      ? _autoCloseRemaining
                      : null,
                ),
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
    required this.onBack,
    required this.autoCloseRemaining,
  });

  final SendState state;
  final SendTransferPageData viewData;
  final VoidCallback onExit;
  final VoidCallback onRetry;
  final VoidCallback onBack;

  /// Seconds left on the success auto-return, or `null` when not counting.
  /// Drives the "Done (n)" label; `null` renders a plain "Done".
  final int? autoCloseRemaining;

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
        connectionCandidates: viewData.connectionCandidates,
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
      footerNote: state is SendStateTransferring
          ? RelayTipNote(path: viewData.connectionPath)
          : null,
      footer: _buildFooter(
        context: context,
        state: state,
        showFooterButton: showFooterButton,
        isSuccessResult: isSuccessResult,
        primary: primary,
        accent: accent,
        onExit: onExit,
        onRetry: onRetry,
        onBack: onBack,
        autoCloseRemaining: autoCloseRemaining,
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
  required BuildContext context,
  required SendState state,
  required bool showFooterButton,
  required bool isSuccessResult,
  required Color primary,
  required Color accent,
  required VoidCallback onExit,
  required VoidCallback onRetry,
  required VoidCallback onBack,
  required int? autoCloseRemaining,
}) {
  if (!showFooterButton) {
    return const SizedBox.shrink();
  }

  if (state is SendStateTransferring) {
    final cancelButton = TextButton(
      onPressed: onExit,
      style: TextButton.styleFrom(
        foregroundColor: kDanger,
        backgroundColor: kDanger.withValues(alpha: 0.08),
        minimumSize: const Size(0, 48),
        shape: RoundedRectangleBorder(
          borderRadius: BorderRadius.circular(12),
          side: BorderSide(color: kDanger.withValues(alpha: 0.15)),
        ),
      ),
      child: const Text('Cancel transfer'),
    );

    // Still connecting (no bytes sent yet): offer a lighter "Back" alongside
    // Cancel so the user can return to the draft and pick a different connect
    // method / files instead of aborting to home. Back takes 1/3, Cancel keeps
    // the dominant 2/3 as the definitive escape.
    if (state.transfer.phase == SendTransferPhase.connecting) {
      return Row(
        children: [
          Expanded(
            flex: 1,
            child: OutlinedButton(
              onPressed: onBack,
              style: OutlinedButton.styleFrom(
                foregroundColor: context.wc.ink,
                minimumSize: const Size(0, 48),
                shape: RoundedRectangleBorder(
                  borderRadius: BorderRadius.circular(12),
                ),
                side: BorderSide(color: context.wc.border),
              ),
              child: const Text('Back'),
            ),
          ),
          const SizedBox(width: 12),
          Expanded(flex: 2, child: cancelButton),
        ],
      );
    }

    return Row(children: [Expanded(child: cancelButton)]);
  }

  if (state is! SendStateResult) {
    return const SizedBox.shrink();
  }

  if (isSuccessResult) {
    // Counting down → "Done (n)" so the auto-return is visible and tappable to
    // leave immediately; cancelled → plain "Done".
    final doneLabel = autoCloseRemaining != null
        ? 'Done ($autoCloseRemaining)'
        : 'Done';
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
            child: Text(doneLabel),
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
            foregroundColor: context.wc.ink,
            minimumSize: const Size(0, 48),
            shape: RoundedRectangleBorder(
              borderRadius: BorderRadius.circular(12),
            ),
            side: BorderSide(color: context.wc.border),
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
        color: context.wc.fill.withValues(alpha: 0.4),
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
                  color: context.wc.border.withValues(alpha: 0.5),
                ),
              Expanded(
                child: Column(
                  children: [
                    Text(
                      displayStats[i].label,
                      style: wispSans(
                        fontSize: 9,
                        fontWeight: FontWeight.w800,
                        color: context.wc.muted,
                        letterSpacing: 0.5,
                      ),
                    ),
                    const SizedBox(height: 2),
                    Text(
                      displayStats[i].value,
                      style: wispSans(
                        fontSize: 13,
                        fontWeight: FontWeight.w700,
                        color: context.wc.ink,
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
