import 'dart:async';

import 'package:flutter/material.dart';

import '../../../platform/android_file_picker.dart';
import '../../../theme/wisp_theme.dart';
import '../../transfers/application/format_utils.dart';

/// Modal shown while the native picker streams a large selection into the app
/// cache. Bound to [AndroidFilePicker.pickProgress] so it animates as bytes
/// are copied; the caller pops it once the pick future resolves.
class PickProgressDialog extends StatefulWidget {
  const PickProgressDialog({super.key});

  @override
  State<PickProgressDialog> createState() => _PickProgressDialogState();
}

class _PickProgressDialogState extends State<PickProgressDialog> {
  /// Latched so a second tap cannot re-issue the cancel, and so the button can
  /// say that the request was heard: the unwind takes until the current file's
  /// binder call returns, which on a slow provider is visible.
  bool _cancelling = false;

  @override
  Widget build(BuildContext context) {
    return ValueListenableBuilder<AndroidPickProgress?>(
      valueListenable: AndroidFilePicker.pickProgress,
      builder: (context, progress, _) {
        final fraction = progress?.fraction;
        final copied = progress?.bytesCopied ?? 0;
        final total = progress?.totalBytes ?? 0;
        final multiple = (progress?.count ?? 1) > 1;
        // Opening a descriptor per file is not copying, and saying "copying"
        // through half a minute of it is both wrong and alarming — the whole
        // point of the descriptor path is that nothing is duplicated.
        final countsFiles = progress?.countsFiles ?? false;

        return AlertDialog(
          backgroundColor: context.wc.surface,
          title: Text(
            'Preparing files…',
            style: wispSans(
              fontSize: 16,
              fontWeight: FontWeight.w700,
              color: context.wc.ink,
            ),
          ),
          actions: [
            TextButton(
              // Enabled through the whole preparing phase, which is the point:
              // a 1911-file folder spends 27-37 s here and the only way out
              // used to be killing the app.
              onPressed: _cancelling
                  ? null
                  : () {
                      setState(() => _cancelling = true);
                      unawaited(AndroidFilePicker.cancelPick());
                    },
              style: TextButton.styleFrom(foregroundColor: kDanger),
              child: Text(
                _cancelling ? 'Cancelling…' : 'Cancel',
                style: wispSans(fontSize: 14, fontWeight: FontWeight.w700),
              ),
            ),
          ],
          content: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Text(
                countsFiles
                    ? 'Opening the selected files so they are ready to send. '
                          'Nothing is being copied.'
                    : multiple
                    ? 'Copying selected files so they are ready to send.'
                    : 'Copying the selected file so it is ready to send.',
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
                  value: fraction,
                  minHeight: 8,
                  backgroundColor: context.wc.border,
                  valueColor: const AlwaysStoppedAnimation(kAccentCyanStrong),
                ),
              ),
              const SizedBox(height: 8),
              Text(
                countsFiles
                    ? '${progress?.index ?? 0} / ${progress?.count ?? 0} files'
                          '${fraction != null ? '  ·  ${(fraction * 100).round()}%' : ''}'
                    : total > 0
                    ? '${formatBytes(BigInt.from(copied))} / ${formatBytes(BigInt.from(total))}'
                          '${fraction != null ? '  ·  ${(fraction * 100).round()}%' : ''}'
                    : formatBytes(BigInt.from(copied)),
                style: wispSans(fontSize: 12, color: context.wc.muted),
              ),
            ],
          ),
        );
      },
    );
  }
}
