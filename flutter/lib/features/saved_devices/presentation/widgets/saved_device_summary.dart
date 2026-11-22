import 'package:flutter/material.dart';

import '../../../../theme/wisp_theme.dart';
import '../../../transfers/application/pubkey_visual.dart';
import '../../application/saved_device.dart';

/// Device icon, name (nickname over broadcast name), identity badge and
/// transfer stats: the read-only part of a saved-device row, shared by the
/// Saved devices page and the send picker.
class SavedDeviceSummary extends StatelessWidget {
  const SavedDeviceSummary({super.key, required this.device});

  final SavedDevice device;

  @override
  Widget build(BuildContext context) {
    final isPhone = device.deviceType.toLowerCase() == 'phone';
    final nickname = device.nickname?.trim() ?? '';
    final hasNickname = nickname.isNotEmpty;
    final broadcast = device.label.trim();
    final primary = hasNickname
        ? nickname
        : (broadcast.isEmpty ? 'Saved device' : broadcast);
    return Row(
      children: [
        Icon(
          isPhone ? Icons.smartphone_rounded : Icons.laptop_mac_rounded,
          size: 26,
          color: context.wc.ink.withValues(alpha: 0.8),
        ),
        const SizedBox(width: 12),
        Expanded(
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Text(
                primary,
                style: wispSans(
                  fontSize: 14,
                  fontWeight: FontWeight.w700,
                  color: context.wc.ink,
                ),
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
              ),
              if (hasNickname) ...[
                const SizedBox(height: 2),
                Text(
                  broadcast.isEmpty
                      ? 'No name from them'
                      : 'Their name: $broadcast',
                  style: wispSans(
                    fontSize: 11.5,
                    fontWeight: FontWeight.w400,
                    color: context.wc.muted,
                  ),
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                ),
              ],
              const SizedBox(height: 6),
              Row(
                children: [
                  PubkeyBadge(
                    endpointId: device.endpointId,
                    size: PubkeyBadgeSize.small,
                    tooltip:
                        'Identity badge (from public key) — '
                        'same color = same device.',
                  ),
                  const SizedBox(width: 8),
                  Flexible(
                    child: Text(
                      '${device.transferCount} transfer'
                      '${device.transferCount == 1 ? '' : 's'} · '
                      '${_relativeTime(device.lastSeenAt)}',
                      style: wispSans(
                        fontSize: 11.5,
                        fontWeight: FontWeight.w400,
                        color: context.wc.muted,
                      ),
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                    ),
                  ),
                ],
              ),
            ],
          ),
        ),
      ],
    );
  }
}

String _relativeTime(DateTime t) {
  final delta = DateTime.now().toUtc().difference(t.toUtc());
  if (delta.inMinutes < 1) return 'just now';
  if (delta.inHours < 1) return '${delta.inMinutes}m ago';
  if (delta.inDays < 1) return '${delta.inHours}h ago';
  if (delta.inDays < 7) return '${delta.inDays}d ago';
  if (delta.inDays < 30) return '${(delta.inDays / 7).floor()}w ago';
  return '${(delta.inDays / 30).floor()}mo ago';
}
