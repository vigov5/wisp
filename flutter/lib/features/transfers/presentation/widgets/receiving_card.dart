import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../application/state.dart';
import '../../../saved_devices/application/device_display_name.dart';
import 'package:app/features/send/presentation/widgets/recipient_avatar.dart';
import 'sending_connection_strip.dart';
import 'transfer_flow_layout.dart';
import 'transfer_manifest_panel.dart';
import 'transfer_presentation_helpers.dart';

class ReceivingCard extends ConsumerWidget {
  const ReceivingCard({
    super.key,
    required this.offer,
    required this.progress,
    required this.animate,
    required this.onCancel,
  });

  final TransferIncomingOffer offer;
  final TransferTransferProgress progress;
  final bool animate;
  final VoidCallback onCancel;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final senderName = resolveDeviceName(
      ref,
      endpointId: offer.senderEndpointId ?? '',
      broadcastLabel: displaySender(offer.sender.displayName),
    ).primary;

    final Widget subtitle;
    if (progress.speedLabel != null) {
      subtitle = buildSpeedLine(
        speedLabel: progress.speedLabel!,
        etaLabel: progress.etaLabel,
      );
    } else {
      subtitle = buildSubtitleText(
        offer.statusMessage.trim().isEmpty
            ? 'Receiving files...'
            : offer.statusMessage.trim(),
      );
    }

    final connectionPath = progress.connectionPath ?? offer.connectionPath;

    return SizedBox.expand(
      child: TransferFlowLayout(
        statusLabel: 'Receiving',
        statusColor: const Color(0xFFD4A824),
        subtitle: subtitle,
        explainer: null,

        illustration: RecipientAvatar(
          deviceName: senderName,
          deviceType: deviceTypeLabel(offer.sender.deviceType),
          animate: animate,
          mode: SendingStripMode.transferring,
          progress: progress.progressFraction,
          connectionPath: connectionPath,
        ),
        manifest: TransferManifestPanel(
          mode: TransferManifestPanelMode.liveList,
          items: offer.manifest.items,
          progress: progress,
        ),
        footer: progress.progressFraction >= 1.0
            ? const SizedBox(height: 48)
            : Row(
                children: [
                  Expanded(
                    child: TextButton(
                      onPressed: onCancel,
                      style: TextButton.styleFrom(
                        foregroundColor: const Color(0xFFB34A4A),
                        backgroundColor: const Color(
                          0xFFB34A4A,
                        ).withValues(alpha: 0.08),
                        minimumSize: const Size(0, 48),
                        shape: RoundedRectangleBorder(
                          borderRadius: BorderRadius.circular(12),
                          side: BorderSide(
                            color: const Color(
                              0xFFB34A4A,
                            ).withValues(alpha: 0.15),
                          ),
                        ),
                      ),
                      child: const Text('Cancel transfer'),
                    ),
                  ),
                ],
              ),
      ),
    );
  }
}
