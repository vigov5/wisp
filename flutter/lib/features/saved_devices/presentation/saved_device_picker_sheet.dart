import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../../theme/wisp_theme.dart';
import '../application/saved_device.dart';
import '../application/saved_devices_controller.dart';
import 'widgets/saved_device_summary.dart';

/// Opens a read-only, scrollable list of every saved device (favorites first)
/// and resolves to the one tapped, or null if dismissed. [selectedTicket]
/// marks the device the sender currently targets.
Future<SavedDevice?> showSavedDevicePicker(
  BuildContext context, {
  String? selectedTicket,
}) {
  return showModalBottomSheet<SavedDevice>(
    context: context,
    isScrollControlled: true,
    backgroundColor: Colors.transparent,
    builder: (_) => SavedDevicePickerSheet(selectedTicket: selectedTicket),
  );
}

class SavedDevicePickerSheet extends ConsumerWidget {
  const SavedDevicePickerSheet({super.key, this.selectedTicket});

  final String? selectedTicket;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final devices = favoritesFirst(ref.watch(savedDevicesProvider));
    return DraggableScrollableSheet(
      key: const ValueKey('saved-device-picker'),
      expand: false,
      initialChildSize: 0.6,
      minChildSize: 0.3,
      maxChildSize: 0.9,
      builder: (context, scrollController) => DecoratedBox(
        decoration: BoxDecoration(
          color: context.wc.bg,
          borderRadius: const BorderRadius.vertical(top: Radius.circular(24)),
          border: Border.all(color: context.wc.border),
        ),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Center(
              child: Container(
                margin: const EdgeInsets.only(top: 10),
                width: 36,
                height: 4,
                decoration: BoxDecoration(
                  color: context.wc.border,
                  borderRadius: BorderRadius.circular(2),
                ),
              ),
            ),
            Padding(
              padding: const EdgeInsets.fromLTRB(20, 14, 20, 6),
              child: Text(
                'Pick a saved device',
                style: wispSans(
                  fontSize: 17,
                  fontWeight: FontWeight.w700,
                  color: context.wc.ink,
                ),
              ),
            ),
            Expanded(
              child: ListView.separated(
                controller: scrollController,
                padding: const EdgeInsets.fromLTRB(16, 8, 16, 24),
                itemCount: devices.length,
                separatorBuilder: (_, _) => const SizedBox(height: 8),
                itemBuilder: (context, index) {
                  final device = devices[index];
                  final ticket = device.lastTicket ?? '';
                  return _PickerRow(
                    device: device,
                    isSelected: ticket.isNotEmpty && ticket == selectedTicket,
                    onTap: () => Navigator.of(context).pop(device),
                  );
                },
              ),
            ),
          ],
        ),
      ),
    );
  }
}

class _PickerRow extends StatelessWidget {
  const _PickerRow({
    required this.device,
    required this.isSelected,
    required this.onTap,
  });

  final SavedDevice device;
  final bool isSelected;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    return Material(
      color: Colors.transparent,
      child: InkWell(
        onTap: onTap,
        borderRadius: BorderRadius.circular(14),
        child: Ink(
          padding: const EdgeInsets.symmetric(horizontal: 14, vertical: 12),
          decoration: BoxDecoration(
            color: isSelected
                ? kAccentCyan.withValues(alpha: 0.10)
                : context.wc.surface,
            borderRadius: BorderRadius.circular(14),
            border: Border.all(
              color: isSelected
                  ? kAccentCyan.withValues(alpha: 0.5)
                  : context.wc.border,
            ),
          ),
          child: Row(
            children: [
              Expanded(child: SavedDeviceSummary(device: device)),
              if (device.favorite) ...[
                const SizedBox(width: 8),
                Icon(
                  Icons.star_rounded,
                  size: 20,
                  color: context.wc.favoriteFg,
                  semanticLabel: 'Favorite',
                ),
              ],
              if (isSelected) ...[
                const SizedBox(width: 8),
                Icon(
                  Icons.check_circle_rounded,
                  size: 20,
                  color: context.wc.accentFg,
                  semanticLabel: 'Selected',
                ),
              ],
            ],
          ),
        ),
      ),
    );
  }
}
