import 'package:flutter_riverpod/flutter_riverpod.dart';

import 'saved_device.dart';
import 'saved_devices_repository.dart';

/// Override this in `ProviderScope` overrides during bootstrap once
/// `SharedPreferences` is available.
final savedDevicesRepositoryProvider = Provider<SavedDevicesRepository>(
  (ref) => throw UnimplementedError(
    'savedDevicesRepositoryProvider must be overridden during bootstrap',
  ),
);

final savedDevicesProvider =
    NotifierProvider<SavedDevicesController, List<SavedDevice>>(
      SavedDevicesController.new,
    );

/// Cheap `endpointId -> nickname` lookup for any screen that needs to resolve
/// a device's display name by pubkey. Only includes devices with a non-empty
/// user-authored nickname.
final savedNicknamesProvider = Provider<Map<String, String>>((ref) {
  final devices = ref.watch(savedDevicesProvider);
  return {
    for (final d in devices)
      if ((d.nickname ?? '').isNotEmpty) d.endpointId: d.nickname!,
  };
});

/// The set of endpointIds the user has marked auto-accept (trusted). Cheap
/// membership test for the receive flow, which checks it the instant an offer
/// (or even the pre-offer connect) arrives to decide whether to skip the
/// Accept/Decline prompt. Whether the auto-accept actually fires is additionally
/// gated by the app-wide `autoAcceptTrustedDevices` master switch.
final trustedEndpointIdsProvider = Provider<Set<String>>((ref) {
  final devices = ref.watch(savedDevicesProvider);
  return {
    for (final d in devices)
      if (d.autoAccept) d.endpointId,
  };
});

class SavedDevicesController extends Notifier<List<SavedDevice>> {
  @override
  List<SavedDevice> build() {
    final repo = ref.watch(savedDevicesRepositoryProvider);
    return repo.loadAll();
  }

  Future<void> recordTransfer({
    required String endpointId,
    required String label,
    required String deviceType,
    required BigInt bytesTransferred,
    String? lastTicket,
  }) async {
    final repo = ref.read(savedDevicesRepositoryProvider);
    await repo.recordTransfer(
      endpointId: endpointId,
      label: label,
      deviceType: deviceType,
      bytesTransferred: bytesTransferred,
      lastTicket: lastTicket,
    );
    state = repo.loadAll();
  }

  /// Set or clear the user-authored nickname for a saved device.
  Future<void> rename(String endpointId, String? nickname) async {
    final repo = ref.read(savedDevicesRepositoryProvider);
    await repo.rename(endpointId, nickname);
    state = repo.loadAll();
  }

  /// Turn auto-accept (trust) on or off for a saved device.
  Future<void> setAutoAccept(String endpointId, bool value) async {
    final repo = ref.read(savedDevicesRepositoryProvider);
    await repo.setAutoAccept(endpointId, value);
    state = repo.loadAll();
  }

  /// Trust a device straight from the incoming-offer card, creating the record
  /// if this is the first transfer with it.
  Future<void> trustFromOffer({
    required String endpointId,
    required String label,
    required String deviceType,
  }) async {
    final repo = ref.read(savedDevicesRepositoryProvider);
    await repo.trustFromOffer(
      endpointId: endpointId,
      label: label,
      deviceType: deviceType,
    );
    state = repo.loadAll();
  }

  Future<void> remove(String endpointId) async {
    final repo = ref.read(savedDevicesRepositoryProvider);
    await repo.remove(endpointId);
    state = repo.loadAll();
  }

  Future<void> clear() async {
    final repo = ref.read(savedDevicesRepositoryProvider);
    await repo.clear();
    state = const [];
  }
}
