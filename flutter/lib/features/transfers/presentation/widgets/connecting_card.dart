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
  const ConnectingCard({super.key, required this.offer, required this.animate});

  final TransferIncomingOffer offer;
  final bool animate;

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
          deviceType: deviceTypeLabel(offer.sender.deviceType),
          animate: animate,
          mode: SendingStripMode.looping,
        ),
        manifest: null,
        footer: const SizedBox(height: 48),
      ),
    );
  }
}
