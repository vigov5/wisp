import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';

/// Point-to-point tunnel addresses (must match UsbAoaChannel.kt). The USB host
/// takes [host], the accessory [accessory].
abstract final class UsbAoaTunnelIps {
  static const String host = '10.42.0.1';
  static const String accessory = '10.42.0.2';
}

/// An event from the native AOA link: either a state change or inbound bytes.
sealed class UsbAoaEvent {
  const UsbAoaEvent();
}

/// The link came up; [role] is "host" or "accessory".
class UsbAoaConnected extends UsbAoaEvent {
  const UsbAoaConnected(this.role);
  final String role;
}

/// The link was torn down (cable unplugged, peer closed, or disconnect()).
class UsbAoaClosed extends UsbAoaEvent {
  const UsbAoaClosed();
}

/// The IP-over-AOA tunnel came up; [ip] is this device's tunnel address.
class UsbAoaTunnelUp extends UsbAoaEvent {
  const UsbAoaTunnelUp(this.ip);
  final String ip;
}

/// The IP-over-AOA tunnel went down.
class UsbAoaTunnelClosed extends UsbAoaEvent {
  const UsbAoaTunnelClosed();
}

/// Android-only bridge to the direct phone-to-phone USB (Android Open
/// Accessory) link. The *sending* phone connects as USB host and drives the
/// *receiving* phone into accessory mode; the IP-over-AOA tunnel (path A) is
/// then built on top so iroh can run over the cable. The byte pump lives in
/// native code; Dart only drives connection setup + status. No-op on every
/// other platform.
class UsbAoa {
  static const _method = MethodChannel('dev.vigov5.wisp/usb_aoa');
  static const _events = EventChannel('dev.vigov5.wisp/usb_aoa/events');

  static bool get isSupported =>
      !kIsWeb && defaultTargetPlatform == TargetPlatform.android;

  // Cached so the stream is shared rather than re-subscribed: each
  // receiveBroadcastStream() call registers with the channel and the native
  // side keeps only the last sink, so a second subscription would silently
  // starve the first.
  static Stream<UsbAoaEvent>? _eventStream;

  /// Connection events from the native link. Emits [UsbAoaConnected],
  /// [UsbAoaTunnelUp], [UsbAoaTunnelClosed], and [UsbAoaClosed].
  static Stream<UsbAoaEvent> events() {
    if (!isSupported) return const Stream.empty();
    return _eventStream ??= _events.receiveBroadcastStream().map(_decodeEvent);
  }

  static UsbAoaEvent _decodeEvent(dynamic raw) {
    if (raw is Map) {
      switch (raw['event']) {
        case 'connected':
          return UsbAoaConnected((raw['role'] as String?) ?? 'unknown');
        case 'closed':
          return const UsbAoaClosed();
        case 'tunnelUp':
          return UsbAoaTunnelUp((raw['ip'] as String?) ?? '');
        case 'tunnelClosed':
          return const UsbAoaTunnelClosed();
      }
    }
    return const UsbAoaClosed();
  }

  /// The current link role ("host"/"accessory") or null when not connected.
  static Future<String?> state() async {
    if (!isSupported) return null;
    try {
      return await _method.invokeMethod<String>('state');
    } on PlatformException {
      return null;
    } on MissingPluginException {
      return null;
    }
  }

  /// Whether a USB device is attached that we could drive into accessory mode
  /// (host side). True when a cable to another phone is plugged in.
  static Future<bool> hasHostDevice() async {
    if (!isSupported) return false;
    try {
      return await _method.invokeMethod<bool>('hasHostDevice') ?? false;
    } on PlatformException {
      return false;
    } on MissingPluginException {
      return false;
    }
  }

  /// Host side: request USB permission if needed, run the AOA handshake to
  /// switch the peer into accessory mode, and claim the bulk endpoints.
  /// Throws [PlatformException] on failure (no device, denied, handshake).
  static Future<bool> connectHost() async {
    if (!isSupported) return false;
    return await _method.invokeMethod<bool>('connectHost') ?? false;
  }

  /// Accessory side: open the accessory delivered by the attach intent.
  static Future<bool> connectAccessory() async {
    if (!isSupported) return false;
    return await _method.invokeMethod<bool>('connectAccessory') ?? false;
  }

  /// True (once) when this launch came from an accessory-attach intent — i.e.
  /// this phone was just plugged into a sending Wisp. Drives the receive flow.
  static Future<bool> consumeAccessoryAttach() async {
    if (!isSupported) return false;
    try {
      return await _method.invokeMethod<bool>('consumeAccessoryAttach') ?? false;
    } on PlatformException {
      return false;
    } on MissingPluginException {
      return false;
    }
  }

  static Future<void> disconnect() async {
    if (!isSupported) return;
    try {
      await _method.invokeMethod<void>('disconnect');
    } on PlatformException {
      // already gone
    } on MissingPluginException {
      // off-Android
    }
  }

  /// Bring up the IP-over-AOA tunnel over the live link. May trigger the
  /// one-time Android VPN consent dialog. After this, the tunnel IPs
  /// (10.42.0.1 host / 10.42.0.2 accessory) are reachable and iroh can run
  /// over the cable. Throws [PlatformException] if there's no link or consent
  /// is denied.
  static Future<bool> startTunnel() async {
    if (!isSupported) return false;
    return await _method.invokeMethod<bool>('startTunnel') ?? false;
  }

  static Future<void> stopTunnel() async {
    if (!isSupported) return;
    try {
      await _method.invokeMethod<void>('stopTunnel');
    } on PlatformException {
      // already gone
    } on MissingPluginException {
      // off-Android
    }
  }

  /// This device's tunnel IP when the tunnel is up, else null.
  static Future<String?> tunnelLocalIp() async {
    if (!isSupported) return null;
    try {
      return await _method.invokeMethod<String>('tunnelLocalIp');
    } on PlatformException {
      return null;
    } on MissingPluginException {
      return null;
    }
  }
}
