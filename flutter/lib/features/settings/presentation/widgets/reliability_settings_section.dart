import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';

import '../../../../platform/android/transfer_keepalive_channel.dart';
import '../../../../theme/wisp_theme.dart';
import 'settings_section_field.dart';

/// Android-only settings section that surfaces battery-optimisation status
/// and lets the user open the system prompt to disable it. Hidden on every
/// other platform.
class ReliabilitySettingsSection extends StatefulWidget {
  const ReliabilitySettingsSection({super.key});

  @override
  State<ReliabilitySettingsSection> createState() =>
      _ReliabilitySettingsSectionState();
}

class _ReliabilitySettingsSectionState extends State<ReliabilitySettingsSection>
    with WidgetsBindingObserver {
  bool _ignoring = false;
  bool _loading = true;

  bool get _isAndroid =>
      !kIsWeb && defaultTargetPlatform == TargetPlatform.android;

  @override
  void initState() {
    super.initState();
    if (_isAndroid) {
      WidgetsBinding.instance.addObserver(this);
      _refreshStatus();
    }
  }

  @override
  void dispose() {
    if (_isAndroid) {
      WidgetsBinding.instance.removeObserver(this);
    }
    super.dispose();
  }

  @override
  void didChangeAppLifecycleState(AppLifecycleState state) {
    if (state == AppLifecycleState.resumed) {
      _refreshStatus();
    }
  }

  Future<void> _refreshStatus() async {
    final value = await TransferKeepalive.isIgnoringBatteryOptimizations();
    if (!mounted) return;
    setState(() {
      _ignoring = value;
      _loading = false;
    });
  }

  Future<void> _onDisableTap() async {
    await TransferKeepalive.requestIgnoreBatteryOptimizations();
    // Result observed via didChangeAppLifecycleState when user returns.
  }

  @override
  Widget build(BuildContext context) {
    if (!_isAndroid) {
      return const SizedBox.shrink();
    }
    return SettingsSectionField(
      label: 'Reliability',
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          _StatusRow(loading: _loading, ignoring: _ignoring),
          const SizedBox(height: 12),
          FilledButton(
            onPressed: _ignoring || _loading ? null : _onDisableTap,
            style: FilledButton.styleFrom(
              backgroundColor: kAccentCyanStrong,
              foregroundColor: Colors.white,
              minimumSize: const Size(0, 44),
              shape: RoundedRectangleBorder(
                borderRadius: BorderRadius.circular(12),
              ),
            ),
            child: const Text('Disable battery optimisation'),
          ),
          const SizedBox(height: 10),
          Text(
            'On some devices (Xiaomi, Huawei, Samsung), additional steps are '
            'required to keep transfers running while the screen is locked. '
            'See dontkillmyapp.com for vendor-specific instructions.',
            style: wispSans(
              fontSize: 11.5,
              fontWeight: FontWeight.w400,
              color: kMuted,
              height: 1.45,
            ),
          ),
        ],
      ),
    );
  }
}

class _StatusRow extends StatelessWidget {
  const _StatusRow({required this.loading, required this.ignoring});

  final bool loading;
  final bool ignoring;

  @override
  Widget build(BuildContext context) {
    if (loading) {
      return Row(
        children: [
          const SizedBox(
            width: 14,
            height: 14,
            child: CircularProgressIndicator(strokeWidth: 2),
          ),
          const SizedBox(width: 10),
          Text(
            'Checking battery optimisation status…',
            style: wispSans(fontSize: 12.5, color: kMuted),
          ),
        ],
      );
    }
    final color = ignoring ? const Color(0xFF49B36C) : const Color(0xFFC78F2A);
    final icon = ignoring ? Icons.check_circle_rounded : Icons.warning_rounded;
    final message = ignoring
        ? 'Battery optimisation disabled — transfers should survive long sessions.'
        : 'Battery optimisation enabled — long transfers may be killed by the system.';
    return Row(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Icon(icon, size: 18, color: color),
        const SizedBox(width: 10),
        Expanded(
          child: Text(
            message,
            style: wispSans(
              fontSize: 12.5,
              fontWeight: FontWeight.w500,
              color: kInk,
              height: 1.4,
            ),
          ),
        ),
      ],
    );
  }
}
