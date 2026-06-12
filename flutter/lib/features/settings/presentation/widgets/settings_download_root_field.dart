import 'package:flutter/material.dart';

import '../../../../theme/wisp_theme.dart';

class SettingsDownloadRootField extends StatelessWidget {
  const SettingsDownloadRootField({
    super.key,
    required this.controller,
    required this.onChoose,
  });

  final TextEditingController controller;
  final VoidCallback onChoose;

  @override
  Widget build(BuildContext context) {
    // Choose used to live as a suffix inside the TextField, which left an
    // unavoidable inner padding so the button never sat flush against the
    // field's right edge. Split into a Row so the path text gets the full
    // remaining width and the Choose button's right edge aligns with the
    // field's right edge.
    return Row(
      crossAxisAlignment: CrossAxisAlignment.center,
      children: [
        Expanded(
          child: TextField(
            key: const ValueKey<String>('settings-download-root-field'),
            controller: controller,
            readOnly: true,
            showCursor: false,
            onTap: onChoose,
            decoration: const InputDecoration(hintText: '/Users/you/Downloads'),
          ),
        ),
        const SizedBox(width: 8),
        // Soft-tint secondary style (see drift button-style conventions):
        // accent foreground, accent fill at 0.08, accent border at 0.15.
        OutlinedButton(
          onPressed: onChoose,
          style: OutlinedButton.styleFrom(
            minimumSize: const Size(0, 48),
            padding: const EdgeInsets.symmetric(horizontal: 14),
            foregroundColor: kAccentCyanStrong,
            backgroundColor: kAccentCyanStrong.withValues(alpha: 0.08),
            side: BorderSide(color: kAccentCyanStrong.withValues(alpha: 0.15)),
            shape: RoundedRectangleBorder(
              borderRadius: BorderRadius.circular(8),
            ),
          ),
          child: Text(
            'Choose',
            style: wispSans(
              fontSize: 13,
              fontWeight: FontWeight.w600,
              color: kAccentCyanStrong,
            ),
          ),
        ),
      ],
    );
  }
}
