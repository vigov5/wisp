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

Widget buildSpeedLine({required String speedLabel, required String? etaLabel}) {
  return Text.rich(
    TextSpan(
      children: [
        TextSpan(
          text: speedLabel,
          style: wispSans(
            fontSize: 14,
            fontWeight: FontWeight.w700,
            color: kInk,
          ),
        ),
        if (etaLabel != null) ...[
          TextSpan(
            text: '  ·  ',
            style: wispSans(
              fontSize: 14,
              fontWeight: FontWeight.w500,
              color: kSubtle,
            ),
          ),
          TextSpan(
            text: etaLabel,
            style: wispSans(
              fontSize: 13,
              fontWeight: FontWeight.w500,
              color: kMuted,
            ),
          ),
        ],
      ],
    ),
    textAlign: TextAlign.center,
  );
}
