import 'package:flutter/material.dart';

import '../../../theme/wisp_theme.dart';

enum CheckStatus { running, pass, warn, fail, skipped }

extension CheckStatusX on CheckStatus {
  Color get color {
    switch (this) {
      case CheckStatus.pass:
        return kAccentDirect;
      case CheckStatus.warn:
        return kAccentRelay;
      case CheckStatus.fail:
        return const Color(0xFFCC3333);
      case CheckStatus.running:
        return kMuted;
      case CheckStatus.skipped:
        return kSubtle;
    }
  }

  IconData? get icon {
    switch (this) {
      case CheckStatus.pass:
        return Icons.check_circle_rounded;
      case CheckStatus.warn:
        return Icons.warning_rounded;
      case CheckStatus.fail:
        return Icons.error_rounded;
      case CheckStatus.skipped:
        return Icons.remove_circle_outline_rounded;
      case CheckStatus.running:
        return null;
    }
  }
}

enum CheckGroup { network, rendezvous, lan, p2p, permissions, local }

extension CheckGroupX on CheckGroup {
  String get label {
    switch (this) {
      case CheckGroup.network:
        return 'Network';
      case CheckGroup.rendezvous:
        return 'Rendezvous';
      case CheckGroup.lan:
        return 'LAN';
      case CheckGroup.p2p:
        return 'P2P';
      case CheckGroup.permissions:
        return 'Permissions';
      case CheckGroup.local:
        return 'Local';
    }
  }
}

enum CheckActionKind { openAppSettings, openUrl, retry, createFirewallRule }

class CheckAction {
  final String label;
  final CheckActionKind kind;
  final String? target;

  const CheckAction({required this.label, required this.kind, this.target});
}

class CheckResult {
  final String id;
  final CheckGroup group;
  final CheckStatus status;
  final String label;
  final String detail;
  final String? hint;
  final CheckAction? action;

  const CheckResult({
    required this.id,
    required this.group,
    required this.status,
    required this.label,
    this.detail = '',
    this.hint,
    this.action,
  });

  CheckResult copyWith({
    CheckStatus? status,
    String? detail,
    String? hint,
    CheckAction? action,
  }) {
    return CheckResult(
      id: id,
      group: group,
      status: status ?? this.status,
      label: label,
      detail: detail ?? this.detail,
      hint: hint ?? this.hint,
      action: action ?? this.action,
    );
  }
}
