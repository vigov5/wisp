import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:permission_handler/permission_handler.dart' as ph;

import '../../../src/rust/api/diagnostics.dart' as rust;
import '../../../theme/wisp_theme.dart';
import '../application/diagnostics_controller.dart';
import '../application/firewall_warning_controller.dart';
import '../domain/check_result.dart';
import 'widgets/group_section.dart';
import 'widgets/summary_banner.dart';

const List<CheckGroup> _groupOrder = [
  CheckGroup.network,
  CheckGroup.rendezvous,
  CheckGroup.lan,
  CheckGroup.p2p,
  CheckGroup.permissions,
  CheckGroup.local,
];

class ConnectionTestPage extends ConsumerStatefulWidget {
  const ConnectionTestPage({super.key});

  @override
  ConsumerState<ConnectionTestPage> createState() => _ConnectionTestPageState();
}

class _ConnectionTestPageState extends ConsumerState<ConnectionTestPage> {
  Timer? _timestampTicker;

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addPostFrameCallback((_) {
      unawaited(ref.read(diagnosticsControllerProvider.notifier).runAll());
    });
    _timestampTicker = Timer.periodic(const Duration(seconds: 5), (_) {
      if (mounted) setState(() {});
    });
  }

  @override
  void dispose() {
    _timestampTicker?.cancel();
    super.dispose();
  }

  Future<void> _rerun() async {
    await ref.read(diagnosticsControllerProvider.notifier).runAll();
  }

  @override
  Widget build(BuildContext context) {
    final state = ref.watch(diagnosticsControllerProvider);

    return Scaffold(
      backgroundColor: context.wc.bg,
      appBar: AppBar(
        backgroundColor: context.wc.bg,
        elevation: 0,
        title: Text(
          'Connection Test',
          style: wispSans(
            fontSize: 17,
            fontWeight: FontWeight.w700,
            color: context.wc.ink,
          ),
        ),
        actions: [
          TextButton.icon(
            onPressed: state.isRunning ? null : () => unawaited(_rerun()),
            icon: const Icon(Icons.refresh_rounded, size: 18),
            label: const Text('Re-run'),
          ),
          const SizedBox(width: 8),
        ],
      ),
      body: SafeArea(
        child: RefreshIndicator(
          onRefresh: _rerun,
          child: ListView(
            padding: const EdgeInsets.fromLTRB(16, 8, 16, 24),
            physics: const AlwaysScrollableScrollPhysics(),
            children: [
              SummaryBanner(state: state),
              const SizedBox(height: 16),
              for (final group in _groupOrder)
                if (state.resultsFor(group).isNotEmpty) ...[
                  GroupSection(
                    group: group,
                    groupStatus: state.statusFor(group),
                    results: state.resultsFor(group),
                    onAction: _handleAction,
                  ),
                  const SizedBox(height: 12),
                ],
            ],
          ),
        ),
      ),
    );
  }

  void _handleAction(CheckAction action) {
    switch (action.kind) {
      case CheckActionKind.openAppSettings:
        unawaited(ph.openAppSettings());
        break;
      case CheckActionKind.openUrl:
        ScaffoldMessenger.of(context).showSnackBar(
          SnackBar(content: Text('${action.label} — not wired yet')),
        );
        break;
      case CheckActionKind.retry:
        unawaited(_rerun());
        break;
      case CheckActionKind.createFirewallRule:
        unawaited(_createFirewallRule());
        break;
    }
  }

  Future<void> _createFirewallRule() async {
    final messenger = ScaffoldMessenger.of(context);
    messenger.showSnackBar(
      const SnackBar(
        content: Text('Requesting admin permission to add firewall rule…'),
        duration: Duration(seconds: 4),
      ),
    );
    try {
      await rust.createFirewallRule();
      if (!mounted) return;
      messenger
        ..hideCurrentSnackBar()
        ..showSnackBar(const SnackBar(content: Text('Firewall rule created.')));
      unawaited(ref.read(firewallWarningControllerProvider.notifier).recheck());
      await _rerun();
    } catch (error) {
      if (!mounted) return;
      messenger
        ..hideCurrentSnackBar()
        ..showSnackBar(
          SnackBar(content: Text('Couldn\'t create firewall rule: $error')),
        );
    }
  }
}
