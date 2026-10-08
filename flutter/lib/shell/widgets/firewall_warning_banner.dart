import 'package:flutter/material.dart';

import '../../theme/wisp_theme.dart';

/// Startup banner shown on Windows when the firewall probe suggests inbound
/// transfers are at risk (no rule, block rule, disabled rule).  Lives above
/// ReceiverErrorBanner in the desktop shell so the user notices the
/// underlying cause before the receiver fails.
class FirewallWarningBanner extends StatelessWidget {
  const FirewallWarningBanner({
    super.key,
    required this.detail,
    required this.onDismiss,
    required this.onOpenSelfTest,
  });

  final String detail;
  final VoidCallback onDismiss;
  final VoidCallback onOpenSelfTest;

  @override
  Widget build(BuildContext context) {
    return Container(
      padding: const EdgeInsets.fromLTRB(14, 12, 8, 12),
      decoration: BoxDecoration(
        color: context.wc.warnBg,
        border: Border.all(color: context.wc.warnBorder, width: 0.8),
        borderRadius: BorderRadius.circular(12),
      ),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Icon(Icons.warning_rounded, size: 18, color: context.wc.warnFg),
          const SizedBox(width: 10),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(
                  'Inbound transfers may be blocked',
                  style: wispSans(
                    fontSize: 13,
                    fontWeight: FontWeight.w700,
                    color: context.wc.warnInk,
                  ),
                ),
                const SizedBox(height: 4),
                Text(
                  detail,
                  style: wispSans(
                    fontSize: 12.5,
                    fontWeight: FontWeight.w400,
                    color: context.wc.warnInk,
                    height: 1.4,
                  ),
                  maxLines: 4,
                  overflow: TextOverflow.ellipsis,
                ),
                const SizedBox(height: 8),
                OutlinedButton(
                  onPressed: onOpenSelfTest,
                  style: OutlinedButton.styleFrom(
                    foregroundColor: context.wc.warnInk,
                    side: BorderSide(color: context.wc.warnBorder),
                    visualDensity: VisualDensity.compact,
                    padding: const EdgeInsets.symmetric(
                      horizontal: 12,
                      vertical: 4,
                    ),
                  ),
                  child: const Text('Run connection test'),
                ),
              ],
            ),
          ),
          IconButton(
            icon: const Icon(Icons.close_rounded, size: 18),
            color: context.wc.warnInk,
            tooltip: 'Dismiss',
            onPressed: onDismiss,
            visualDensity: VisualDensity.compact,
          ),
        ],
      ),
    );
  }
}
