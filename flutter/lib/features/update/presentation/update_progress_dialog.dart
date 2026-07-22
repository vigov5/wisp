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
    final isManual = state.phase == UpdatePhase.manualInstall;
    final isReady = state.phase == UpdatePhase.readyToInstall;
    final progress = state.downloadProgress;

    final title = isError
        ? 'Update failed'
        : isManual
        ? 'Finish the update'
        : 'Updating Wisp';

    return AlertDialog(
      backgroundColor: context.wc.surface,
      title: Text(
        title,
        style: wispSans(
          fontSize: 16,
          fontWeight: FontWeight.w700,
          color: context.wc.ink,
        ),
      ),
      content: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          if (isError) ...[
            Text(
              state.errorMessage ??
                  'Something went wrong while downloading the update.',
              style: wispSans(
                fontSize: 13,
                color: context.wc.muted,
                height: 1.4,
              ),
            ),
          ] else if (isManual) ...[
            Text(
              'The installer is downloaded but couldn\'t start on its own. '
              'We\'ve opened its folder — double-click it to finish, then '
              'Wisp will close so its files can be replaced.',
              style: wispSans(
                fontSize: 13,
                color: context.wc.muted,
                height: 1.4,
              ),
            ),
            if (_installerName(state.installerPath) != null) ...[
              const SizedBox(height: 12),
              Text(
                _installerName(state.installerPath)!,
                style: wispSans(
                  fontSize: 12.5,
                  fontWeight: FontWeight.w700,
                  color: context.wc.ink,
                ),
              ),
            ],
          ] else ...[
            Text(
              isReady
                  ? 'Download complete. Opening the installer…'
                  : 'Downloading the latest version…',
              style: wispSans(
                fontSize: 13,
                color: context.wc.muted,
                height: 1.4,
              ),
            ),
            const SizedBox(height: 16),
            ClipRRect(
              borderRadius: BorderRadius.circular(6),
              child: LinearProgressIndicator(
                value: isReady ? 1.0 : progress,
                minHeight: 8,
                backgroundColor: context.wc.border,
                valueColor: const AlwaysStoppedAnimation(kAccentCyanStrong),
              ),
            ),
            if (!isReady && progress != null) ...[
              const SizedBox(height: 8),
              Text(
                '${(progress * 100).round()}%',
                style: wispSans(fontSize: 12, color: context.wc.muted),
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
          : isManual
          ? [
              TextButton(
                onPressed: () => Navigator.of(context).pop(),
                child: const Text('Close'),
              ),
              FilledButton(
                onPressed: controller.revealDownloadedInstaller,
                style: FilledButton.styleFrom(
                  backgroundColor: kAccentCyanStrong,
                ),
                child: const Text('Open folder'),
              ),
            ]
          : null,
    );
  }

  /// The installer's file name (last path segment) for display, or null when no
  /// path is set. Splits on both separators so it works regardless of platform.
  String? _installerName(String? path) {
    if (path == null || path.isEmpty) return null;
    final segments = path.split(RegExp(r'[\\/]'));
    return segments.isEmpty ? null : segments.last;
  }
}
