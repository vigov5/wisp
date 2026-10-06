import 'dart:io';

import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';
import 'package:package_info_plus/package_info_plus.dart';
import 'package:url_launcher/url_launcher.dart';

import '../../theme/wisp_theme.dart';

/// GitHub releases page — where desktop builds (macOS/Windows/Linux) live.
/// Shown next to the version on mobile so an Android/iOS user who just
/// installed Wisp can find the "other half" they need on their computer.
const _desktopDownloadUrl = 'https://github.com/vigov5/wisp/releases';

/// The browser build. The other half of the same answer: when the device at
/// the far end can't install anything — a work laptop, someone else's machine
/// — it can still receive from a tab.
const _webAppUrl = 'https://web.wisp.mooo.com';

class AppVersionText extends StatefulWidget {
  const AppVersionText({super.key});

  @override
  State<AppVersionText> createState() => _AppVersionTextState();
}

class _AppVersionTextState extends State<AppVersionText> {
  String? _version;
  TapGestureRecognizer? _desktopRecognizer;
  TapGestureRecognizer? _webRecognizer;

  @override
  void initState() {
    super.initState();
    PackageInfo.fromPlatform().then((info) {
      if (mounted) {
        setState(() => _version = 'v${info.version}');
      }
    });
  }

  @override
  void dispose() {
    _desktopRecognizer?.dispose();
    _webRecognizer?.dispose();
    super.dispose();
  }

  Future<void> _open(String url) async {
    await launchUrl(Uri.parse(url), mode: LaunchMode.externalApplication);
  }

  @override
  Widget build(BuildContext context) {
    final v = _version;
    if (v == null) return const SizedBox.shrink();

    final baseStyle = wispSans(
      fontSize: 11,
      fontWeight: FontWeight.w400,
      color: context.wc.muted,
    );

    // Desktop users already have the desktop app, and the web app is for
    // devices that can't install one — neither link earns its space here.
    final isMobile = Platform.isAndroid || Platform.isIOS;
    if (!isMobile) {
      return Text(v, style: baseStyle);
    }

    final linkStyle = baseStyle.copyWith(
      color: context.wc.accentFg,
      fontWeight: FontWeight.w600,
    );
    _desktopRecognizer ??= TapGestureRecognizer()
      ..onTap = () => _open(_desktopDownloadUrl);
    _webRecognizer ??= TapGestureRecognizer()..onTap = () => _open(_webAppUrl);

    // One line: version, then the desktop build, then the browser one. It
    // wraps on its own if a narrow screen or a large text scale needs it to.
    return Text.rich(
      TextSpan(
        style: baseStyle,
        children: [
          TextSpan(text: '$v  ·  '),
          TextSpan(
            text: 'Get Wisp for desktop ↗',
            style: linkStyle,
            recognizer: _desktopRecognizer,
          ),
          const TextSpan(text: '  ·  '),
          WidgetSpan(
            alignment: PlaceholderAlignment.middle,
            child: Padding(
              padding: const EdgeInsets.only(right: 3),
              child: Icon(
                Icons.language_rounded,
                size: 12,
                color: context.wc.accentFg,
              ),
            ),
          ),
          TextSpan(
            text: 'Wisp Web ↗',
            style: linkStyle,
            recognizer: _webRecognizer,
          ),
        ],
      ),
    );
  }
}
