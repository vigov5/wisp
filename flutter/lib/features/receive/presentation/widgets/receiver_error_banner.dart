import 'package:flutter/material.dart';

import '../../../../app/app_router.dart';
import '../../../../theme/wisp_theme.dart';
import '../../application/state.dart';

/// Surface for transfer-stream errors raised by the receiver service.
///
/// The receiver was previously silent when the underlying Rust stream
/// errored — the idle card kept showing as if everything were fine. This
/// banner makes the failure visible so the user can act on it (typically
/// open Connection Test to diagnose, or jump straight to Settings when the
/// error message points at a folder/permission misconfiguration).
class ReceiverErrorBanner extends StatelessWidget {
  const ReceiverErrorBanner({
    super.key,
    required this.error,
    required this.onDismiss,
  });

  final ReceiverServiceError error;
  final VoidCallback onDismiss;

  static const _bg = Color(0xFFFCEEEE);
  static const _border = Color(0xFFE6B5B5);
  static const _ink = Color(0xFF8A1F1F);

  @override
  Widget build(BuildContext context) {
    final actionLabel = _actionLabel(error.action);
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
          const Icon(Icons.error_rounded, size: 18, color: _ink),
          const SizedBox(width: 10),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(
                  'Receiver service stopped',
                  style: wispSans(
                    fontSize: 13,
                    fontWeight: FontWeight.w700,
                    color: _ink,
                  ),
                ),
                const SizedBox(height: 4),
                Text(
                  error.message,
                  style: wispSans(
                    fontSize: 12.5,
                    fontWeight: FontWeight.w400,
                    color: _ink,
                    height: 1.4,
                  ),
                  maxLines: 4,
                  overflow: TextOverflow.ellipsis,
                ),
                if (actionLabel != null) ...[
                  const SizedBox(height: 8),
                  OutlinedButton(
                    onPressed: () => _handleAction(context),
                    style: OutlinedButton.styleFrom(
                      foregroundColor: _ink,
                      side: const BorderSide(color: _border),
                      visualDensity: VisualDensity.compact,
                      padding: const EdgeInsets.symmetric(
                        horizontal: 12,
                        vertical: 4,
                      ),
                    ),
                    child: Text(actionLabel),
                  ),
                ],
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

  String? _actionLabel(ReceiverErrorAction action) {
    switch (action) {
      case ReceiverErrorAction.openConnectionTest:
        return 'Run connection test';
      case ReceiverErrorAction.openSettings:
        return 'Open Settings';
      case ReceiverErrorAction.none:
        return null;
    }
  }

  void _handleAction(BuildContext context) {
    onDismiss();
    switch (error.action) {
      case ReceiverErrorAction.openConnectionTest:
        context.pushConnectionTest();
        break;
      case ReceiverErrorAction.openSettings:
        context.goSettings();
        break;
      case ReceiverErrorAction.none:
        break;
    }
  }
}
