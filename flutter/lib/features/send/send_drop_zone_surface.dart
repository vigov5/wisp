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
        padding: const EdgeInsets.symmetric(horizontal: 40, vertical: 42),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          mainAxisAlignment: MainAxisAlignment.center,
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
                  Icons.drive_folder_upload_outlined,
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
            Wrap(
              alignment: WrapAlignment.center,
              spacing: 10,
              runSpacing: 10,
              children: [
                _DropZoneAction(
                  icon: Icons.insert_drive_file_outlined,
                  label: 'Select files',
                  onPressed: onChooseFiles,
                ),
                _DropZoneAction(
                  icon: Icons.notes_rounded,
                  label: 'Share text',
                  onPressed: onShareText,
                ),
                _DropZoneAction(
                  icon: Icons.content_paste_rounded,
                  label: 'Share clipboard',
                  onPressed: onShareClipboard,
                ),
              ],
            ),
          ],
        ),
      ),
    );
  }
}

class _DropZoneAction extends StatelessWidget {
  const _DropZoneAction({
    required this.icon,
    required this.label,
    required this.onPressed,
  });

  final IconData icon;
  final String label;
  final Future<void> Function() onPressed;

  @override
  Widget build(BuildContext context) {
    return OutlinedButton.icon(
      onPressed: () => unawaited(onPressed()),
      style: OutlinedButton.styleFrom(
        backgroundColor: Colors.white.withValues(alpha: 0.35),
        foregroundColor: const Color(0xFF444444),
        minimumSize: const Size(0, 32),
        padding: const EdgeInsets.symmetric(horizontal: 13, vertical: 7),
        side: const BorderSide(color: Color(0xFFE7E7E7), width: 0.9),
        textStyle: wispSans(fontSize: 12.5, fontWeight: FontWeight.w600),
      ),
      icon: Icon(icon, size: 15),
      label: Text(label),
    );
  }
}
