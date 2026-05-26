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

  static const _bg = Color(0xFFFBF1DC);
  static const _border = Color(0xFFE6C98E);
  static const _ink = Color(0xFF7A5511);

  @override
  Widget build(BuildContext context) {
    return Container(
      padding: const EdgeInsets.fromLTRB(14, 12, 8, 12),
      decoration: BoxDecoration(
        color: _bg,
        border: Border.all(color: _border, width: 0.8),
        borderRadius: BorderRadius.circular(12),
      ),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          const Icon(Icons.warning_rounded, size: 18, color: _ink),
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
                    color: _ink,
                  ),
                ),
                const SizedBox(height: 4),
                Text(
                  detail,
                  style: wispSans(
                    fontSize: 12.5,
                    fontWeight: FontWeight.w400,
                    color: _ink,
                    height: 1.4,
                  ),
                  maxLines: 4,
                  overflow: TextOverflow.ellipsis,
                ),
                const SizedBox(height: 8),
                OutlinedButton(
                  onPressed: onOpenSelfTest,
                  style: OutlinedButton.styleFrom(
                    foregroundColor: _ink,
                    side: const BorderSide(color: _border),
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
            color: _ink,
            tooltip: 'Dismiss',
            onPressed: onDismiss,
            visualDensity: VisualDensity.compact,
          ),
        ],
      ),
    );
  }
}
