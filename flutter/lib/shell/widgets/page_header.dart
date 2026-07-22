import 'package:flutter/material.dart';

import '../../theme/wisp_theme.dart';

/// Shared page header (back button + title) matching the Settings screen.
///
/// Pushed full-screen pages (USB transfer, Pair via QR, …) previously used a
/// Material [AppBar], whose `kToolbarHeight` and top-left leading button don't
/// line up with the Settings header and — on desktop — collide with the window
/// controls / macOS traffic lights drawn at the very top of the window. Using
/// this header inside the same `SafeArea` + `Padding(top: 24)` wrapper Settings
/// uses keeps the title size and top spacing consistent across all three.
class PageHeader extends StatelessWidget {
  const PageHeader({super.key, required this.title, this.onBack});

  final String title;

  /// Defaults to [Navigator.maybePop]; pass a custom handler for pages that
  /// need to intercept back (e.g. a dirty-state guard).
  final VoidCallback? onBack;

  @override
  Widget build(BuildContext context) {
    return Row(
      children: [
        IconButton(
          onPressed: onBack ?? () => Navigator.of(context).maybePop(),
          icon: const Icon(Icons.arrow_back_rounded),
          tooltip: 'Back',
        ),
        const SizedBox(width: 4),
        Text(
          title,
          style: wispSans(
            fontSize: 22,
            fontWeight: FontWeight.w700,
            color: context.wc.ink,
            letterSpacing: -0.35,
          ),
        ),
      ],
    );
  }
}
