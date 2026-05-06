import 'package:flutter/material.dart';
import 'package:package_info_plus/package_info_plus.dart';

import '../../theme/drift_theme.dart';

class AppVersionText extends StatefulWidget {
  const AppVersionText({super.key});

  @override
  State<AppVersionText> createState() => _AppVersionTextState();
}

class _AppVersionTextState extends State<AppVersionText> {
  String? _version;

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
  Widget build(BuildContext context) {
    final v = _version;
    if (v == null) return const SizedBox.shrink();
    return Text(
      v,
      style: driftSans(
        fontSize: 11,
        fontWeight: FontWeight.w400,
        color: kMuted,
      ),
    );
  }
}
