import 'dart:convert';

import 'package:shared_preferences/shared_preferences.dart';

import 'saved_device.dart';

const String _key = 'saved_devices.v1';
const int _maxEntries = 30;

/// JSON-backed repository for `SavedDevice` entries. Bounded to [_maxEntries]
/// — when full, the oldest `lastSeenAt` is evicted.
class SavedDevicesRepository {
  SavedDevicesRepository({required this.prefs});

  final SharedPreferences prefs;

  List<SavedDevice> loadAll() {
    final raw = prefs.getString(_key);
    if (raw == null || raw.isEmpty) return const [];
    try {
      final list = jsonDecode(raw) as List<dynamic>;
      return list
          .whereType<Map<String, Object?>>()
          .map(SavedDevice.fromJson)
          .whereType<SavedDevice>()
          .toList(growable: false);
    } catch (_) {
      return const [];
    }
  }

  Future<void> upsert(SavedDevice next) async {
    final all = [...loadAll()];
    final index = all.indexWhere((d) => d.endpointId == next.endpointId);
    if (index >= 0) {
      all[index] = next;
    } else {
      all.add(next);
    }
    // Sort by lastSeenAt desc; evict tail beyond the cap.
    all.sort((a, b) => b.lastSeenAt.compareTo(a.lastSeenAt));
    final trimmed = all.length > _maxEntries
        ? all.sublist(0, _maxEntries)
        : all;
    await _save(trimmed);
  }

  /// Convenience: build/update a record after a successful transfer.
  Future<void> recordTransfer({
    required String endpointId,
    required String label,
    required String deviceType,
    required BigInt bytesTransferred,
    String? lastTicket,
  }) async {
    final all = loadAll();
    final existing = all.firstWhere(
      (d) => d.endpointId == endpointId,
      orElse: () => SavedDevice(
        endpointId: endpointId,
        label: label,
        deviceType: deviceType,
        lastSeenAt: DateTime.now().toUtc(),
        transferCount: 0,
        totalBytes: BigInt.zero,
      ),
    );
    final updated = existing.copyWith(
      label: label.isEmpty ? existing.label : label,
      deviceType: deviceType.isEmpty ? existing.deviceType : deviceType,
      lastSeenAt: DateTime.now().toUtc(),
      transferCount: existing.transferCount + 1,
      totalBytes: existing.totalBytes + bytesTransferred,
      lastTicket: lastTicket ?? existing.lastTicket,
    );
    await upsert(updated);
  }

  Future<void> remove(String endpointId) async {
    final all = loadAll().where((d) => d.endpointId != endpointId).toList();
    await _save(all);
  }

  Future<void> clear() async {
    await prefs.remove(_key);
  }

  Future<void> _save(List<SavedDevice> devices) async {
    final encoded = jsonEncode(
      devices.map((d) => d.toJson()).toList(growable: false),
    );
    await prefs.setString(_key, encoded);
  }
}
