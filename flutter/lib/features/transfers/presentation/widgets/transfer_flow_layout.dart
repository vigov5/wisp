import 'package:flutter/material.dart';

import '../../../../theme/wisp_theme.dart';

class TransferFlowLayout extends StatelessWidget {
  const TransferFlowLayout({
    super.key,
    required this.statusLabel,
    required this.statusColor,
    required this.subtitle,
    this.explainer,
    required this.illustration,
    this.manifest,
    this.footerNote,
    required this.footer,
  });

  final String statusLabel;
  final Color statusColor;
  final Widget subtitle;
  final Widget? explainer;
  final Widget illustration;
  final Widget? manifest;

  /// Optional advisory pinned directly above the footer's divider line (the
  /// "horizon" rule above the action buttons). Stays out of the scroll area so
  /// it remains visible next to Cancel / Decline. Used for the relay-speed tip.
  final Widget? footerNote;
  final Widget footer;

  @override
  Widget build(BuildContext context) {
    return Column(
      children: [
        Expanded(
          child: SingleChildScrollView(
            physics: const BouncingScrollPhysics(),
            padding: const EdgeInsets.fromLTRB(24, 20, 24, 0),
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.center,
              children: [
                Container(
                  padding: const EdgeInsets.symmetric(
                    horizontal: 10,
                    vertical: 4,
                  ),
                  decoration: BoxDecoration(
                    color: statusColor.withValues(alpha: 0.06),
                    borderRadius: BorderRadius.circular(20),
                    border: Border.all(
                      color: statusColor.withValues(alpha: 0.15),
                      width: 1.0,
                    ),
                  ),
                  child: Row(
                    mainAxisSize: MainAxisSize.min,
                    children: [
                      Container(
                        width: 6,
                        height: 6,
                        decoration: BoxDecoration(
                          color: statusColor,
                          shape: BoxShape.circle,
                        ),
                      ),
                      const SizedBox(width: 6),
                      Text(
                        statusLabel.toUpperCase(),
                        style: wispSans(
                          fontSize: 9.5,
                          fontWeight: FontWeight.w800,
                          color: statusColor,
                          letterSpacing: 0.6,
                        ),
                      ),
                    ],
                  ),
                ),
                const SizedBox(height: 24),
                illustration,
                const SizedBox(height: 8),
                subtitle,
                if (explainer != null) ...[
                  const SizedBox(height: 16),
                  explainer!,
                ],
                const SizedBox(height: 32),
                if (manifest != null) ...[
                  manifest!,
                  const SizedBox(height: 16),
                ],
              ],
            ),
          ),
        ),
        ?footerNote,
        Container(
          padding: const EdgeInsets.fromLTRB(24, 12, 24, 16),
          decoration: BoxDecoration(
            color: context.wc.bg,
            border: Border(
              top: BorderSide(color: context.wc.border.withValues(alpha: 0.4)),
            ),
          ),
          child: footer,
        ),
      ],
    );
  }
}
