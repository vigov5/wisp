import 'package:flutter/material.dart';

import '../../../../theme/drift_theme.dart';
import '../../application/connection_path.dart';

/// Compact pill showing whether the active iroh transfer is "P2P direct" or
/// "Via relay: <host>". Tapping/long-pressing reveals the full relay URL.
class ConnectionPathBadge extends StatelessWidget {
  const ConnectionPathBadge({super.key, required this.path, this.dense = true});

  final ConnectionPathInfo? path;
  final bool dense;

  @override
  Widget build(BuildContext context) {
    final info = path;
    if (info == null || info.kind == ConnectionPathKind.unknown) {
      return const SizedBox.shrink();
    }

    final accent = info.isDirect ? kAccentDirect : kAccentRelay;
    final label = _label(info);
    final tooltip = info.isDirect
        ? (info.directAddr ?? label)
        : (info.relayUrl ?? label);
    final semantic = info.isDirect
        ? 'Connection: peer-to-peer direct${info.directIpHost == null ? '' : ' via ${info.directIpHost}'}'
        : 'Connection: via relay ${info.relayHost ?? ''}';

    final padding = dense
        ? const EdgeInsets.symmetric(horizontal: 8, vertical: 4)
        : const EdgeInsets.symmetric(horizontal: 10, vertical: 6);

    final pill = Container(
      padding: padding,
      decoration: BoxDecoration(
        color: accent.withValues(alpha: 0.12),
        border: Border.all(color: accent.withValues(alpha: 0.45), width: 1),
        borderRadius: BorderRadius.circular(999),
      ),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          Container(
            width: 8,
            height: 8,
            decoration: BoxDecoration(color: accent, shape: BoxShape.circle),
          ),
          const SizedBox(width: 6),
          Flexible(
            child: Text(
              label,
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              style: driftSans(
                fontSize: dense ? 11 : 12,
                fontWeight: FontWeight.w600,
                color: kInk.withValues(alpha: 0.92),
                letterSpacing: -0.1,
              ),
            ),
          ),
        ],
      ),
    );

    return Semantics(
      label: semantic,
      child: Tooltip(
        message: tooltip,
        child: AnimatedSwitcher(
          duration: const Duration(milliseconds: 220),
          switchInCurve: Curves.easeOut,
          switchOutCurve: Curves.easeIn,
          child: KeyedSubtree(
            key: ValueKey('${info.kind}-${info.relayHost ?? ''}'),
            child: pill,
          ),
        ),
      ),
    );
  }

  String _label(ConnectionPathInfo info) {
    if (info.isDirect) {
      final ip = info.directIpHost;
      if (ip == null || ip.isEmpty) {
        return 'P2P direct';
      }
      return 'P2P direct via $ip';
    }
    final host = info.relayHost;
    if (host == null || host.isEmpty) {
      return 'Via relay';
    }
    return 'Via relay: $host';
  }
}
