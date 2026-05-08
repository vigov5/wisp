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
