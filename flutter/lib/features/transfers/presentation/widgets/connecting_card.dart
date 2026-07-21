import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../../../theme/wisp_theme.dart';
import '../../application/state.dart';
import '../../../saved_devices/application/device_display_name.dart';
import 'package:app/features/send/presentation/widgets/recipient_avatar.dart';
import 'sending_connection_strip.dart';
import 'transfer_flow_layout.dart';
import 'transfer_presentation_helpers.dart';

/// Shown the moment a sender connects and identifies itself (Hello exchanged)
/// but before its offer arrives. Mirrors [ReceivingCard]'s layout with the
/// looping "connecting" avatar animation and no manifest/actions — there's
/// nothing to accept until the offer lands (or the wait fails).
class ConnectingCard extends ConsumerWidget {
  const ConnectingCard({
    super.key,
    required this.offer,
    required this.animate,
    this.onCancel,
  });

  final TransferIncomingOffer offer;
  final bool animate;

  /// Bails out of a stalled connect without waiting for the sender or the
  /// offer-wait timeout. Null hides the button (e.g. off desktop wiring).
  final VoidCallback? onCancel;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final senderName = resolveDeviceName(
      ref,
      endpointId: offer.senderEndpointId ?? '',
      broadcastLabel: displaySender(offer.sender.displayName),
    ).primary;

    final statusMessage = offer.statusMessage.trim().isEmpty
        ? '$senderName is connecting…'
        : offer.statusMessage.trim();

    return SizedBox.expand(
      child: TransferFlowLayout(
        statusLabel: 'Connecting',
        statusColor: kAccentCyanStrong,
        subtitle: buildSubtitleText(statusMessage),
        explainer: null,
        illustration: RecipientAvatar(
          deviceName: senderName,
          deviceType: avatarDeviceType(offer.sender),
          animate: animate,
          mode: SendingStripMode.looping,
        ),
        manifest: null,
        footer: onCancel == null
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
                      child: const Text('Cancel'),
                    ),
                  ),
                ],
              ),
      ),
    );
  }
}
