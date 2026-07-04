import 'package:flutter/material.dart';

import '../../../../theme/wisp_theme.dart';

class SettingsToggleField extends StatelessWidget {
  const SettingsToggleField({
    super.key,
    required this.title,
    required this.subtitle,
    required this.value,
    required this.onChanged,
  });

  final String title;
  final String subtitle;
  final bool value;
  final ValueChanged<bool> onChanged;

  @override
  Widget build(BuildContext context) {
    return Row(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Expanded(
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Text(
                title,
                style: wispSans(
                  fontSize: 13.5,
                  fontWeight: FontWeight.w600,
                  color: context.wc.ink,
                ),
              ),
              const SizedBox(height: 4),
              Text(
                subtitle,
                style: wispSans(
                  fontSize: 11.5,
                  fontWeight: FontWeight.w400,
                  color: context.wc.muted,
                  height: 1.45,
                ),
              ),
            ],
          ),
        ),
        const SizedBox(width: 12),
        Padding(
          padding: const EdgeInsets.only(top: 2),
          child: Switch(
            value: value,
            onChanged: onChanged,
            thumbColor: WidgetStateProperty.resolveWith((states) {
              if (states.contains(WidgetState.selected)) {
                return Colors.white;
              }
              return Colors.white;
            }),
            trackColor: WidgetStateProperty.resolveWith((states) {
              if (states.contains(WidgetState.selected)) {
                return kAccentCyanStrong;
              }
              return context.wc.border;
            }),
          ),
        ),
      ],
    );
  }
}
