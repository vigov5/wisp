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
  ///
  /// `label` and `deviceType` are upserted on the existing record only when
  /// they carry a *real* value — placeholders ("Sender", "Recipient device",
  /// "Code XYZ ABC", default "laptop" fallback) are dropped so they don't
  /// overwrite a previously-saved real name.  Pass an empty string to
  /// explicitly skip the field.
  Future<void> recordTransfer({
    required String endpointId,
    required String label,
    required String deviceType,
    required BigInt bytesTransferred,
    String? lastTicket,
  }) async {
    final cleanedLabel = _meaningfulLabel(label);
    final cleanedType = _meaningfulDeviceType(deviceType);
    final all = loadAll();
    final existing = all.firstWhere(
      (d) => d.endpointId == endpointId,
      orElse: () => SavedDevice(
        endpointId: endpointId,
        // First save: prefer the cleaned (real) value, fall back to the raw
        // placeholder/default so the tile still has *something* to show.
        // A later transfer with a real value will replace it.
        label: cleanedLabel.isNotEmpty
            ? cleanedLabel
            : (label.trim().isEmpty ? 'Saved device' : label.trim()),
        deviceType: cleanedType.isNotEmpty ? cleanedType : 'laptop',
        lastSeenAt: DateTime.now().toUtc(),
        transferCount: 0,
        totalBytes: BigInt.zero,
      ),
    );
    final updated = existing.copyWith(
      label: cleanedLabel.isEmpty ? existing.label : cleanedLabel,
      deviceType: cleanedType.isEmpty ? existing.deviceType : cleanedType,
      lastSeenAt: DateTime.now().toUtc(),
      transferCount: existing.transferCount + 1,
      totalBytes: existing.totalBytes + bytesTransferred,
      lastTicket: lastTicket ?? existing.lastTicket,
    );
    await upsert(updated);
  }

  /// Strip known placeholder labels emitted by the Rust app layer when a
  /// peer didn't supply a real device name.  Returns empty string for
  /// placeholders so callers (and the upsert logic above) treat it as
  /// "no info, don't overwrite".
  static String _meaningfulLabel(String raw) {
    final trimmed = raw.trim();
    if (trimmed.isEmpty) return '';
    if (trimmed == 'Sender') return '';
    if (trimmed == 'Recipient device') return '';
    if (RegExp(r'^Code [A-Z0-9 ]+$').hasMatch(trimmed)) return '';
    return trimmed;
  }

  /// Mirror of [_meaningfulLabel] for device type — `'laptop'` is the
  /// app-wide default fallback when the peer didn't surface a real type;
  /// treat it as a placeholder on update only (still kept on first save).
  static String _meaningfulDeviceType(String raw) {
    final trimmed = raw.trim().toLowerCase();
    if (trimmed.isEmpty) return '';
    return trimmed;
  }

  /// Set or clear the user-authored nickname for an existing saved device.
  /// A null/blank value clears the nickname (reverts to the broadcast label).
  /// No-op if the device isn't in the list.
  Future<void> rename(String endpointId, String? nickname) async {
    final trimmed = nickname?.trim();
    final next = (trimmed == null || trimmed.isEmpty) ? null : trimmed;
    final all = loadAll();
    final index = all.indexWhere((d) => d.endpointId == endpointId);
    if (index < 0) return;
    await upsert(
      all[index].copyWith(nickname: next, clearNickname: next == null),
    );
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
