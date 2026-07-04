import 'dart:async';

import 'package:flutter/material.dart';

import '../../theme/wisp_theme.dart';

class SendDropZoneSurface extends StatelessWidget {
  const SendDropZoneSurface({
    super.key,
    required this.isInteractive,
    required this.onChooseFiles,
    required this.onShareText,
    required this.onShareClipboard,
  });

  final bool isInteractive;
  final Future<void> Function() onChooseFiles;
  final Future<void> Function() onShareText;
  final Future<void> Function() onShareClipboard;

  @override
  Widget build(BuildContext context) {
    return AnimatedContainer(
      duration: const Duration(milliseconds: 180),
      curve: Curves.easeOutCubic,
      padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 12),
      decoration: BoxDecoration(
        color: isInteractive ? context.wc.fill : context.wc.bg,
        borderRadius: BorderRadius.circular(16),
        border: Border.all(
          color: isInteractive ? context.wc.subtle : context.wc.border,
        ),
        boxShadow: const [
          BoxShadow(
            color: Color(0x10000000),
            blurRadius: 20,
            offset: Offset(0, 10),
          ),
        ],
      ),
      child: Padding(
        padding: const EdgeInsets.symmetric(horizontal: 32, vertical: 24),
        // Center when there's room; scroll instead of overflowing on a short
        // window now that the actions are full-size cards rather than a
        // compact button row.
        child: Center(
          child: SingleChildScrollView(
            child: Column(
              mainAxisSize: MainAxisSize.min,
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                Center(
                  child: AnimatedContainer(
                    duration: const Duration(milliseconds: 180),
                    curve: Curves.easeOutCubic,
                    width: 42,
                    height: 42,
                    decoration: BoxDecoration(
                      color: context.wc.surface,
                      borderRadius: BorderRadius.circular(15),
                      border: Border.all(color: context.wc.border),
                    ),
                    child: Icon(
                      Icons.upload_file_outlined,
                      size: 18,
                      color: isInteractive
                          ? context.wc.muted
                          : context.wc.muted.withValues(alpha: 0.72),
                    ),
                  ),
                ),
                const SizedBox(height: 14),
                Text(
                  'Drop files to send',
                  style: wispSans(
                    fontSize: 24,
                    fontWeight: FontWeight.w600,
                    color: context.wc.ink,
                    letterSpacing: -0.7,
                  ),
                  textAlign: TextAlign.center,
                ),
                const SizedBox(height: 6),
                Text(
                  'or share text and clipboard',
                  style: wispSans(
                    fontSize: 13,
                    fontWeight: FontWeight.w500,
                    color: context.wc.muted.withValues(alpha: 0.85),
                  ),
                  textAlign: TextAlign.center,
                ),
                const SizedBox(height: 22),
                // One column, three rows — mirrors the mobile SelectFilesCard
                // layout (icon + title + description) so both platforms read
                // the same.
                _DropZoneAction(
                  icon: Icons.insert_drive_file_outlined,
                  title: 'Share file',
                  description: 'Send files or a folder.',
                  onPressed: onChooseFiles,
                ),
                const SizedBox(height: 12),
                _DropZoneAction(
                  icon: Icons.notes_rounded,
                  title: 'Share text',
                  description: 'Type or paste text to send.',
                  onPressed: onShareText,
                ),
                const SizedBox(height: 12),
                _DropZoneAction(
                  icon: Icons.content_paste_rounded,
                  title: 'Share clipboard',
                  description: 'Send what\'s on your clipboard.',
                  onPressed: onShareClipboard,
                ),
              ],
            ),
          ),
        ),
      ),
    );
  }
}

/// Full-width action card matching the mobile `SelectFilesCard` design: an icon
/// tile, a bold title and a one-line description, stacked one per row.
class _DropZoneAction extends StatelessWidget {
  const _DropZoneAction({
    required this.icon,
    required this.title,
    required this.description,
    required this.onPressed,
  });

  final IconData icon;
  final String title;
  final String description;
  final Future<void> Function() onPressed;

  @override
  Widget build(BuildContext context) {
    return InkWell(
      onTap: () => unawaited(onPressed()),
      borderRadius: BorderRadius.circular(24),
      child: Container(
        padding: const EdgeInsets.all(20),
        decoration: BoxDecoration(
          color: context.wc.surface,
          borderRadius: BorderRadius.circular(24),
          border: Border.all(color: context.wc.border),
        ),
        child: Row(
          children: [
            Container(
              padding: const EdgeInsets.all(12),
              decoration: BoxDecoration(
                color: context.wc.bg,
                borderRadius: BorderRadius.circular(14),
              ),
              child: Icon(icon, color: context.wc.ink, size: 24),
            ),
            const SizedBox(width: 16),
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(
                    title,
                    style: wispSans(
                      fontSize: 16,
                      fontWeight: FontWeight.w700,
                      color: context.wc.ink,
                    ),
                  ),
                  const SizedBox(height: 2),
                  Text(
                    description,
                    style: wispSans(
                      fontSize: 14,
                      color: context.wc.muted,
                      height: 1.3,
                    ),
                  ),
                ],
              ),
            ),
          ],
        ),
      ),
    );
  }
}
