import 'package:flutter_riverpod/flutter_riverpod.dart';

import 'saved_devices_controller.dart';

/// The resolved name(s) to show for a device, after applying any user-authored
/// nickname pinned to its pubkey.
class DeviceDisplayName {
  const DeviceDisplayName({required this.primary, this.broadcast});

  /// What to show as the main label: the nickname if the user set one,
  /// otherwise the peer-reported broadcast name.
  final String primary;

  /// The peer-reported broadcast name, set only when a nickname is overriding
  /// it — so the UI can render a secondary "broadcasts as {broadcast}" line.
  /// `null` when no nickname is set (the broadcast name is already [primary]).
  final String? broadcast;

  bool get hasNickname => broadcast != null;
}

/// Resolve the display name for a device by its [endpointId] (pubkey), applying
/// any saved nickname. [broadcastLabel] is the peer-reported name; callers
/// should pass their own empty-string fallback already applied (e.g.
/// "Unknown device" / "Saved device") since this helper does not invent one.
///
/// When [endpointId] is empty (some incoming offers don't carry one), the
/// broadcast name is returned unchanged.
DeviceDisplayName resolveDeviceName(
  WidgetRef ref, {
  required String endpointId,
  required String broadcastLabel,
}) {
  if (endpointId.isEmpty) {
    return DeviceDisplayName(primary: broadcastLabel);
  }
  final nickname = ref.watch(savedNicknamesProvider)[endpointId];
  if (nickname == null || nickname.isEmpty) {
    return DeviceDisplayName(primary: broadcastLabel);
  }
  return DeviceDisplayName(primary: nickname, broadcast: broadcastLabel);
}
