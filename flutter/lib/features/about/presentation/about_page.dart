import 'package:flutter/material.dart';
import 'package:package_info_plus/package_info_plus.dart';
import 'package:url_launcher/url_launcher.dart';

import '../../../theme/wisp_theme.dart';

const _repoUrl = 'https://github.com/vigov5/wisp';
const _issuesUrl = 'https://github.com/vigov5/wisp/issues/new';
const _releasesUrl = 'https://github.com/vigov5/wisp/releases';

class AboutPage extends StatefulWidget {
  const AboutPage({super.key});

  @override
  State<AboutPage> createState() => _AboutPageState();
}

class _AboutPageState extends State<AboutPage> {
  String _version = '';

  @override
  void initState() {
    super.initState();
    PackageInfo.fromPlatform().then((info) {
      if (mounted) {
        setState(() => _version = 'v${info.version} (${info.buildNumber})');
      }
    });
  }

  Future<void> _open(String url) async {
    await launchUrl(Uri.parse(url), mode: LaunchMode.externalApplication);
  }

  void _showLicenses() {
    showLicensePage(
      context: context,
      applicationName: 'Wisp',
      applicationVersion: _version,
      applicationIcon: Padding(
        padding: const EdgeInsets.all(8),
        child: Image.asset('assets/wisp_rounded_logo.png', width: 48),
      ),
    );
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      backgroundColor: context.wc.bg,
      appBar: AppBar(
        backgroundColor: context.wc.bg,
        elevation: 0,
        title: Text(
          'About',
          style: wispSans(
            fontSize: 17,
            fontWeight: FontWeight.w700,
            color: context.wc.ink,
          ),
        ),
      ),
      body: ListView(
        padding: const EdgeInsets.fromLTRB(24, 16, 24, 32),
        children: [
          const SizedBox(height: 12),
          Center(child: Image.asset('assets/wisp_rounded_logo.png', width: 88)),
          const SizedBox(height: 16),
          Center(
            child: Text(
              'Wisp',
              style: wispSans(
                fontSize: 24,
                fontWeight: FontWeight.w700,
                color: context.wc.ink,
                letterSpacing: -0.4,
              ),
            ),
          ),
          const SizedBox(height: 4),
          Center(
            child: Text(
              _version,
              style: wispSans(
                fontSize: 13,
                fontWeight: FontWeight.w400,
                color: context.wc.muted,
              ),
            ),
          ),
          const SizedBox(height: 16),
          Text(
            'AirDrop-like file sharing for any device, anywhere. Free, '
            'open source, and end-to-end encrypted.',
            textAlign: TextAlign.center,
            style: wispSans(
              fontSize: 13.5,
              fontWeight: FontWeight.w400,
              color: context.wc.muted,
              height: 1.5,
            ),
          ),
          const SizedBox(height: 28),
          _AboutTile(
            icon: Icons.code_rounded,
            title: 'View source on GitHub',
            subtitle: 'github.com/vigov5/wisp',
            onTap: () => _open(_repoUrl),
          ),
          const SizedBox(height: 10),
          _AboutTile(
            icon: Icons.desktop_windows_rounded,
            title: 'Get Wisp for desktop',
            subtitle: 'Windows, macOS & Linux builds',
            onTap: () => _open(_releasesUrl),
          ),
          const SizedBox(height: 10),
          _AboutTile(
            icon: Icons.bug_report_rounded,
            title: 'Report an issue',
            subtitle: 'Something broken or confusing?',
            onTap: () => _open(_issuesUrl),
          ),
          const SizedBox(height: 10),
          _AboutTile(
            icon: Icons.gavel_rounded,
            title: 'Open source licenses',
            subtitle: 'Licenses of the packages Wisp uses',
            onTap: _showLicenses,
          ),
        ],
      ),
    );
  }
}

class _AboutTile extends StatelessWidget {
  const _AboutTile({
    required this.icon,
    required this.title,
    required this.subtitle,
    required this.onTap,
  });

  final IconData icon;
  final String title;
  final String subtitle;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    return InkWell(
      onTap: onTap,
      borderRadius: BorderRadius.circular(12),
      child: Container(
        padding: const EdgeInsets.symmetric(horizontal: 14, vertical: 12),
        decoration: BoxDecoration(
          color: context.wc.surface,
          borderRadius: BorderRadius.circular(12),
          border: Border.all(color: context.wc.border),
        ),
        child: Row(
          children: [
            Icon(icon, size: 22, color: context.wc.ink.withValues(alpha: 0.8)),
            const SizedBox(width: 14),
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(
                    title,
                    style: wispSans(
                      fontSize: 14,
                      fontWeight: FontWeight.w600,
                      color: context.wc.ink,
                    ),
                  ),
                  const SizedBox(height: 2),
                  Text(
                    subtitle,
                    style: wispSans(
                      fontSize: 11.5,
                      fontWeight: FontWeight.w400,
                      color: context.wc.muted,
                    ),
                  ),
                ],
              ),
            ),
            Icon(Icons.chevron_right_rounded, color: context.wc.muted),
          ],
        ),
      ),
    );
  }
}
