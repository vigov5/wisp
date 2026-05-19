import 'package:flutter/material.dart';

import '../../../../theme/drift_theme.dart';
import '../../domain/check_result.dart';
import '../../domain/diagnostics_state.dart';

class SummaryBanner extends StatelessWidget {
  const SummaryBanner({super.key, required this.state});

  final DiagnosticsState state;

  @override
  Widget build(BuildContext context) {
    final overall = state.overallStatus;
    final color = overall.color;
    final running = state.isRunning;

    final title = running
        ? 'Running checks…'
        : _titleForStatus(overall, state);

    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 14, vertical: 12),
      decoration: BoxDecoration(
        color: color.withValues(alpha: 0.10),
        borderRadius: BorderRadius.circular(12),
        border: Border.all(color: color.withValues(alpha: 0.45), width: 0.8),
      ),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.center,
        children: [
          _icon(overall, running),
          const SizedBox(width: 10),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(
                  title,
                  style: driftSans(
                    fontSize: 14,
                    fontWeight: FontWeight.w700,
                    color: kInk,
                  ),
                ),
                const SizedBox(height: 4),
                Text(
                  _subtitle(state),
                  style: driftSans(
                    fontSize: 12,
                    fontWeight: FontWeight.w500,
                    color: kMuted,
                  ),
                ),
              ],
            ),
          ),
        ],
      ),
    );
  }

  Widget _icon(CheckStatus status, bool running) {
    if (running) {
      return SizedBox(
        width: 18,
        height: 18,
        child: CircularProgressIndicator(
          strokeWidth: 2,
          color: status.color,
        ),
      );
    }
    final iconData = status.icon ?? Icons.help_outline_rounded;
    return Icon(iconData, color: status.color, size: 20);
  }

  String _subtitle(DiagnosticsState state) {
    final parts = <String>[
      '${state.passCount} pass',
      '${state.warnCount} warn',
      '${state.failCount} fail',
    ];
    if (state.skippedCount > 0) {
      parts.add('${state.skippedCount} skipped');
    }
    final base = parts.join(' · ');
    final when = state.lastRunAt;
    if (when == null || state.isRunning) return base;
    return '$base · ${_formatTimestamp(when)}';
  }

  String _titleForStatus(CheckStatus status, DiagnosticsState state) {
    switch (status) {
      case CheckStatus.pass:
        return 'All checks passed';
      case CheckStatus.warn:
        return '${state.warnCount} warning${state.warnCount == 1 ? '' : 's'}';
      case CheckStatus.fail:
        return '${state.failCount} failure${state.failCount == 1 ? '' : 's'}';
      case CheckStatus.skipped:
        return 'No checks ran';
      case CheckStatus.running:
        return 'Running checks…';
    }
  }

  String _formatTimestamp(DateTime when) {
    final delta = DateTime.now().difference(when);
    if (delta.inSeconds < 5) return 'just now';
    if (delta.inMinutes < 1) return '${delta.inSeconds}s ago';
    if (delta.inHours < 1) return '${delta.inMinutes}m ago';
    return '${delta.inHours}h ago';
  }
}
