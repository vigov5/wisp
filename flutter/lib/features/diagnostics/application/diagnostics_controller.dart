import 'package:flutter/foundation.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../settings/application/controller.dart';
import '../domain/check_result.dart';
import '../domain/diagnostics_state.dart';
import 'diagnostics_source.dart';
import 'permission_probe.dart';

final diagnosticsControllerProvider =
    NotifierProvider<DiagnosticsController, DiagnosticsState>(
      DiagnosticsController.new,
    );

bool get _isWindows =>
    !kIsWeb && defaultTargetPlatform == TargetPlatform.windows;

const PermissionProbe _permissionProbe = PermissionProbe();
const DiagnosticsSource _source = DiagnosticsSource();

class DiagnosticsController extends Notifier<DiagnosticsState> {
  @override
  DiagnosticsState build() {
    return const DiagnosticsState();
  }

  Future<void> runAll() async {
    if (state.isRunning) return;
    state = DiagnosticsState(
      results: _initialPendingChecks(),
      isRunning: true,
    );
    final settings = ref.read(settingsControllerProvider).settings;
    await Future.wait([
      _runRust(
        serverUrl: settings.discoveryServerUrl,
        downloadRoot: settings.downloadRoot,
      ),
      _runPermissions(),
    ]);
    state = state.copyWith(isRunning: false, lastRunAt: DateTime.now());
  }

  List<CheckResult> _initialPendingChecks() {
    return <CheckResult>[
      const CheckResult(
        id: 'network.internet',
        group: CheckGroup.network,
        status: CheckStatus.running,
        label: 'Internet reachable',
      ),
      const CheckResult(
        id: 'rendezvous.health',
        group: CheckGroup.rendezvous,
        status: CheckStatus.running,
        label: 'Server /healthz',
      ),
      const CheckResult(
        id: 'lan.self_scan',
        group: CheckGroup.lan,
        status: CheckStatus.running,
        label: 'mDNS self-scan',
      ),
      const CheckResult(
        id: 'p2p.transport',
        group: CheckGroup.p2p,
        status: CheckStatus.running,
        label: 'iroh transport',
      ),
      const CheckResult(
        id: 'p2p.vpn',
        group: CheckGroup.p2p,
        status: CheckStatus.running,
        label: 'VPN interference',
      ),
      ..._permissionProbe.initialPendingChecks(),
      const CheckResult(
        id: 'local.writable',
        group: CheckGroup.local,
        status: CheckStatus.running,
        label: 'Download folder writable',
      ),
      const CheckResult(
        id: 'local.disk_space',
        group: CheckGroup.local,
        status: CheckStatus.running,
        label: 'Disk space',
      ),
      if (_isWindows)
        const CheckResult(
          id: 'local.firewall_win',
          group: CheckGroup.local,
          status: CheckStatus.running,
          label: 'Firewall rule (Wisp.exe)',
        ),
    ];
  }

  Future<void> _runRust({
    String? serverUrl,
    required String downloadRoot,
  }) async {
    final stream = _source.run(
      serverUrl: serverUrl,
      downloadRoot: downloadRoot,
    );
    await for (final result in stream) {
      state = state.upsert(result);
    }
  }

  Future<void> _runPermissions() async {
    final results = await _permissionProbe.runChecks();
    for (final result in results) {
      state = state.upsert(result);
    }
  }

}
