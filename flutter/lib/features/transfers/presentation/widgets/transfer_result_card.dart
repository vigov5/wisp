import 'package:flutter/material.dart';

import 'package:app/theme/wisp_theme.dart';
import 'package:app/features/send/presentation/widgets/recipient_avatar.dart';
import 'package:app/features/transfers/application/manifest.dart';
import 'package:app/features/transfers/application/result_view_data.dart';
import 'package:app/features/transfers/presentation/widgets/transfer_manifest_panel.dart';
import 'sending_connection_strip.dart';
import 'transfer_flow_layout.dart';
import 'transfer_presentation_helpers.dart';

class TransferResultCard extends StatelessWidget {
  const TransferResultCard({
    super.key,
    required this.viewData,
    this.onPrimary,
    this.onSecondary,
    this.secondaryLabel,
    this.onOpenFile,
  });

  final TransferResultViewData viewData;
  final VoidCallback? onPrimary;
  final VoidCallback? onSecondary;
  final String? secondaryLabel;

  /// Per-file "open" callback for the manifest list. Non-null only on a
  /// successful receive, where every file is on disk and can be opened.
  final void Function(TransferManifestItem item)? onOpenFile;

  @override
  Widget build(BuildContext context) {
    final visual = _visualForOutcome(viewData.outcome);

    return SizedBox.expand(
      child: TransferFlowLayout(
        statusLabel: visual.statusLabel,
        statusColor: visual.accentColor,
        subtitle: buildSubtitleText(viewData.message),
        explainer: _StatsGrid(viewData: viewData),
        illustration: RecipientAvatar(
          deviceName: viewData.deviceName,
          deviceType: viewData.web
              ? 'web'
              : viewData.deviceType != null
              ? deviceTypeLabel(viewData.deviceType!)
              : 'laptop',
          mode: SendingStripMode.transferring,
          progress: viewData.outcome == TransferResultOutcome.success
              ? 1.0
              : 0.0,
          animate: false,
        ),
        manifest:
            viewData.manifestItems == null || viewData.manifestItems!.isEmpty
            ? null
            : TransferManifestPanel(
                mode: TransferManifestPanelMode.liveList,
                items: viewData.manifestItems!,
                initiallyExpanded: true,
                // On a successful receive every file is saved — show the same
                // success ticks the sender shows. Cancelled/failed keep the
                // neutral file icons (not all files made it).
                allComplete: viewData.outcome == TransferResultOutcome.success,
                // Only a successful receive has every file on disk to open.
                onOpenFile: viewData.outcome == TransferResultOutcome.success
                    ? onOpenFile
                    : null,
              ),
        footer: Row(
          children: [
            if (secondaryLabel != null && onSecondary != null) ...[
              Expanded(
                child: OutlinedButton(
                  onPressed: onSecondary,
                  style: OutlinedButton.styleFrom(
                    // Cyan tint so "Show in Files" reads as a real action
                    // rather than a washed-out outline. Same soft-tint formula
                    // as the red Cancel/Decline buttons (bg @0.08, border
                    // @0.15), just in the accent colour.
                    foregroundColor: kAccentCyanStrong,
                    backgroundColor: kAccentCyan.withValues(alpha: 0.08),
                    side: BorderSide(
                      color: kAccentCyan.withValues(alpha: 0.15),
                    ),
                    minimumSize: const Size(0, 52),
                    shape: RoundedRectangleBorder(
                      borderRadius: BorderRadius.circular(14),
                    ),
                  ),
                  child: Text(
                    secondaryLabel!,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: wispSans(fontWeight: FontWeight.w700, fontSize: 15),
                  ),
                ),
              ),
              const SizedBox(width: 12),
            ],
            if (viewData.primaryLabel.isNotEmpty && onPrimary != null)
              Expanded(
                child: FilledButton(
                  onPressed: onPrimary,
                  style: FilledButton.styleFrom(
                    backgroundColor: visual.buttonColor,
                    foregroundColor: Colors.white,
                    minimumSize: const Size(0, 52),
                    shape: RoundedRectangleBorder(
                      borderRadius: BorderRadius.circular(14),
                    ),
                    elevation: 0,
                  ),
                  child: Text(
                    viewData.primaryLabel,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: wispSans(fontWeight: FontWeight.w700, fontSize: 15),
                  ),
                ),
              ),
          ],
        ),
      ),
    );
  }
}

class _StatsGrid extends StatelessWidget {
  const _StatsGrid({required this.viewData});

  final TransferResultViewData viewData;

  @override
  Widget build(BuildContext context) {
    if (viewData.outcome != TransferResultOutcome.success) {
      return const SizedBox.shrink();
    }

    final stats = <_StatItem>[
      if (viewData.totalSizeLabel != null)
        _StatItem(label: 'SIZE', value: viewData.totalSizeLabel!),
      if (viewData.durationLabel != null)
        _StatItem(label: 'TIME', value: viewData.durationLabel!),
      if (viewData.averageSpeedLabel != null)
        _StatItem(label: 'SPEED', value: viewData.averageSpeedLabel!),
    ];

    if (stats.isEmpty) return const SizedBox.shrink();

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
            for (int i = 0; i < stats.length; i++) ...[
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
                      stats[i].label,
                      style: wispSans(
                        fontSize: 9,
                        fontWeight: FontWeight.w800,
                        color: context.wc.muted,
                        letterSpacing: 0.5,
                      ),
                    ),
                    const SizedBox(height: 2),
                    Text(
                      stats[i].value,
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

class _TransferResultVisualData {
  const _TransferResultVisualData({
    required this.statusLabel,
    required this.accentColor,
    required this.buttonColor,
    required this.icon,
  });

  final String statusLabel;
  final Color accentColor;
  final Color buttonColor;
  final IconData icon;
}

_TransferResultVisualData _visualForOutcome(TransferResultOutcome outcome) {
  return switch (outcome) {
    TransferResultOutcome.success => _TransferResultVisualData(
      statusLabel: 'Success',
      accentColor: const Color(0xFF49B36C),
      buttonColor: kAccentCyanStrong,
      icon: Icons.check_circle_rounded,
    ),
    TransferResultOutcome.cancelled => const _TransferResultVisualData(
      statusLabel: 'Cancelled',
      accentColor: Color(0xFFC0912C),
      buttonColor: Color(0xFF617B87),
      icon: Icons.do_not_disturb_on_rounded,
    ),
    TransferResultOutcome.failed => const _TransferResultVisualData(
      statusLabel: 'Failed',
      accentColor: Color(0xFFCC3333),
      buttonColor: kDanger,
      icon: Icons.error_rounded,
    ),
  };
}
