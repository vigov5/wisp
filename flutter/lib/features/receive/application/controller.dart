import 'package:riverpod_annotation/riverpod_annotation.dart';

import '../../../src/rust/api/simple.dart' as rust_simple;
import '../../settings/feature.dart';
import 'service.dart';
import 'state.dart';

part 'controller.g.dart';

@riverpod
ReceiverIdleViewState receiverIdleViewState(Ref ref) {
  final service = ref.watch(receiverServiceProvider);
  final snapshot = service.snapshot;
  final pairingCode = service.pairingCode;

  final badge = switch (snapshot.lifecycle) {
    ReceiverLifecycle.starting => const ReceiverBadgeState.registering(),
    ReceiverLifecycle.ready =>
      pairingCode.isAvailable
          ? const ReceiverBadgeState.ready()
          : const ReceiverBadgeState.unavailable(),
    ReceiverLifecycle.stopped => const ReceiverBadgeState.unavailable(),
    ReceiverLifecycle.failed => const ReceiverBadgeState.unavailable(),
  };

  final code = pairingCode.isAvailable ? pairingCode.formattedCode : '......';
  final deviceName = ref.watch(settingsControllerProvider).settings.deviceName;

  // This device's own public key, for the copyable badge below the name.
  // Empty (badge hides) if the bridge isn't initialized yet.
  String endpointId = '';
  try {
    endpointId = rust_simple.currentEndpointId();
  } catch (_) {}

  // Parse the expiry timestamp once so the widget can compute remaining TTL
  // without re-parsing on every animation tick.  Bad/missing values fall
  // through as `null` — the idle card just hides the countdown bar.
  DateTime? expiresAt;
  final raw = pairingCode.expiresAt;
  if (raw != null && raw.isNotEmpty) {
    expiresAt = DateTime.tryParse(raw)?.toUtc();
  }

  return ReceiverIdleViewState(
    deviceName: deviceName,
    badge: badge,
    status: badge.label,
    code: code,
    clipboardCode: pairingCode.clipboardCode,
    lifecycle: snapshot.lifecycle,
    endpointId: endpointId,
    expiresAt: expiresAt,
    isStale: pairingCode.isStale,
  );
}
