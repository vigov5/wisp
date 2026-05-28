import 'package:flutter/material.dart';

enum ReceiverLifecycle { starting, ready, stopped, failed }

enum ReceiverBadgePhase { unavailable, registering, ready }

@immutable
class ReceiverSnapshot {
  const ReceiverSnapshot({
    required this.lifecycle,
    required this.discoverableRequested,
    required this.advertisingActive,
    required this.hasRegistration,
    required this.hasPendingOffer,
  });

  final ReceiverLifecycle lifecycle;
  final bool discoverableRequested;
  final bool advertisingActive;
  final bool hasRegistration;
  final bool hasPendingOffer;
}

@immutable
class PairingCodeState {
  const PairingCodeState.unavailable()
    : code = null,
      expiresAt = null,
      isStale = false;

  const PairingCodeState.active({required this.code, this.expiresAt})
    : isStale = false;

  /// Server reports the code is no longer claimable (probably the previous
  /// sender already pulled the ticket) but background re-registration has
  /// not yet succeeded.  Keep showing the existing code so the user has
  /// context, but prompt them to tap Refresh.
  const PairingCodeState.stale({required this.code, this.expiresAt})
    : isStale = true;

  final String? code;
  final String? expiresAt;
  final bool isStale;

  bool get isAvailable => code != null && code!.trim().isNotEmpty;

  String get normalizedCode {
    return (code ?? '').replaceAll(' ', '').trim().toUpperCase();
  }

  String get clipboardCode => normalizedCode;

  String get formattedCode {
    final value = normalizedCode;
    if (value.length != 6) {
      return value;
    }
    return '${value.substring(0, 3)} ${value.substring(3)}';
  }
}

@immutable
class NearbyReceiver {
  const NearbyReceiver({
    required this.fullname,
    required this.label,
    required this.deviceType,
    required this.code,
    required this.ticket,
    this.endpointId = '',
  });

  final String fullname;
  final String label;
  final String deviceType;
  final String code;
  final String ticket;
  final String endpointId;
}

@immutable
class ReceiverBadgeState {
  const ReceiverBadgeState._({
    required this.phase,
    required this.label,
    required this.color,
  });

  const ReceiverBadgeState.unavailable()
    : this._(
        phase: ReceiverBadgePhase.unavailable,
        label: 'Unavailable',
        color: const Color(0xFF8A8A8A),
      );

  const ReceiverBadgeState.registering()
    : this._(
        phase: ReceiverBadgePhase.registering,
        label: 'Registering',
        color: const Color(0xFFD4A824),
      );

  const ReceiverBadgeState.ready()
    : this._(
        phase: ReceiverBadgePhase.ready,
        label: 'Ready',
        color: const Color(0xFF49B36C),
      );

  final ReceiverBadgePhase phase;
  final String label;
  final Color color;
}

@immutable
class ReceiverIdleViewState {
  const ReceiverIdleViewState({
    required this.deviceName,
    required this.badge,
    required this.status,
    required this.code,
    required this.clipboardCode,
    required this.lifecycle,
    this.endpointId = '',
    this.expiresAt,
    this.isStale = false,
  });

  final String deviceName;

  /// This device's own iroh EndpointId / public key.  Surfaced below the
  /// device name on the idle screen as a copyable badge.  Empty when the
  /// FFI bridge hasn't initialized yet — the badge hides itself.
  final String endpointId;
  final ReceiverBadgeState badge;
  final String status;
  final String code;
  final String clipboardCode;
  final ReceiverLifecycle lifecycle;

  /// RFC3339 timestamp parsed from the rendezvous-server registration.  Used
  /// by the idle card to render a TTL countdown indicator below the code.
  /// `null` when the receiver is unregistered (no countdown to show).
  final DateTime? expiresAt;

  /// `true` when the rendezvous server reports the code is no longer
  /// claimable but background re-registration hasn't succeeded yet.  UI
  /// surfaces a "Code may have been used. Tap to refresh" hint and styles
  /// the code area as muted.
  final bool isStale;
}

/// Categorises a receiver-service failure so the UI can offer a sensible
/// remediation button. Defaults to [openConnectionTest] which routes to the
/// self-diagnose page — from there the user can decide whether the problem
/// is config, permission, or network.
enum ReceiverErrorAction { openConnectionTest, openSettings, none }

@immutable
class ReceiverServiceError {
  const ReceiverServiceError({
    required this.message,
    this.action = ReceiverErrorAction.openConnectionTest,
  });

  final String message;
  final ReceiverErrorAction action;

  @override
  bool operator ==(Object other) =>
      identical(this, other) ||
      other is ReceiverServiceError &&
          runtimeType == other.runtimeType &&
          message == other.message &&
          action == other.action;

  @override
  int get hashCode => Object.hash(message, action);
}

@immutable
class ReceiverServiceState {
  const ReceiverServiceState({
    required this.snapshot,
    required this.pairingCode,
    this.error,
  });

  factory ReceiverServiceState.ready({
    required String code,
    String? expiresAt,
  }) {
    return ReceiverServiceState(
      snapshot: const ReceiverSnapshot(
        lifecycle: ReceiverLifecycle.ready,
        discoverableRequested: false,
        advertisingActive: false,
        hasRegistration: true,
        hasPendingOffer: false,
      ),
      pairingCode: PairingCodeState.active(code: code, expiresAt: expiresAt),
    );
  }

  /// Same as [ready] but flags the pairing code as stale — UI uses this to
  /// show a "may have been used, tap Refresh" hint.  The code itself stays
  /// visible so the user has context for the warning.
  factory ReceiverServiceState.stale({
    required String code,
    String? expiresAt,
  }) {
    return ReceiverServiceState(
      snapshot: const ReceiverSnapshot(
        lifecycle: ReceiverLifecycle.ready,
        discoverableRequested: false,
        advertisingActive: false,
        hasRegistration: true,
        hasPendingOffer: false,
      ),
      pairingCode: PairingCodeState.stale(code: code, expiresAt: expiresAt),
    );
  }

  const ReceiverServiceState.unavailable()
    : snapshot = const ReceiverSnapshot(
        lifecycle: ReceiverLifecycle.stopped,
        discoverableRequested: false,
        advertisingActive: false,
        hasRegistration: false,
        hasPendingOffer: false,
      ),
      pairingCode = const PairingCodeState.unavailable(),
      error = null;

  const ReceiverServiceState.registering()
    : snapshot = const ReceiverSnapshot(
        lifecycle: ReceiverLifecycle.starting,
        discoverableRequested: false,
        advertisingActive: false,
        hasRegistration: false,
        hasPendingOffer: false,
      ),
      pairingCode = const PairingCodeState.unavailable(),
      error = null;

  const ReceiverServiceState.stopped()
    : snapshot = const ReceiverSnapshot(
        lifecycle: ReceiverLifecycle.stopped,
        discoverableRequested: false,
        advertisingActive: false,
        hasRegistration: false,
        hasPendingOffer: false,
      ),
      pairingCode = const PairingCodeState.unavailable(),
      error = null;

  const ReceiverServiceState.failed()
    : snapshot = const ReceiverSnapshot(
        lifecycle: ReceiverLifecycle.failed,
        discoverableRequested: false,
        advertisingActive: false,
        hasRegistration: false,
        hasPendingOffer: false,
      ),
      pairingCode = const PairingCodeState.unavailable(),
      error = null;

  /// Failed state carrying a user-visible error message + a suggested action.
  /// The receiver-service source emits this when an underlying Rust stream
  /// reports an unrecoverable error (e.g. download root not writable, FFI
  /// channel disconnect) so the shell can render a banner with a remediation
  /// button instead of leaving the user staring at an unresponsive page.
  factory ReceiverServiceState.failedWith(ReceiverServiceError error) {
    return ReceiverServiceState(
      snapshot: const ReceiverSnapshot(
        lifecycle: ReceiverLifecycle.failed,
        discoverableRequested: false,
        advertisingActive: false,
        hasRegistration: false,
        hasPendingOffer: false,
      ),
      pairingCode: const PairingCodeState.unavailable(),
      error: error,
    );
  }

  final ReceiverSnapshot snapshot;
  final PairingCodeState pairingCode;
  final ReceiverServiceError? error;

  ReceiverServiceState copyWith({
    ReceiverSnapshot? snapshot,
    PairingCodeState? pairingCode,
    ReceiverServiceError? error,
    bool clearError = false,
  }) {
    return ReceiverServiceState(
      snapshot: snapshot ?? this.snapshot,
      pairingCode: pairingCode ?? this.pairingCode,
      error: clearError ? null : (error ?? this.error),
    );
  }
}
