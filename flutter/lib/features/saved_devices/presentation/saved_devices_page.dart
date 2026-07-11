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
      backgroundColor: context.wc.bg,
      appBar: AppBar(
        backgroundColor: context.wc.bg,
        elevation: 0,
        title: Text(
          'Saved devices',
          style: wispSans(
            fontSize: 17,
            fontWeight: FontWeight.w700,
            color: context.wc.ink,
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
                  color: kDanger,
                ),
              ),
            ),
        ],
      ),
      body: devices.isEmpty
          ? _emptyState(context)
          : ListView.separated(
              padding: const EdgeInsets.all(16),
              itemCount: devices.length,
              separatorBuilder: (_, _) => const SizedBox(height: 8),
              itemBuilder: (context, index) {
                final device = devices[index];
                return _SavedDeviceRow(
                  device: device,
                  onRename: () => _renameDevice(context, ref, device),
                  onDelete: () => ref
                      .read(savedDevicesProvider.notifier)
                      .remove(device.endpointId),
                );
              },
            ),
    );
  }

  Widget _emptyState(BuildContext context) {
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
            color: context.wc.muted,
            height: 1.5,
          ),
        ),
      ),
    );
  }

  Future<void> _renameDevice(
    BuildContext context,
    WidgetRef ref,
    SavedDevice device,
  ) async {
    final result = await showDialog<String?>(
      context: context,
      builder: (context) => _RenameDialog(device: device),
    );
    // null => dialog dismissed/cancelled (no change). A returned string (incl.
    // empty) => apply; empty clears the nickname back to the broadcast name.
    if (result == null) return;
    await ref
        .read(savedDevicesProvider.notifier)
        .rename(device.endpointId, result);
  }

  Future<void> _confirmClearAll(BuildContext context, WidgetRef ref) async {
    final ok = await showDialog<bool>(
      context: context,
      builder: (context) => AlertDialog(
        title: const Text('Clear all saved devices?'),
        content: const Text('You can re-discover them next time you transfer.'),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(context).pop(false),
            child: const Text('Cancel'),
          ),
          FilledButton(
            onPressed: () => Navigator.of(context).pop(true),
            style: FilledButton.styleFrom(backgroundColor: kAccentCyanStrong),
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

/// Owns its own [TextEditingController] so the controller's lifecycle matches
/// the dialog route — disposing in the parent right after `showDialog` returns
/// races the dialog's exit animation, which rebuilds the field and re-attaches
/// a listener to a disposed controller.
class _RenameDialog extends StatefulWidget {
  const _RenameDialog({required this.device});

  final SavedDevice device;

  @override
  State<_RenameDialog> createState() => _RenameDialogState();
}

class _RenameDialogState extends State<_RenameDialog> {
  late final TextEditingController _controller;

  @override
  void initState() {
    super.initState();
    _controller = TextEditingController(text: widget.device.nickname ?? '');
  }

  @override
  void dispose() {
    _controller.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final broadcast = widget.device.label.trim();
    return AlertDialog(
      title: const Text('Rename device'),
      content: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          TextField(
            controller: _controller,
            autofocus: true,
            textCapitalization: TextCapitalization.words,
            decoration: InputDecoration(
              labelText: 'Nickname',
              hintText: broadcast.isEmpty ? 'My device' : broadcast,
            ),
            onSubmitted: (value) => Navigator.of(context).pop(value),
          ),
          const SizedBox(height: 10),
          Text(
            broadcast.isEmpty
                ? 'Leave empty to use their device name.'
                : 'Leave empty to use their name "$broadcast".',
            style: wispSans(fontSize: 12, color: context.wc.muted, height: 1.4),
          ),
        ],
      ),
      actions: [
        TextButton(
          onPressed: () => Navigator.of(context).pop(),
          child: const Text('Cancel'),
        ),
        FilledButton(
          onPressed: () => Navigator.of(context).pop(_controller.text),
          style: FilledButton.styleFrom(backgroundColor: kAccentCyanStrong),
          child: const Text('Save'),
        ),
      ],
    );
  }
}

class _SavedDeviceRow extends StatelessWidget {
  const _SavedDeviceRow({
    required this.device,
    required this.onRename,
    required this.onDelete,
  });

  final SavedDevice device;
  final VoidCallback onRename;
  final VoidCallback onDelete;

  @override
  Widget build(BuildContext context) {
    final isPhone = device.deviceType.toLowerCase() == 'phone';
    final nickname = device.nickname?.trim() ?? '';
    final hasNickname = nickname.isNotEmpty;
    final broadcast = device.label.trim();
    final primary = hasNickname
        ? nickname
        : (broadcast.isEmpty ? 'Saved device' : broadcast);
    return Dismissible(
      key: ValueKey('saved-${device.endpointId}'),
      direction: DismissDirection.endToStart,
      background: Container(
        decoration: BoxDecoration(
          color: kDanger.withValues(alpha: 0.12),
          borderRadius: BorderRadius.circular(14),
        ),
        alignment: Alignment.centerRight,
        padding: const EdgeInsets.symmetric(horizontal: 20),
        child: const Icon(Icons.delete_outline, color: kDanger),
      ),
      onDismissed: (_) => onDelete(),
      child: Container(
        padding: const EdgeInsets.symmetric(horizontal: 14, vertical: 12),
        decoration: BoxDecoration(
          color: context.wc.surface,
          borderRadius: BorderRadius.circular(14),
          border: Border.all(color: context.wc.border),
        ),
        child: Row(
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
            IconButton(
              icon: const Icon(Icons.edit_outlined, size: 19),
              color: context.wc.muted,
              onPressed: onRename,
              tooltip: 'Rename',
            ),
            IconButton(
              icon: const Icon(Icons.close_rounded, size: 20),
              color: context.wc.muted,
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
