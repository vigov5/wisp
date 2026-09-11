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
Widget buildFinalizingLine(String detail) =>
    buildBackgroundWorkLine(detail: detail, emphasis: 'keep Wisp open');

/// A subtitle for work the transfer is doing that has no speed to report:
/// [detail] in muted text, then [emphasis] in the ink colour so the part the
/// user has to act on carries the weight.
///
/// Shared by the two ends of a transfer that used to say nothing at all — the
/// stretch before the first byte, and the stretch after the last.
Widget buildBackgroundWorkLine({
  required String detail,
  required String emphasis,
}) {
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
            text: '  ·  $emphasis',
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

/// The line shown while the receiver is creating its destinations — the seconds
/// between Accept and the first byte, during which the receiver has answered
/// nothing yet and the sender is still waiting on it.
///
/// Counts files rather than bytes because that is what the work is: one
/// MediaStore insert and one open per file, the same cost whatever its size.
Widget buildPreparingToReceiveLine({
  required int created,
  required int total,
}) => buildBackgroundWorkLine(
  detail: 'Preparing to receive… ($created/$total)',
  emphasis: 'keep Wisp open',
);

/// Files above which the sender warns that a wait on the recipient is normal.
///
/// The sender cannot tell the two halves of that wait apart — the recipient may
/// not have tapped yet, or may have tapped and be creating one destination per
/// file, because the answer only goes out once that is done. So the line has to
/// be true either way, and it is only worth saying when the second half is
/// long: preparation costs a few milliseconds a file, so a couple of hundred
/// files is a second or two and the wait is simply the person deciding.
const int preparingHintFileCount = 200;

/// The sender's counterpart, shown while it waits on a large folder's
/// recipient. Claims nothing about whether they have accepted yet.
Widget buildRecipientPreparingLine() => buildBackgroundWorkLine(
  detail: 'A folder this large takes the other device a moment to prepare',
  emphasis: 'keep Wisp open',
);

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
