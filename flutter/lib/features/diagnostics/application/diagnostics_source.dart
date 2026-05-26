import '../../../src/rust/api/diagnostics.dart' as rust;
import '../domain/check_result.dart';

class DiagnosticsSource {
  const DiagnosticsSource();

  Stream<CheckResult> run({String? serverUrl, required String downloadRoot}) {
    return rust
        .runConnectionTest(serverUrl: serverUrl, downloadRoot: downloadRoot)
        .map(_mapCheck);
  }
}

CheckResult _mapCheck(rust.DiagnosticsCheckData data) {
  return CheckResult(
    id: data.id,
    group: _mapGroup(data.group),
    status: _mapStatus(data.status),
    label: data.label,
    detail: data.detail,
    hint: data.hint,
    action: data.action != null ? _mapAction(data.action!) : null,
  );
}

CheckStatus _mapStatus(rust.DiagnosticsCheckStatus status) {
  switch (status) {
    case rust.DiagnosticsCheckStatus.running:
      return CheckStatus.running;
    case rust.DiagnosticsCheckStatus.pass:
      return CheckStatus.pass;
    case rust.DiagnosticsCheckStatus.warn:
      return CheckStatus.warn;
    case rust.DiagnosticsCheckStatus.fail:
      return CheckStatus.fail;
    case rust.DiagnosticsCheckStatus.skipped:
      return CheckStatus.skipped;
  }
}

CheckGroup _mapGroup(rust.DiagnosticsCheckGroup group) {
  switch (group) {
    case rust.DiagnosticsCheckGroup.network:
      return CheckGroup.network;
    case rust.DiagnosticsCheckGroup.rendezvous:
      return CheckGroup.rendezvous;
    case rust.DiagnosticsCheckGroup.lan:
      return CheckGroup.lan;
    case rust.DiagnosticsCheckGroup.p2P:
      return CheckGroup.p2p;
    case rust.DiagnosticsCheckGroup.permissions:
      return CheckGroup.permissions;
    case rust.DiagnosticsCheckGroup.local:
      return CheckGroup.local;
  }
}

CheckAction _mapAction(rust.DiagnosticsActionData action) {
  return CheckAction(
    label: action.label,
    kind: _mapActionKind(action.kind),
    target: action.target,
  );
}

CheckActionKind _mapActionKind(rust.DiagnosticsActionKind kind) {
  switch (kind) {
    case rust.DiagnosticsActionKind.openAppSettings:
      return CheckActionKind.openAppSettings;
    case rust.DiagnosticsActionKind.openUrl:
      return CheckActionKind.openUrl;
    case rust.DiagnosticsActionKind.retry:
      return CheckActionKind.retry;
    case rust.DiagnosticsActionKind.createFirewallRule:
      return CheckActionKind.createFirewallRule;
  }
}
