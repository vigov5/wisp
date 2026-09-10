import 'package:flutter/material.dart';
import '../../../../theme/wisp_theme.dart';
import '../../application/identity.dart';

export '../../application/format_utils.dart';

String displaySender(String value) {
  final trimmed = value.trim();
  return trimmed.isEmpty ? 'Unknown sender' : trimmed;
}

String incomingSubtitle(int itemCount, String totalSize) {
  final fileWord = itemCount == 1 ? 'file' : 'files';
  return 'wants to send you $itemCount $fileWord ($totalSize)';
}

String resumeSubtitle({
  required int itemCount,
  required String receivedSize,
  required String totalSize,
}) {
  final fileWord = itemCount == 1 ? 'file' : 'files';
  return 'will resume receiving $itemCount $fileWord ($receivedSize of $totalSize)';
}

String fileCountLabel(int itemCount) {
  return itemCount == 1 ? '1 file' : '$itemCount files';
}

String deviceTypeLabel(DeviceType type) {
  return switch (type) {
    DeviceType.phone => 'phone',
    DeviceType.laptop => 'laptop',
  };
}

/// Device-type string for [RecipientAvatar], which renders `'web'` as a globe.
/// A browser peer overrides its laptop/phone type so it reads as a web app.
String avatarDeviceType(TransferIdentity identity) =>
    identity.web ? 'web' : deviceTypeLabel(identity.deviceType);

Widget buildSubtitleText(String text) {
  return Text(
    text,
    textAlign: TextAlign.center,
    style: wispSans(
      fontSize: 14,
      fontWeight: FontWeight.w500,
      color: kMuted,
      height: 1.4,
    ),
  );
}

/// Failure subtitle: a bold, accent-coloured [title] (e.g. "Incompatible
/// version"), the descriptive [message], and — when present — an actionable
/// [recovery] hint ("Update Wisp …") in the accent colour so it reads as the
/// next step rather than more error prose.
Widget buildFailureSubtitle({
  required String title,
  required String message,
  String? recovery,
  required Color accent,
}) {
  return Column(
    mainAxisSize: MainAxisSize.min,
    children: [
      Text(
        title,
        textAlign: TextAlign.center,
        style: wispSans(
          fontSize: 15,
          fontWeight: FontWeight.w700,
          color: accent,
        ),
      ),
      const SizedBox(height: 6),
      buildSubtitleText(message),
      if (recovery != null) ...[
        const SizedBox(height: 10),
        Text(
          recovery,
          textAlign: TextAlign.center,
          // Not the error accent (red): the recovery line is an actionable
          // next step, so it uses the app's action cyan, distinct from the
          // red failure title above it.
          style: wispSans(
            fontSize: 13.5,
            fontWeight: FontWeight.w600,
            color: kAccentCyan,
            height: 1.4,
          ),
        ),
      ],
    ],
  );
}

/// Subtitle line plus an optional, smaller "broadcasts as …" line shown only
/// when the user has renamed the device — keeps the peer-reported name visible
/// for trust without repeating it inside the instruction text. [broadcast] is
/// the peer-reported name (null when no nickname overrides it).
Widget buildSubtitleWithBroadcast(String text, String? broadcast) {
  if (broadcast == null) return buildSubtitleText(text);
  return Column(
    mainAxisSize: MainAxisSize.min,
    children: [
      buildSubtitleText(text),
      const SizedBox(height: 4),
      Text(
        'Their name: "$broadcast"',
        textAlign: TextAlign.center,
        style: wispSans(
          fontSize: 12,
          fontWeight: FontWeight.w500,
          color: kSubtle,
          height: 1.3,
        ),
      ),
    ],
  );
}

/// True once every byte has moved but the transfer has not reported a result.
///
/// What happens in that window is real work and takes real time — the receiver
/// writes each file to its final destination and clears its pending flag,
/// measured at 23-26 s for 1911 files — but nothing on either screen said so.
/// The speed reading disappears with the last byte and both sides fell back to
/// a bare "Sending"/"Receiving files...", which looks like a transfer that has
/// stalled and invites the user to close the app in the middle of it.
bool isFinishingUp({
  required BigInt bytesTransferred,
  required BigInt totalBytes,
}) => totalBytes > BigInt.zero && bytesTransferred >= totalBytes;

/// The line shown while that finishing work runs.
Widget buildFinalizingLine(String detail) {
  return Builder(
    builder: (context) => Text.rich(
      TextSpan(
        children: [
          TextSpan(
            text: detail,
            style: wispSans(
              fontSize: 14,
              fontWeight: FontWeight.w500,
              color: context.wc.muted,
              height: 1.4,
            ),
          ),
          TextSpan(
            text: '  ·  keep Wisp open',
            style: wispSans(
              fontSize: 14,
              fontWeight: FontWeight.w700,
              color: context.wc.ink,
              height: 1.4,
            ),
          ),
        ],
      ),
      textAlign: TextAlign.center,
    ),
  );
}

Widget buildSpeedLine({required String speedLabel, required String? etaLabel}) {
  return Builder(
    builder: (context) => Text.rich(
      TextSpan(
        children: [
          TextSpan(
            text: speedLabel,
            style: wispSans(
              fontSize: 14,
              fontWeight: FontWeight.w700,
              color: context.wc.ink,
            ),
          ),
          if (etaLabel != null) ...[
            TextSpan(
              text: '  ·  ',
              style: wispSans(
                fontSize: 14,
                fontWeight: FontWeight.w500,
                color: context.wc.subtle,
              ),
            ),
            TextSpan(
              text: etaLabel,
              style: wispSans(
                fontSize: 13,
                fontWeight: FontWeight.w500,
                color: context.wc.muted,
              ),
            ),
          ],
        ],
      ),
      textAlign: TextAlign.center,
    ),
  );
}
