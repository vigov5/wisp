import 'package:flutter/foundation.dart';

/// One peer the user has successfully transferred files with. Persisted to
/// `shared_preferences` so the sender's "Recent" list survives restarts.
@immutable
class SavedDevice {
  const SavedDevice({
    required this.endpointId,
    required this.label,
    required this.deviceType,
    required this.lastSeenAt,
    required this.transferCount,
    required this.totalBytes,
    this.lastTicket,
    this.nickname,
  });

  /// Iroh `EndpointId` (= base32 public key, ~52 chars).  Stable across the
  /// peer's network changes; only changes if they reinstall.
  final String endpointId;

  /// Human-readable name reported by the peer (e.g. "Maya MacBook").
  final String label;

  /// Optional user-authored name pinned to [endpointId]. Distinct from the
  /// peer-controlled [label]: when set it takes display precedence everywhere
  /// the device appears, while [label] is still surfaced as "broadcasts as …".
  /// `null` means the user hasn't renamed this device.
  final String? nickname;

  /// "phone" | "laptop" — used to pick the icon on the tile.
  final String deviceType;

  final DateTime lastSeenAt;
  final int transferCount;
  final BigInt totalBytes;

  /// Optional cached ticket (= EndpointAddr base64) for fast reconnect. May be
  /// stale after the peer changes networks; sender falls back to pkarr lookup
  /// by [endpointId] if a connect attempt with this ticket fails.
  final String? lastTicket;

  SavedDevice copyWith({
    String? endpointId,
    String? label,
    String? deviceType,
    DateTime? lastSeenAt,
    int? transferCount,
    BigInt? totalBytes,
    String? lastTicket,
    String? nickname,
    bool clearNickname = false,
  }) {
    return SavedDevice(
      endpointId: endpointId ?? this.endpointId,
      label: label ?? this.label,
      deviceType: deviceType ?? this.deviceType,
      lastSeenAt: lastSeenAt ?? this.lastSeenAt,
      transferCount: transferCount ?? this.transferCount,
      totalBytes: totalBytes ?? this.totalBytes,
      lastTicket: lastTicket ?? this.lastTicket,
      nickname: clearNickname ? null : (nickname ?? this.nickname),
    );
  }

  Map<String, Object?> toJson() => {
    'endpointId': endpointId,
    'label': label,
    'deviceType': deviceType,
    'lastSeenAt': lastSeenAt.toUtc().toIso8601String(),
    'transferCount': transferCount,
    'totalBytes': totalBytes.toString(),
    'lastTicket': lastTicket,
    'nickname': nickname,
  };

  static SavedDevice? fromJson(Map<String, Object?> json) {
    final endpointId = json['endpointId'] as String?;
    if (endpointId == null || endpointId.isEmpty) return null;
    final lastSeen = DateTime.tryParse(json['lastSeenAt'] as String? ?? '');
    return SavedDevice(
      endpointId: endpointId,
      label: (json['label'] as String?) ?? '',
      deviceType: (json['deviceType'] as String?) ?? 'laptop',
      lastSeenAt: lastSeen ?? DateTime.now().toUtc(),
      transferCount: (json['transferCount'] as int?) ?? 0,
      totalBytes:
          BigInt.tryParse((json['totalBytes'] as String?) ?? '0') ??
          BigInt.zero,
      lastTicket: json['lastTicket'] as String?,
      nickname: json['nickname'] as String?,
    );
  }

  @override
  bool operator ==(Object other) =>
      identical(this, other) ||
      other is SavedDevice &&
          runtimeType == other.runtimeType &&
          endpointId == other.endpointId &&
          label == other.label &&
          deviceType == other.deviceType &&
          lastSeenAt == other.lastSeenAt &&
          transferCount == other.transferCount &&
          totalBytes == other.totalBytes &&
          lastTicket == other.lastTicket &&
          nickname == other.nickname;

  @override
  int get hashCode => Object.hash(
    endpointId,
    label,
    deviceType,
    lastSeenAt,
    transferCount,
    totalBytes,
    lastTicket,
    nickname,
  );
}
