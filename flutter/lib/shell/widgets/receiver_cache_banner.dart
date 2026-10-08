import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../features/settings/application/receiver_cache.dart';
import '../../theme/wisp_theme.dart';

/// Startup warning for a receiver cache that has grown past
/// [kReceiverCacheWarningBytes].
///
/// It sits directly above the version footer: visible on every screen, but
/// without taking the top of the window away from the transfer itself. Same
/// amber as the firewall banner, which says the same kind of thing — nothing
/// is broken, but something wants attention.
///
/// Measured once per download root by [receiverCacheProvider], so walking the
/// tree happens at startup rather than on every navigation.
class ReceiverCacheBanner extends ConsumerWidget {
  const ReceiverCacheBanner({super.key});

  Future<void> _clean(BuildContext context, WidgetRef ref) async {
    final messenger = ScaffoldMessenger.of(context);
    final failure = await ref.read(receiverCacheProvider.notifier).clear();
    messenger.showSnackBar(
      SnackBar(
        content: Text(
          failure == null
              ? 'Receiver cache cleared'
              : "Couldn't clear cache: $failure",
        ),
      ),
    );
  }

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final cache = ref.watch(receiverCacheProvider);
    if (!cache.shouldWarn) return const SizedBox.shrink();
    // The receiver is still using the folder; clearing it now would delete
    // the transfer out from under itself.
    final busy = ref.watch(receiverCacheBusyProvider);

    return Container(
      margin: const EdgeInsets.only(bottom: 8),
      padding: const EdgeInsets.fromLTRB(12, 10, 4, 10),
      decoration: BoxDecoration(
        color: context.wc.warnBg,
        border: Border.all(color: context.wc.warnBorder, width: 0.8),
        borderRadius: BorderRadius.circular(12),
      ),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Icon(Icons.warning_amber_rounded, size: 18, color: context.wc.warnFg),
          const SizedBox(width: 10),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(
                  'Receiver cache is using '
                  '${formatCacheBytes(cache.sizeBytes ?? 0)}',
                  style: wispSans(
                    fontSize: 13,
                    fontWeight: FontWeight.w700,
                    color: context.wc.warnInk,
                  ),
                ),
                const SizedBox(height: 3),
                Text(
                  busy
                      ? 'Waiting for the transfer on screen to finish — '
                            'clearing now would cancel it.'
                      : 'Leftovers from earlier transfers. Clearing frees the '
                            'space; files already saved stay where they are, '
                            'but anything half-received loses its place.',
                  style: wispSans(
                    fontSize: 12,
                    fontWeight: FontWeight.w400,
                    color: context.wc.warnInk,
                    height: 1.35,
                  ),
                ),
                const SizedBox(height: 8),
                OutlinedButton(
                  onPressed: cache.clearing || busy
                      ? null
                      : () => _clean(context, ref),
                  style: OutlinedButton.styleFrom(
                    foregroundColor: context.wc.warnInk,
                    disabledForegroundColor: context.wc.warnInk.withValues(
                      alpha: 0.4,
                    ),
                    side: BorderSide(color: context.wc.warnBorder),
                    visualDensity: VisualDensity.compact,
                    padding: const EdgeInsets.symmetric(
                      horizontal: 12,
                      vertical: 4,
                    ),
                  ),
                  child: Text(cache.clearing ? 'Cleaning…' : 'Clean now'),
                ),
              ],
            ),
          ),
          IconButton(
            icon: const Icon(Icons.close_rounded, size: 18),
            color: context.wc.warnInk,
            tooltip: 'Dismiss',
            visualDensity: VisualDensity.compact,
            onPressed: () => ref.read(receiverCacheProvider.notifier).dismiss(),
          ),
        ],
      ),
    );
  }
}
