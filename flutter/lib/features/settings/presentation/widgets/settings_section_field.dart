import 'package:flutter/material.dart';

import '../../../../theme/wisp_theme.dart';

class SettingsSectionField extends StatelessWidget {
  const SettingsSectionField({
    super.key,
    required this.label,
    required this.child,
  });

  final String label;
  final Widget child;

  @override
  Widget build(BuildContext context) {
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Text(
          label,
          style: wispSans(
            fontSize: 13.5,
            fontWeight: FontWeight.w600,
            color: context.wc.ink,
          ),
        ),
        const SizedBox(height: 10),
        child,
      ],
    );
  }
}
