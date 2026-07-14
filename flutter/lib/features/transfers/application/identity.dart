import 'package:flutter/foundation.dart';

enum TransferRole { sender, receiver }

enum DeviceType { phone, laptop }

@immutable
class TransferIdentity {
  const TransferIdentity({
    required this.role,
    required this.endpointId,
    required this.deviceName,
    required this.deviceType,
    this.web = false,
    this.ephemeral = false,
  });

  final TransferRole role;
  final String endpointId;
  final String deviceName;
  final DeviceType deviceType;

  /// True when the peer is a browser (no-install web app). The UI shows a globe
  /// glyph instead of the [deviceType] laptop/phone icon.
  final bool web;

  /// True when the peer's identity is ephemeral (browser or CLI — a fresh key
  /// each session), so it must not be persisted to the saved-devices list.
  final bool ephemeral;

  String get displayName {
    final value = deviceName.trim();
    return value.isEmpty ? 'Unknown device' : value;
  }
}
