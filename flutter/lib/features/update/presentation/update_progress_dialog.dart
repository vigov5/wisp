import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../../theme/wisp_theme.dart';
import '../application/update_providers.dart';
import '../domain/update_status.dart';

/// Shows the download/install progress dialog and kicks off the download.
/// Returns when the dialog is dismissed.
Future<void> showUpdateProgressDialog(BuildContext context, WidgetRef ref) {
  // Fire-and-forget: the dialog reflects controller state reactively.
  ref.read(updateControllerProvider.notifier).downloadAndInstall();
  return showDialog<void>(
    context: context,
    barrierDismissible: false,
    builder: (_) => const _UpdateProgressDialog(),
  );
}

class _UpdateProgressDialog extends ConsumerWidget {
  const _UpdateProgressDialog();

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final state = ref.watch(updateControllerProvider);
    final controller = ref.read(updateControllerProvider.notifier);

    final isError = state.phase == UpdatePhase.error;
    final isReady = state.phase == UpdatePhase.readyToInstall;
    final progress = state.downloadProgress;

    return AlertDialog(
      backgroundColor: kSurface,
      title: Text(
        isError ? 'Update failed' : 'Updating Wisp',
        style: wispSans(fontSize: 16, fontWeight: FontWeight.w700, color: kInk),
      ),
      content: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          if (isError) ...[
            Text(
              state.errorMessage ??
                  'Something went wrong while downloading the update.',
              style: wispSans(fontSize: 13, color: kMuted, height: 1.4),
            ),
          ] else ...[
            Text(
              isReady
                  ? 'Download complete. Launching the installer…'
                  : 'Downloading the latest version…',
              style: wispSans(fontSize: 13, color: kMuted, height: 1.4),
            ),
            const SizedBox(height: 16),
            ClipRRect(
              borderRadius: BorderRadius.circular(6),
              child: LinearProgressIndicator(
                value: isReady ? 1.0 : progress,
                minHeight: 8,
                backgroundColor: kBorder,
                valueColor: const AlwaysStoppedAnimation(kAccentCyanStrong),
              ),
            ),
            if (!isReady && progress != null) ...[
              const SizedBox(height: 8),
              Text(
                '${(progress * 100).round()}%',
                style: wispSans(fontSize: 12, color: kMuted),
              ),
            ],
          ],
        ],
      ),
      actions: isError
          ? [
              TextButton(
                onPressed: () => Navigator.of(context).pop(),
                child: const Text('Close'),
              ),
              FilledButton(
                onPressed: () {
                  controller.openUpdatePage();
                  Navigator.of(context).pop();
                },
                style: FilledButton.styleFrom(
                  backgroundColor: kAccentCyanStrong,
                ),
                child: const Text('Update manually'),
              ),
            ]
          : null,
    );
  }
}
