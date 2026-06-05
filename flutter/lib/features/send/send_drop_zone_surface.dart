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
        color: isInteractive ? const Color(0xFFECEDED) : kBg,
        borderRadius: BorderRadius.circular(16),
        border: Border.all(
          color: isInteractive ? const Color(0xFFCED3D4) : kBorder,
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
                      color: isInteractive
                          ? const Color(0xFFF4F4F4)
                          : const Color(0xFFF7F7F7),
                      borderRadius: BorderRadius.circular(15),
                      border: Border.all(
                        color: isInteractive
                            ? const Color(0xFFE2E2E2)
                            : const Color(0xFFE9E9E9),
                      ),
                    ),
                    child: Icon(
                      Icons.upload_file_outlined,
                      size: 18,
                      color: isInteractive
                          ? const Color(0xFF666666)
                          : kMuted.withValues(alpha: 0.72),
                    ),
                  ),
                ),
                const SizedBox(height: 14),
                Text(
                  'Drop files to send',
                  style: wispSans(
                    fontSize: 24,
                    fontWeight: FontWeight.w600,
                    color: kInk,
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
                    color: kMuted.withValues(alpha: 0.85),
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
          color: kSurface,
          borderRadius: BorderRadius.circular(24),
          border: Border.all(color: kBorder),
        ),
        child: Row(
          children: [
            Container(
              padding: const EdgeInsets.all(12),
              decoration: BoxDecoration(
                color: kBg,
                borderRadius: BorderRadius.circular(14),
              ),
              child: Icon(icon, color: kInk, size: 24),
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
                      color: kInk,
                    ),
                  ),
                  const SizedBox(height: 2),
                  Text(
                    description,
                    style: wispSans(fontSize: 14, color: kMuted, height: 1.3),
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
