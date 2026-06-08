import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../../theme/wisp_theme.dart';
import '../../transfers/application/pubkey_visual.dart';
import '../application/saved_device.dart';
import '../application/saved_devices_controller.dart';

class SavedDevicesPage extends ConsumerWidget {
  const SavedDevicesPage({super.key});

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final devices = ref.watch(savedDevicesProvider);
    return Scaffold(
      backgroundColor: kBg,
      appBar: AppBar(
        backgroundColor: kBg,
        elevation: 0,
        title: Text(
          'Saved devices',
          style: wispSans(
            fontSize: 17,
            fontWeight: FontWeight.w700,
            color: kInk,
          ),
        ),
        actions: [
          if (devices.isNotEmpty)
            TextButton(
              onPressed: () => _confirmClearAll(context, ref),
              child: Text(
                'Clear all',
                style: wispSans(
                  fontSize: 13.5,
                  fontWeight: FontWeight.w600,
                  color: const Color(0xFFB34A4A),
                ),
              ),
            ),
        ],
      ),
      body: devices.isEmpty
          ? _emptyState()
          : ListView.separated(
              padding: const EdgeInsets.all(16),
              itemCount: devices.length,
              separatorBuilder: (_, _) => const SizedBox(height: 8),
              itemBuilder: (context, index) {
                final device = devices[index];
                return _SavedDeviceRow(
                  device: device,
                  onDelete: () => ref
                      .read(savedDevicesProvider.notifier)
                      .remove(device.endpointId),
                );
              },
            ),
    );
  }

  Widget _emptyState() {
    return Center(
      child: Padding(
        padding: const EdgeInsets.symmetric(horizontal: 24),
        child: Text(
          'No saved devices yet.\n\nDevices appear here automatically after '
          'a successful transfer, so you can pick them again from the '
          '"Recent" list without scanning.',
          textAlign: TextAlign.center,
          style: wispSans(
            fontSize: 14,
            fontWeight: FontWeight.w400,
            color: kMuted,
            height: 1.5,
          ),
        ),
      ),
    );
  }

  Future<void> _confirmClearAll(BuildContext context, WidgetRef ref) async {
    final ok = await showDialog<bool>(
      context: context,
      builder: (context) => AlertDialog(
        title: const Text('Clear all saved devices?'),
        content: const Text(
          'You can re-discover them next time you transfer.',
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(context).pop(false),
            child: const Text('Cancel'),
          ),
          FilledButton(
            onPressed: () => Navigator.of(context).pop(true),
            style: FilledButton.styleFrom(
              backgroundColor: kAccentCyanStrong,
            ),
            child: const Text('Clear'),
          ),
        ],
      ),
    );
    if (ok ?? false) {
      await ref.read(savedDevicesProvider.notifier).clear();
    }
  }
}

class _SavedDeviceRow extends StatelessWidget {
  const _SavedDeviceRow({required this.device, required this.onDelete});

  final SavedDevice device;
  final VoidCallback onDelete;

  @override
  Widget build(BuildContext context) {
    final isPhone = device.deviceType.toLowerCase() == 'phone';
    return Dismissible(
      key: ValueKey('saved-${device.endpointId}'),
      direction: DismissDirection.endToStart,
      background: Container(
        decoration: BoxDecoration(
          color: const Color(0xFFB34A4A).withValues(alpha: 0.12),
          borderRadius: BorderRadius.circular(14),
        ),
        alignment: Alignment.centerRight,
        padding: const EdgeInsets.symmetric(horizontal: 20),
        child: const Icon(Icons.delete_outline, color: Color(0xFFB34A4A)),
      ),
      onDismissed: (_) => onDelete(),
      child: Container(
        padding: const EdgeInsets.symmetric(horizontal: 14, vertical: 12),
        decoration: BoxDecoration(
          color: kSurface,
          borderRadius: BorderRadius.circular(14),
          border: Border.all(color: kBorder),
        ),
        child: Row(
          children: [
            Icon(
              isPhone ? Icons.smartphone_rounded : Icons.laptop_mac_rounded,
              size: 26,
              color: kInk.withValues(alpha: 0.8),
            ),
            const SizedBox(width: 12),
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(
                    device.label.isEmpty ? 'Saved device' : device.label,
                    style: wispSans(
                      fontSize: 14,
                      fontWeight: FontWeight.w700,
                      color: kInk,
                    ),
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                  ),
                  const SizedBox(height: 6),
                  Row(
                    children: [
                      PubkeyBadge(
                        endpointId: device.endpointId,
                        size: PubkeyBadgeSize.small,
                        tooltip: 'Identity badge (from public key) — '
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
                            color: kMuted,
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
            IconButton(
              icon: const Icon(Icons.close_rounded, size: 20),
              color: kMuted,
              onPressed: onDelete,
              tooltip: 'Remove',
            ),
          ],
        ),
      ),
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
