import 'dart:io';
import 'package:flutter/material.dart';
import 'package:window_manager/window_manager.dart';

import '../theme/wisp_theme.dart';
import 'widgets/app_version_text.dart';

class TitleBarShell extends StatelessWidget {
  const TitleBarShell({super.key, required this.child});

  final Widget child;

  @override
  Widget build(BuildContext context) {
    final isDesktop =
        Platform.isMacOS || Platform.isWindows || Platform.isLinux;

    return Scaffold(
      body: Column(
        children: [
          if (isDesktop)
            _DesktopTitleBar(showWindowControls: Platform.isWindows),
          Expanded(child: child),
          // Android 15 (targetSdk 35) enforces edge-to-edge, so the system
          // navigation bar draws over the bottom of the window. Without a
          // bottom inset the gesture/3-button nav bar covers the "Get Wisp for
          // desktop" link and swallows its taps. SafeArea(top: false) lifts the
          // version row above the nav bar; on desktop the inset is 0 (no-op).
          SafeArea(
            top: false,
            child: Padding(
              padding: const EdgeInsets.fromLTRB(12, 0, 12, 8),
              child: Align(
                alignment: Alignment.centerLeft,
                child: const AppVersionText(),
              ),
            ),
          ),
        ],
      ),
    );
  }
}

class _DesktopTitleBar extends StatelessWidget {
  const _DesktopTitleBar({required this.showWindowControls});

  final bool showWindowControls;

  @override
  Widget build(BuildContext context) {
    return DragToMoveArea(
      child: SizedBox(
        height: 40,
        child: Row(
          children: [
            const SizedBox(width: 12),
            // App label — replaces the native title bar text we hid via
            // `TitleBarStyle.hidden`, so the window is identifiable at a
            // glance like a normal Windows app.  Maximize button is
            // intentionally omitted because the window is fixed-size
            // (see main.dart: maximumSize == initialSize).
            Text(
              'Wisp',
              style: wispSans(
                fontSize: 13,
                fontWeight: FontWeight.w700,
                color: kInk,
                letterSpacing: -0.2,
              ),
            ),
            const Expanded(child: SizedBox.shrink()),
            if (showWindowControls) ...[
              _TitleBarButton(
                icon: Icons.remove_rounded,
                tooltip: 'Minimize',
                onTap: () => windowManager.minimize(),
              ),
              _TitleBarButton(
                icon: Icons.close_rounded,
                tooltip: 'Close',
                onTap: () => windowManager.close(),
              ),
              const SizedBox(width: 4),
            ],
          ],
        ),
      ),
    );
  }
}

class _TitleBarButton extends StatelessWidget {
  const _TitleBarButton({
    required this.icon,
    required this.tooltip,
    required this.onTap,
  });

  final IconData icon;
  final String tooltip;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    return Tooltip(
      message: tooltip,
      child: InkWell(
        onTap: onTap,
        borderRadius: BorderRadius.circular(6),
        child: SizedBox(
          width: 36,
          height: 36,
          child: Icon(icon, size: 16, color: kMuted),
        ),
      ),
    );
  }
}
