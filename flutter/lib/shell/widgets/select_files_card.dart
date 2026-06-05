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
                    subtitle,
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
