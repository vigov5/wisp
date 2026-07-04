import 'package:flutter/material.dart';
import '../../../theme/wisp_theme.dart';

class SelectFilesCard extends StatelessWidget {
  final VoidCallback? onTap;
  final IconData icon;
  final String title;
  final String subtitle;

  const SelectFilesCard({
    super.key,
    required this.onTap,
    this.icon = Icons.add_rounded,
    this.title = 'Select files',
    this.subtitle = 'Tap to choose files to send.',
  });

  @override
  Widget build(BuildContext context) {
    return InkWell(
      onTap: onTap,
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
                    subtitle,
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
