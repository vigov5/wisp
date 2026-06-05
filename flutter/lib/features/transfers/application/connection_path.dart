import 'package:flutter/foundation.dart';

import '../../../src/rust/api/receiver.dart' as rust_receiver;
import '../../../src/rust/api/sender.dart' as rust_sender;

enum ConnectionPathKind { direct, relay, unknown }

@immutable
class ConnectionPathInfo {
  const ConnectionPathInfo({
    required this.kind,
    this.relayUrl,
    this.directAddr,
  });

  final ConnectionPathKind kind;
  final String? relayUrl;

  /// Active direct UDP socket address ("ip:port") when [kind] is `direct`.
  final String? directAddr;

  bool get isDirect => kind == ConnectionPathKind.direct;
  bool get isRelay => kind == ConnectionPathKind.relay;

  String? get relayHost {
    if (relayUrl == null) {
      return null;
    }
    final parsed = Uri.tryParse(relayUrl!);
    if (parsed == null) {
      return null;
    }
    final host = parsed.host;
    return host.isEmpty ? null : host;
  }

  /// Strips port off [directAddr] (handles IPv4 "1.2.3.4:567" and IPv6
  /// "[::1]:567"). Returns `null` if [directAddr] is null or unparseable.
  String? get directIpHost {
    final addr = directAddr;
    if (addr == null || addr.isEmpty) {
      return null;
    }
    if (addr.startsWith('[')) {
      final end = addr.indexOf(']');
      if (end > 1) {
        return addr.substring(1, end);
      }
      return null;
    }
    final colon = addr.lastIndexOf(':');
    if (colon <= 0) {
      return addr;
    }
    return addr.substring(0, colon);
  }

  static ConnectionPathInfo? fromReceiver(
    rust_receiver.ReceiverConnectionPath? path,
  ) {
    if (path == null) {
      return null;
    }
    return ConnectionPathInfo(
      kind: _parseKind(path.kind),
      relayUrl: path.relayUrl,
      directAddr: path.directAddr,
    );
  }

  static ConnectionPathInfo? fromSender(rust_sender.SendConnectionPath? path) {
    if (path == null) {
      return null;
    }
    return ConnectionPathInfo(
      kind: _parseKind(path.kind),
      relayUrl: path.relayUrl,
      directAddr: path.directAddr,
    );
  }

  // Hidden contract with Rust: the string values "p2p" / "relay" / "unknown"
  // come from `ConnectionPath::label()` in `crates/core/src/util/mod.rs`. If
  // those labels change, update this switch — there is no compile-time check.
  static ConnectionPathKind _parseKind(String raw) {
    switch (raw) {
      case 'p2p':
        return ConnectionPathKind.direct;
      case 'relay':
        return ConnectionPathKind.relay;
      default:
        return ConnectionPathKind.unknown;
    }
  }

  @override
  bool operator ==(Object other) =>
      identical(this, other) ||
      other is ConnectionPathInfo &&
          runtimeType == other.runtimeType &&
          kind == other.kind &&
          relayUrl == other.relayUrl &&
          directAddr == other.directAddr;

  @override
  int get hashCode => Object.hash(kind, relayUrl, directAddr);
}
