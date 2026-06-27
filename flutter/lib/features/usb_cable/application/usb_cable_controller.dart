import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../../platform/android/usb_aoa_channel.dart';

/// Lifecycle phase of the direct phone-to-phone USB (AOA) cable link.
enum UsbCablePhase {
  /// No cable / not supported.
  idle,

  /// A USB peer is present (host side sees a device) but no role chosen yet.
  detected,

  /// Establishing the AOA link (after the user picked a role).
  connecting,

  /// AOA link up, IP tunnel (VPN) not yet started.
  linkUp,

  /// IP tunnel up — iroh transfers run over the cable, either direction.
  tunnelUp,

  /// AOA link attempt failed; [UsbCableState.error] has the reason. (A *VPN*
  /// failure keeps [linkUp] and sets [error] instead, so the right step shows
  /// the failure.)
  error,
}

@immutable
class UsbCableState {
  const UsbCableState({
    required this.supported,
    required this.phase,
    this.role,
    this.detectedRole = 'accessory',
    this.localIp,
    this.error,
    this.tunnelStarting = false,
  });

  final bool supported;
  final UsbCablePhase phase;

  /// "host" | "accessory" — the actual role once linked.
  final String? role;

  /// The role this phone *should* take, detected from the USB bus: a phone that
  /// enumerates a USB device is the USB host ("host"); one that doesn't is the
  /// peripheral being controlled, i.e. the "accessory" (the phone whose USB
  /// notification reads "USB controlled by this device"). Used to pre-select the
  /// right button and show the press order before anything is connected.
  final String detectedRole;
  final String? localIp; // tunnel IP once up
  final String? error;

  /// VPN/tunnel bring-up is in flight (between Start VPN and the tunnel coming
  /// up). Distinct from [UsbCablePhase.connecting], which is the AOA link.
  final bool tunnelStarting;

  bool get tunnelUp => phase == UsbCablePhase.tunnelUp;
  bool get linkUp =>
      phase == UsbCablePhase.linkUp || phase == UsbCablePhase.tunnelUp;

  UsbCableState copyWith({
    UsbCablePhase? phase,
    String? role,
    String? detectedRole,
    String? localIp,
    String? error,
    bool? tunnelStarting,
    bool clearError = false,
  }) => UsbCableState(
    supported: supported,
    phase: phase ?? this.phase,
    role: role ?? this.role,
    detectedRole: detectedRole ?? this.detectedRole,
    localIp: localIp ?? this.localIp,
    error: clearError ? null : (error ?? this.error),
    tunnelStarting: tunnelStarting ?? this.tunnelStarting,
  );

  static const unsupported = UsbCableState(
    supported: false,
    phase: UsbCablePhase.idle,
  );
}

/// Owns the single AOA event subscription and drives the cable through its
/// lifecycle. The flow is fully **manual and in a fixed order**, mirroring the
/// sequence proven to connect reliably on hardware (auto role-detection + auto
/// VPN chaining failed):
///
///   1. On the phone that shows "USB controlled by this device", the user taps
///      **Connect as accessory** — its open retries for a few seconds, arming
///      it to be switched into accessory mode.
///   2. On the other phone, the user taps **Connect as host** — this drives the
///      AOA handshake that switches the first phone into accessory mode, so both
///      links come up.
///   3. On *both* phones the user taps **Start VPN** to bring up the IP tunnel
///      that iroh runs over.
///
/// A 2s poll surfaces cable presence as [detected] (never auto-connects) and is
/// a liveness backstop: the host's bulk read can't see a yanked cable, so a
/// sustained-missing USB device while linked means the cable is gone.
class UsbCableController extends Notifier<UsbCableState> {
  StreamSubscription<UsbAoaEvent>? _sub;
  Timer? _poll;
  bool _busy = false;
  // Set by [stop] to break the connect-retry loop between attempts (a native
  // attempt already in flight can't be interrupted, but no further retry runs).
  bool _cancelRequested = false;
  // Consecutive liveness-poll ticks where the host's USB device was missing.
  // Debounced so a single transient enumeration blip can't kill a live tunnel.
  int _hostMissTicks = 0;
  static const int _hostMissThreshold = 2;

  // Connect-retry tuning. For the accessory this extends the window during
  // which the host can drive the switch (each native attempt itself retries
  // ~3s); for the host it wins the post-switch readiness race.
  static const int _maxConnectAttempts = 4;
  static const Duration _connectRetryDelay = Duration(milliseconds: 500);

  @override
  UsbCableState build() {
    if (!UsbAoa.isSupported) return UsbCableState.unsupported;
    _sub = UsbAoa.events().listen(_onEvent, onError: (_) {});
    _poll = Timer.periodic(const Duration(seconds: 2), (_) => _tick());
    ref.onDispose(() {
      _sub?.cancel();
      _poll?.cancel();
    });
    unawaited(_seed());
    unawaited(_tick()); // detect role immediately rather than after the first 2s
    return const UsbCableState(supported: true, phase: UsbCablePhase.idle);
  }

  // Restore state if the link/tunnel was already up before this controller was
  // first watched.
  Future<void> _seed() async {
    final role = await UsbAoa.state();
    if (role == null) return;
    final ip = await UsbAoa.tunnelLocalIp();
    state = state.copyWith(
      phase: ip != null ? UsbCablePhase.tunnelUp : UsbCablePhase.linkUp,
      role: role,
      localIp: ip,
      clearError: true,
    );
  }

  void _onEvent(UsbAoaEvent event) {
    switch (event) {
      case UsbAoaConnected(:final role):
        // AOA link up. VPN is a separate, explicit step — do NOT auto-start it.
        state = state.copyWith(
          phase: UsbCablePhase.linkUp,
          role: role,
          clearError: true,
        );
      case UsbAoaTunnelUp(:final ip):
        state = state.copyWith(
          phase: UsbCablePhase.tunnelUp,
          localIp: ip,
          tunnelStarting: false,
          clearError: true,
        );
      case UsbAoaTunnelClosed():
        if (state.linkUp) {
          state = state.copyWith(
            phase: UsbCablePhase.linkUp,
            tunnelStarting: false,
          );
        }
      case UsbAoaClosed():
        _reset();
    }
  }

  // Poll: surfaces cable presence as [detected] (never auto-connects) and acts
  // as a liveness backstop while connected.
  Future<void> _tick() async {
    if (_busy) return;
    final hasDevice = await UsbAoa.hasHostDevice();

    // Auto-assign the role from the bus: a phone that enumerates a USB device is
    // the host; one that doesn't is the accessory (the "USB controlled by this
    // device" side). Only meaningful before we're linked.
    if (!state.linkUp && state.phase != UsbCablePhase.connecting) {
      final detected = hasDevice ? 'host' : 'accessory';
      if (state.detectedRole != detected) {
        state = state.copyWith(detectedRole: detected);
      }
    }

    // Liveness: if we're the host and the USB device has vanished for a couple
    // of ticks while linked, the cable is gone — tear down so the status stops
    // lying. Debounced against transient enumeration blips.
    if (state.role == 'host' && state.linkUp) {
      if (hasDevice) {
        _hostMissTicks = 0;
      } else if (++_hostMissTicks >= _hostMissThreshold) {
        _hostMissTicks = 0;
        await _teardown();
      }
      return;
    }
    _hostMissTicks = 0;

    // Status only.
    if (state.phase == UsbCablePhase.idle && hasDevice) {
      state = state.copyWith(phase: UsbCablePhase.detected);
    } else if (state.phase == UsbCablePhase.detected && !hasDevice) {
      state = state.copyWith(phase: UsbCablePhase.idle);
    }
  }

  /// Step 2 (the other phone): drive the AOA handshake that switches the peer
  /// into accessory mode.
  Future<void> connectAsHost() => _connect('host', UsbAoa.connectHost);

  /// Step 1 (the USB-controlling phone): arm as accessory and wait to be
  /// switched. The native open retries internally; the outer loop widens the
  /// window so the host has time to start.
  Future<void> connectAsAccessory() =>
      _connect('accessory', UsbAoa.connectAccessory);

  Future<void> _connect(String role, Future<bool> Function() attempt) async {
    if (!state.supported || _busy) return;
    if (state.linkUp) return; // already linked
    _busy = true;
    _cancelRequested = false;
    state = state.copyWith(
      phase: UsbCablePhase.connecting,
      role: role,
      clearError: true,
    );
    try {
      await _connectWithRetry(attempt);
    } catch (e) {
      state = state.copyWith(phase: UsbCablePhase.error, error: _short(e));
    } finally {
      _busy = false;
    }
  }

  Future<void> _connectWithRetry(Future<bool> Function() attempt) async {
    Object? lastError;
    for (var i = 0; i < _maxConnectAttempts; i++) {
      // A 'connected' event may have advanced us mid-loop; the user may also
      // have pressed Stop between attempts.
      if (_cancelRequested || state.linkUp) return;
      try {
        if (await attempt()) return;
      } catch (e) {
        lastError = e;
      }
      if (_cancelRequested || state.linkUp) return;
      await Future<void>.delayed(_connectRetryDelay);
    }
    if (_cancelRequested) return;
    throw lastError ?? 'Could not establish the USB link';
  }

  /// Step 3 (both phones): bring up the IP tunnel. May show the one-time VPN
  /// consent dialog.
  Future<void> startVpn() async {
    if (!state.linkUp || state.tunnelUp || state.tunnelStarting) return;
    state = state.copyWith(tunnelStarting: true, clearError: true);
    try {
      final ok = await UsbAoa.startTunnel();
      if (!ok && !state.tunnelUp) {
        state = state.copyWith(
          error: 'VPN did not start',
          tunnelStarting: false,
        );
      }
      // Success arrives as a 'tunnelUp' event which clears tunnelStarting.
    } catch (e) {
      state = state.copyWith(error: _short(e), tunnelStarting: false);
    }
  }

  /// Stop an in-progress connect/retry (or a live link) and return to idle.
  Future<void> stop() async {
    _cancelRequested = true;
    await _teardown();
  }

  /// User-initiated disconnect.
  Future<void> disable() => _teardown();

  Future<void> _teardown() async {
    await UsbAoa.stopTunnel();
    await UsbAoa.disconnect();
    _reset();
  }

  void _reset() {
    _hostMissTicks = 0;
    state = const UsbCableState(supported: true, phase: UsbCablePhase.idle);
  }

  String _short(Object e) {
    final s = e.toString();
    return s.length > 120 ? '${s.substring(0, 120)}…' : s;
  }
}

final usbCableControllerProvider =
    NotifierProvider<UsbCableController, UsbCableState>(UsbCableController.new);
