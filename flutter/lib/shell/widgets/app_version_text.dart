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

class AppVersionText extends StatefulWidget {
  const AppVersionText({super.key});

  @override
  State<AppVersionText> createState() => _AppVersionTextState();
}

class _AppVersionTextState extends State<AppVersionText> {
  String? _version;
  TapGestureRecognizer? _tapRecognizer;

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
    _tapRecognizer?.dispose();
    super.dispose();
  }

  Future<void> _openDesktopDownload() async {
    await launchUrl(
      Uri.parse(_desktopDownloadUrl),
      mode: LaunchMode.externalApplication,
    );
  }

  @override
  Widget build(BuildContext context) {
    final v = _version;
    if (v == null) return const SizedBox.shrink();

    final baseStyle = wispSans(
      fontSize: 11,
      fontWeight: FontWeight.w400,
      color: kMuted,
    );

    // Desktop users already have the desktop app — only mobile needs the link.
    final isMobile = Platform.isAndroid || Platform.isIOS;
    if (!isMobile) {
      return Text(v, style: baseStyle);
    }

    _tapRecognizer ??= TapGestureRecognizer()..onTap = _openDesktopDownload;

    return Text.rich(
      TextSpan(
        style: baseStyle,
        children: [
          TextSpan(text: '$v  ·  '),
          TextSpan(
            text: 'Get Wisp for desktop ↗',
            style: baseStyle.copyWith(
              color: kAccentCyanStrong,
              fontWeight: FontWeight.w600,
            ),
            recognizer: _tapRecognizer,
          ),
        ],
      ),
    );
  }
}
