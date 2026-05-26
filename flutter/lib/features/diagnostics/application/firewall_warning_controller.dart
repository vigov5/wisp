import 'dart:async';
import 'dart:io';

import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../../src/rust/api/diagnostics.dart' as rust;

class FirewallWarningState {
  final String? warning;
  final bool dismissed;

  const FirewallWarningState({this.warning, this.dismissed = false});

  bool get isVisible => warning != null && !dismissed;

  FirewallWarningState copyWith({
    Object? warning = _unset,
    bool? dismissed,
  }) {
    return FirewallWarningState(
      warning: warning == _unset ? this.warning : warning as String?,
      dismissed: dismissed ?? this.dismissed,
    );
  }
}

const Object _unset = Object();

final firewallWarningControllerProvider =
    NotifierProvider<FirewallWarningController, FirewallWarningState>(
      FirewallWarningController.new,
    );

class FirewallWarningController extends Notifier<FirewallWarningState> {
  @override
  FirewallWarningState build() {
    if (Platform.isWindows) {
      unawaited(_probe());
    }
    return const FirewallWarningState();
  }

  Future<void> _probe() async {
    final result = await rust.firewallInboundWarning();
    state = state.copyWith(warning: result);
  }

  void dismissForSession() {
    state = state.copyWith(dismissed: true);
  }

  Future<void> recheck() async {
    state = state.copyWith(dismissed: false);
    await _probe();
  }
}
