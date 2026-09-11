import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../application/state.dart';
import '../../../../platform/android_media_store.dart';
import '../../../saved_devices/application/device_display_name.dart';
import 'package:app/features/send/presentation/widgets/recipient_avatar.dart';
import 'relay_tip_note.dart';
import 'sending_connection_strip.dart';
import 'transfer_flow_layout.dart';
import 'transfer_manifest_panel.dart';
import 'transfer_presentation_helpers.dart';
import 'package:app/theme/wisp_theme.dart';

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

    // Checked before the speed line: once the last byte has landed a stale
    // speed reading is worse than no reading, because the thing left to say is
    // that the device is still writing files.
    final finishingUp =
        progress.isFinalizing ||
        isFinishingUp(
          bytesTransferred: progress.bytesTransferred,
          totalBytes: progress.totalBytes,
        );

    final Widget subtitle;
    if (finishingUp) {
      subtitle = buildFinalizingLine('Saving files to this device');
    } else if (progress.bytesTransferred == BigInt.zero) {
      // Before the first byte the receiver may still be creating one
      // destination per file, which it does *before* answering the sender —
      // so this card is already up while the sender still says "waiting".
      // Rebuilt from the notifier rather than the transfer state: the count
      // comes straight off the platform channel and ticks ~120 times, which
      // has no business going through the session state.
      subtitle = ValueListenableBuilder<AndroidReceivePrepareProgress?>(
        valueListenable: AndroidReceiveDestinations.prepareProgress,
        builder: (context, preparing, child) => preparing == null
            ? child!
            : buildPreparingToReceiveLine(
                created: preparing.created,
                total: preparing.total,
              ),
        child: buildSubtitleText(
          offer.statusMessage.trim().isEmpty
              ? 'Receiving files...'
              : offer.statusMessage.trim(),
        ),
      );
    } else if (progress.speedLabel != null) {
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
        statusLabel: finishingUp ? 'Finalizing' : 'Receiving',
        statusColor: const Color(0xFFD4A824),
        subtitle: subtitle,
        explainer: null,

        illustration: RecipientAvatar(
          deviceName: senderName,
          deviceType: avatarDeviceType(offer.sender),
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
        footerNote: progress.progressFraction >= 1.0
            ? null
            : RelayTipNote(path: connectionPath),
        footer: progress.progressFraction >= 1.0
            ? const SizedBox(height: 48)
            : Row(
                children: [
                  Expanded(
                    child: TextButton(
                      onPressed: onCancel,
                      style: TextButton.styleFrom(
                        foregroundColor: kDanger,
                        backgroundColor: kDanger.withValues(alpha: 0.08),
                        minimumSize: const Size(0, 48),
                        shape: RoundedRectangleBorder(
                          borderRadius: BorderRadius.circular(12),
                          side: BorderSide(
                            color: kDanger.withValues(alpha: 0.15),
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
