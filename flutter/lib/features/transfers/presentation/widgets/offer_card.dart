import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:url_launcher/url_launcher.dart';

import '../../application/service.dart';
import '../../application/state.dart';
import '../../../../theme/wisp_theme.dart';
import '../../../saved_devices/application/device_display_name.dart';
import 'package:app/features/send/presentation/widgets/recipient_avatar.dart';
import 'relay_tip_note.dart';
import 'sending_connection_strip.dart';
import 'transfer_flow_layout.dart';
import 'transfer_manifest_panel.dart';
import 'transfer_presentation_helpers.dart';

class OfferCard extends ConsumerWidget {
  const OfferCard({
    super.key,
    required this.offer,
    required this.animate,
    required this.onAccept,
    required this.onAcceptText,
    required this.onDecline,
  });

  final TransferIncomingOffer offer;
  final bool animate;
  final VoidCallback onAccept;

  /// Accept an inline-text offer, tagging how the user consumed it (Copy vs
  /// Save) so the session can skip the progress screen and route the finish
  /// state correctly. [saved] carries the file name + folder for the Save case
  /// (null for Copy) so the finish screen can name the exact location.
  final void Function(TransferTextDelivery mode, SavedTextLocation? saved)
  onAcceptText;
  final VoidCallback onDecline;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    if (offer.inlineText != null) {
      return _TextOfferCard(
        offer: offer,
        animate: animate,
        onAcceptText: onAcceptText,
        onDecline: onDecline,
      );
    }
    final name = resolveDeviceName(
      ref,
      endpointId: offer.senderEndpointId ?? '',
      broadcastLabel: displaySender(offer.sender.displayName),
    );
    final senderName = name.primary;
    final itemCount = offer.manifest.itemCount;
    final totalSize = formatBytes(offer.manifest.totalSizeBytes);
    final willResume = offer.willResume;
    final subtitle = incomingSubtitle(itemCount, totalSize);

    return SizedBox.expand(
      child: TransferFlowLayout(
        statusLabel: 'Incoming',
        statusColor: kAccentCyanStrong,
        subtitle: buildSubtitleWithBroadcast(subtitle, name.broadcast),
        explainer: Text(
          willResume
              ? 'Resuming previous transfer. Wisp will skip files you already have and download the rest. Accept only if you trust the sender.'
              : 'Review the files and accept only if you trust the sender.',
          textAlign: TextAlign.center,
          style: wispSans(
            fontSize: 13,
            fontWeight: FontWeight.w600,
            color: kInk.withValues(alpha: 0.7),
            height: 1.4,
          ),
        ),
        illustration: RecipientAvatar(
          deviceName: senderName,
          deviceType: deviceTypeLabel(offer.sender.deviceType),
          animate: animate,
          mode: SendingStripMode.waitingOnRecipient,
          // Mirrors crates/core/src/transfer/receiver.rs decision timer
          // (`tokio::time::sleep(Duration::from_secs(120))`).  Keep these
          // two values in sync.
          countdownDuration: const Duration(seconds: 120),
        ),
        manifest: TransferManifestPanel(
          mode: TransferManifestPanelMode.previewTree,
          items: offer.manifest.items,
        ),
        footerNote: RelayTipNote(path: offer.connectionPath),
        // Secondary action (Decline) on the left, primary action (Save) on
        // the right — matches the app-wide button convention.
        footer: Row(
          children: [
            Expanded(
              flex: 1,
              child: TextButton(
                onPressed: onDecline,
                // Same soft-destructive treatment as the "Cancel transfer"
                // button (red text + red tint + red border). Radius/height stay
                // at 14/52 to line up with the Save button beside it.
                style: TextButton.styleFrom(
                  foregroundColor: const Color(0xFFB34A4A),
                  backgroundColor: const Color(
                    0xFFB34A4A,
                  ).withValues(alpha: 0.08),
                  minimumSize: const Size(0, 52),
                  shape: RoundedRectangleBorder(
                    borderRadius: BorderRadius.circular(14),
                    side: BorderSide(
                      color: const Color(0xFFB34A4A).withValues(alpha: 0.15),
                    ),
                  ),
                ),
                child: const Text(
                  'Decline',
                  style: TextStyle(fontWeight: FontWeight.w600),
                ),
              ),
            ),
            const SizedBox(width: 12),
            Expanded(
              flex: 2,
              child: FilledButton(
                onPressed: onAccept,
                style: FilledButton.styleFrom(
                  backgroundColor: kAccentCyanStrong,
                  foregroundColor: Colors.white,
                  minimumSize: const Size(0, 52),
                  shape: RoundedRectangleBorder(
                    borderRadius: BorderRadius.circular(14),
                  ),
                  elevation: 0,
                ),
                child: Text(
                  // Stays generic so the label doesn't claim "Save to
                  // Downloads" when the user has picked a different SAF
                  // folder — the Rust side reports the cache root which
                  // always looks like "Downloads" on Android.
                  'Save',
                  style: wispSans(fontWeight: FontWeight.w700, fontSize: 15),
                ),
              ),
            ),
          ],
        ),
      ),
    );
  }
}

/// Incoming text offer: the snippet already arrived inline, so we render it
/// with Copy and Save-as-.txt actions. Both also accept the transfer (so the
/// sender sees "delivered"); Decline rejects it.
class _TextOfferCard extends ConsumerStatefulWidget {
  const _TextOfferCard({
    required this.offer,
    required this.animate,
    required this.onAcceptText,
    required this.onDecline,
  });

  final TransferIncomingOffer offer;
  final bool animate;
  final void Function(TransferTextDelivery mode, SavedTextLocation? saved)
  onAcceptText;
  final VoidCallback onDecline;

  @override
  ConsumerState<_TextOfferCard> createState() => _TextOfferCardState();
}

class _TextOfferCardState extends ConsumerState<_TextOfferCard> {
  bool _busy = false;

  String get _text => widget.offer.inlineText ?? '';

  /// When the whole snippet is a single web link, we surface an "Open" button.
  /// Detection is deliberately strict — the entire trimmed text must be one
  /// http(s) URL with a host — so we never offer to "open" prose that merely
  /// happens to contain a link somewhere inside it.
  Uri? get _detectedLink {
    final trimmed = _text.trim();
    if (trimmed.isEmpty || trimmed.contains(RegExp(r'\s'))) return null;
    final uri = Uri.tryParse(trimmed);
    if (uri == null || uri.host.isEmpty) return null;
    if (uri.scheme != 'http' && uri.scheme != 'https') return null;
    return uri;
  }

  /// Opens the detected link in the system browser. Unlike Copy/Save this
  /// doesn't resolve the offer — the user still picks Copy, Save, or Decline.
  Future<void> _open() async {
    final link = _detectedLink;
    if (link == null) return;
    final ok = await launchUrl(link, mode: LaunchMode.externalApplication);
    if (!ok && mounted) {
      ScaffoldMessenger.of(
        context,
      ).showSnackBar(const SnackBar(content: Text('Couldn\'t open link')));
    }
  }

  Future<void> _copy() async {
    if (_busy) return;
    setState(() => _busy = true);
    await Clipboard.setData(ClipboardData(text: _text));
    if (mounted) {
      ScaffoldMessenger.of(
        context,
      ).showSnackBar(const SnackBar(content: Text('Text copied')));
    }
    widget.onAcceptText(TransferTextDelivery.copy, null);
  }

  /// e.g. "Shared Text 2026-04-23 100708.txt" — a stable, sortable name
  /// stamped with the moment the text was saved, so repeated shares don't all
  /// collapse onto one generic "shared-text.txt". The protocol doesn't carry
  /// whether the snippet came from the clipboard or was typed, so the receiver
  /// can't tell them apart — hence one generic "Shared Text" label.
  String _suggestedFileName() {
    final now = DateTime.now();
    String two(int n) => n.toString().padLeft(2, '0');
    final date = '${now.year}-${two(now.month)}-${two(now.day)}';
    final time = '${two(now.hour)}${two(now.minute)}${two(now.second)}';
    return 'Shared Text $date $time.txt';
  }

  Future<void> _save() async {
    if (_busy) return;
    setState(() => _busy = true);
    try {
      final saved = await ref
          .read(transfersServiceSourceProvider)
          .saveInlineText(suggestedName: _suggestedFileName(), contents: _text);
      if (mounted) {
        ScaffoldMessenger.of(context).showSnackBar(
          SnackBar(
            content: Text('Saved ${saved.fileName} to ${saved.folderLabel}'),
          ),
        );
      }
      widget.onAcceptText(TransferTextDelivery.save, saved);
    } catch (error) {
      if (mounted) {
        setState(() => _busy = false);
        ScaffoldMessenger.of(
          context,
        ).showSnackBar(SnackBar(content: Text('Couldn\'t save: $error')));
      }
    }
  }

  @override
  Widget build(BuildContext context) {
    final name = resolveDeviceName(
      ref,
      endpointId: widget.offer.senderEndpointId ?? '',
      broadcastLabel: displaySender(widget.offer.sender.displayName),
    );
    final senderName = name.primary;

    return SizedBox.expand(
      child: TransferFlowLayout(
        statusLabel: 'Incoming text',
        statusColor: kAccentCyanStrong,
        subtitle: buildSubtitleWithBroadcast('Sent you text', name.broadcast),
        explainer: Text(
          'Copy it or save it as a .txt file. '
          'Accept only if you trust the sender.',
          textAlign: TextAlign.center,
          style: wispSans(
            fontSize: 13,
            fontWeight: FontWeight.w600,
            color: kInk.withValues(alpha: 0.7),
            height: 1.4,
          ),
        ),
        illustration: RecipientAvatar(
          deviceName: senderName,
          deviceType: deviceTypeLabel(widget.offer.sender.deviceType),
          animate: widget.animate,
          mode: SendingStripMode.waitingOnRecipient,
          countdownDuration: const Duration(seconds: 120),
        ),
        // Text body with the primary actions (Copy / Save) docked directly
        // beneath it — they act on the snippet right above, so keeping them
        // adjacent reads more naturally than burying them in the footer bar.
        // Decline stays in the footer (its usual spot) as the safe way out.
        manifest: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            Container(
              width: double.infinity,
              padding: const EdgeInsets.all(14),
              decoration: BoxDecoration(
                color: kSurface,
                borderRadius: BorderRadius.circular(16),
                border: Border.all(color: kBorder),
              ),
              child: ConstrainedBox(
                constraints: const BoxConstraints(maxHeight: 180),
                child: SingleChildScrollView(
                  physics: const BouncingScrollPhysics(),
                  child: SelectableText(
                    _text,
                    style: wispMono(fontSize: 13.5, color: kInk),
                  ),
                ),
              ),
            ),
            const SizedBox(height: 12),
            // If the snippet is a bare link, offer to open it straight away —
            // sits above Save/Copy so it reads as the obvious thing to do with
            // a URL, without displacing the standard actions below.
            if (_detectedLink != null) ...[
              SizedBox(
                width: double.infinity,
                child: OutlinedButton.icon(
                  onPressed: _busy ? null : _open,
                  style: OutlinedButton.styleFrom(
                    foregroundColor: kAccentCyanStrong,
                    minimumSize: const Size(0, 52),
                    side: const BorderSide(color: kAccentCyanStrong),
                    shape: RoundedRectangleBorder(
                      borderRadius: BorderRadius.circular(14),
                    ),
                  ),
                  icon: const Icon(Icons.open_in_new_rounded, size: 18),
                  label: Text(
                    'Open link',
                    style: wispSans(fontWeight: FontWeight.w700, fontSize: 15),
                  ),
                ),
              ),
              const SizedBox(height: 12),
            ],
            // Secondary action (Save .txt) on the left, primary action (Copy)
            // on the right — matches the app-wide button convention.
            Row(
              children: [
                Expanded(
                  child: OutlinedButton.icon(
                    onPressed: _busy ? null : _save,
                    style: OutlinedButton.styleFrom(
                      foregroundColor: kAccentCyanStrong,
                      minimumSize: const Size(0, 52),
                      side: const BorderSide(color: kAccentCyanStrong),
                      shape: RoundedRectangleBorder(
                        borderRadius: BorderRadius.circular(14),
                      ),
                    ),
                    icon: const Icon(Icons.save_alt_rounded, size: 18),
                    label: Text(
                      'Save .txt',
                      style: wispSans(
                        fontWeight: FontWeight.w700,
                        fontSize: 15,
                      ),
                    ),
                  ),
                ),
                const SizedBox(width: 12),
                Expanded(
                  child: FilledButton.icon(
                    onPressed: _busy ? null : _copy,
                    style: FilledButton.styleFrom(
                      backgroundColor: kAccentCyanStrong,
                      foregroundColor: Colors.white,
                      minimumSize: const Size(0, 52),
                      shape: RoundedRectangleBorder(
                        borderRadius: BorderRadius.circular(14),
                      ),
                      elevation: 0,
                    ),
                    icon: const Icon(Icons.content_copy_rounded, size: 18),
                    label: Text(
                      'Copy',
                      style: wispSans(
                        fontWeight: FontWeight.w700,
                        fontSize: 15,
                      ),
                    ),
                  ),
                ),
              ],
            ),
          ],
        ),
        // Standalone footer (no neighbour to align with), so it mirrors the
        // "Cancel transfer" button exactly: red text + red tint + red border,
        // radius 12, full width.
        footer: TextButton(
          onPressed: _busy ? null : widget.onDecline,
          style: TextButton.styleFrom(
            foregroundColor: const Color(0xFFB34A4A),
            backgroundColor: const Color(0xFFB34A4A).withValues(alpha: 0.08),
            minimumSize: const Size(double.infinity, 48),
            shape: RoundedRectangleBorder(
              borderRadius: BorderRadius.circular(12),
              side: BorderSide(
                color: const Color(0xFFB34A4A).withValues(alpha: 0.15),
              ),
            ),
          ),
          child: const Text(
            'Decline',
            style: TextStyle(fontWeight: FontWeight.w600),
          ),
        ),
      ),
    );
  }
}
