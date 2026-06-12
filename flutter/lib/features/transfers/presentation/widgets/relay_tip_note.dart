import 'package:flutter/material.dart';

import '../../../../theme/wisp_theme.dart';
import '../../application/connection_path.dart';

/// Advisory shown just above the action bar when the active transfer is running
/// over a relay instead of a direct P2P path.
///
/// Wisp uses iroh's public relays, which are "free to use for development and
/// testing" but where "throughput through public relays is rate-limited" to
/// prevent abuse (https://docs.iroh.computer/about/faq). A relayed transfer can
/// therefore be noticeably slower and may be throttled, so we surface a small,
/// non-blocking heads-up — but only while the connection is actually relayed.
class RelayTipNote extends StatelessWidget {
  const RelayTipNote({super.key, required this.path});

  final ConnectionPathInfo? path;

  @override
  Widget build(BuildContext context) {
    final info = path;
    if (info == null || !info.isRelay) {
      return const SizedBox.shrink();
    }

    return Padding(
      padding: const EdgeInsets.fromLTRB(24, 0, 24, 12),
      child: Container(
        padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 10),
        decoration: BoxDecoration(
          color: kAccentRelay.withValues(alpha: 0.08),
          borderRadius: BorderRadius.circular(12),
          border: Border.all(color: kAccentRelay.withValues(alpha: 0.25)),
        ),
        child: Row(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Icon(Icons.info_outline_rounded, size: 16, color: kAccentRelay),
            const SizedBox(width: 8),
            Expanded(
              child: Text.rich(
                TextSpan(
                  children: const [
                    TextSpan(
                      text: 'Tip: ',
                      style: TextStyle(fontWeight: FontWeight.w800),
                    ),
                    TextSpan(
                      text:
                          'Connected through a free public relay instead of a '
                          'direct link. Speed is limited and may be throttled '
                          'to prevent abuse, so this transfer can be slower '
                          'than a direct connection.',
                    ),
                  ],
                ),
                style: wispSans(
                  fontSize: 11.5,
                  fontWeight: FontWeight.w600,
                  color: kInk.withValues(alpha: 0.75),
                  height: 1.35,
                ),
              ),
            ),
          ],
        ),
      ),
    );
  }
}
