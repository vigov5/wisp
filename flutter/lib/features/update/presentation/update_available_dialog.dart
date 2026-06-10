import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:url_launcher/url_launcher.dart';

import '../../../theme/wisp_theme.dart';
import '../application/update_providers.dart';
import '../domain/update_release.dart';
import 'update_progress_dialog.dart';

/// Shows the "update available" prompt for [release]. Safe to call with any
/// [BuildContext] that has a [Navigator] above it.
Future<void> showUpdateAvailableDialog(
  BuildContext context,
  WidgetRef ref,
  UpdateRelease release,
) {
  return showDialog<void>(
    context: context,
    builder: (_) => _UpdateAvailableDialog(release: release),
  );
}

class _UpdateAvailableDialog extends ConsumerWidget {
  const _UpdateAvailableDialog({required this.release});

  final UpdateRelease release;

  Future<void> _openChangelog() async {
    if (release.htmlUrl.isEmpty) return;
    await launchUrl(
      Uri.parse(release.htmlUrl),
      mode: LaunchMode.externalApplication,
    );
  }

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final controller = ref.read(updateControllerProvider.notifier);
    // Windows downloads + installs in-app; other platforms open the page.
    final canAutoInstall =
        Platform.isWindows && release.assetForCurrentPlatform() != null;

    return AlertDialog(
      backgroundColor: kSurface,
      title: Text(
        'Update available',
        style: wispSans(fontSize: 16, fontWeight: FontWeight.w700, color: kInk),
      ),
      contentPadding: const EdgeInsets.fromLTRB(24, 16, 24, 20),
      content: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Text(
            'Wisp ${release.version} is ready to install.',
            style: wispSans(
              fontSize: 13.5,
              fontWeight: FontWeight.w600,
              color: kInk,
            ),
          ),
          if (release.htmlUrl.isNotEmpty) ...[
            const SizedBox(height: 14),
            Align(
              alignment: Alignment.centerLeft,
              child: InkWell(
                onTap: _openChangelog,
                borderRadius: BorderRadius.circular(4),
                child: Padding(
                  padding: const EdgeInsets.symmetric(vertical: 2),
                  child: Text(
                    'Full changelog',
                    style: wispSans(
                      fontSize: 13,
                      fontWeight: FontWeight.w600,
                      color: kAccentCyanStrong,
                    ),
                  ),
                ),
              ),
            ),
          ],
          const SizedBox(height: 22),
          FilledButton(
            onPressed: () {
              // Capture the Navigator's own context before popping — the
              // dialog's build context is defunct once this route is gone.
              final navigator = Navigator.of(context);
              navigator.pop();
              if (canAutoInstall) {
                showUpdateProgressDialog(navigator.context, ref);
              } else {
                controller.openReleasesPage();
              }
            },
            style: FilledButton.styleFrom(
              backgroundColor: kAccentCyanStrong,
              foregroundColor: Colors.white,
              minimumSize: const Size(0, 46),
            ),
            child: Text(canAutoInstall ? 'Update now' : 'Download'),
          ),
          const SizedBox(height: 6),
          Row(
            mainAxisAlignment: MainAxisAlignment.center,
            mainAxisSize: MainAxisSize.min,
            children: [
              TextButton(
                onPressed: () => Navigator.of(context).pop(),
                style: TextButton.styleFrom(
                  minimumSize: const Size(0, 36),
                  padding: const EdgeInsets.symmetric(horizontal: 10),
                ),
                child: Text(
                  'Later',
                  style: wispSans(fontSize: 13, color: kMuted),
                ),
              ),
              Text('·', style: wispSans(fontSize: 13, color: kMuted)),
              TextButton(
                onPressed: () {
                  controller.skipCurrentVersion();
                  Navigator.of(context).pop();
                },
                style: TextButton.styleFrom(
                  minimumSize: const Size(0, 36),
                  padding: const EdgeInsets.symmetric(horizontal: 10),
                ),
                child: Text(
                  'Skip this version',
                  style: wispSans(fontSize: 13, color: kMuted),
                ),
              ),
            ],
          ),
        ],
      ),
      actions: const [],
      actionsPadding: EdgeInsets.zero,
    );
  }
}
