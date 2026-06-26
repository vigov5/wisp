import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:typed_data';

import 'package:flutter/material.dart';

import '../../platform/android/usb_aoa_channel.dart';

/// Phase-1 hardware spike for direct phone-to-phone USB (Android Open
/// Accessory) transfer. Not a shipping screen — it exists to prove, on two
/// physical phones + a USB-C cable, that the AOA link establishes and bytes
/// round-trip. Once that gate passes, the IP-over-AOA tunnel (path A) is built
/// on the same [UsbAoa] link and this page can be removed.
///
/// How to run it:
///  1. Open this page on BOTH phones (Settings → "USB cable spike (dev)").
///  2. Connect them with a USB-C cable.
///  3. On the phone you want as **sender**, tap "Connect as host". Approve the
///     USB permission dialog. The other phone should get an accessory-attach
///     prompt (or auto-connect if this page is already open).
///  4. The host auto-sends a PING; the accessory echoes it back. Watch the log
///     on both ends — a matching PING/echo confirms the round-trip.
class UsbAoaSpikePage extends StatefulWidget {
  const UsbAoaSpikePage({super.key});

  @override
  State<UsbAoaSpikePage> createState() => _UsbAoaSpikePageState();
}

class _UsbAoaSpikePageState extends State<UsbAoaSpikePage> {
  final List<String> _log = [];
  StreamSubscription<UsbAoaEvent>? _sub;
  String? _role;
  int _pingCounter = 0;

  // IP-over-AOA tunnel validation: a UDP echo over the tunnel IPs proves L3
  // connectivity over the cable — the gate before wiring iroh onto path A.
  static const int _udpPort = 47999;
  String? _tunnelIp;
  RawDatagramSocket? _udp;

  @override
  void initState() {
    super.initState();
    if (!UsbAoa.isSupported) {
      _append('USB AOA is Android-only — not supported on this platform.');
      return;
    }
    _sub = UsbAoa.events().listen(_onEvent, onError: (Object e) {
      _append('event stream error: $e');
    });
    _bootstrap();
  }

  Future<void> _bootstrap() async {
    // The link/tunnel persist across navigation, so on re-entry just reflect
    // the existing state instead of re-running the handshake.
    final existing = await UsbAoa.state();
    if (existing != null) {
      setState(() => _role = existing);
      _append('● already connected as $existing');
      final ip = await UsbAoa.tunnelLocalIp();
      if (ip != null) {
        setState(() => _tunnelIp = ip);
        _append('▲ tunnel already up — local IP $ip');
        await _startUdp(ip);
      }
      return;
    }
    // If this phone was launched by the cable (accessory attach), connect as
    // the receiver automatically.
    final attached = await UsbAoa.consumeAccessoryAttach();
    if (attached) {
      _append('launched by accessory attach → connecting as accessory…');
      await _connectAccessory();
      return;
    }
    // Auto role-detect: a USB device on our bus means we're the host (the peer
    // is switched to accessory automatically on its side). Otherwise we wait —
    // the native receiver auto-connects us as accessory the moment the host
    // drives the handshake, so no tap is needed on the receiving phone.
    final hasDevice = await UsbAoa.hasHostDevice();
    if (hasDevice) {
      _append('USB device detected → auto-connecting as host…');
      await _connectHost();
    } else {
      _append('Waiting for cable… (host auto-connects; accessory is automatic)');
    }
  }

  @override
  void dispose() {
    // Intentionally DO NOT tear down the link/tunnel here: the native link +
    // VpnService live on MainActivity, so leaving this page keeps the cable up.
    // That lets you navigate to the normal Send/Receive screens and test
    // whether iroh discovers + transfers over the tunnel. Use "Disconnect /
    // clear" (or unplug) to tear it down.
    _sub?.cancel();
    _udp?.close();
    super.dispose();
  }

  void _onEvent(UsbAoaEvent event) {
    switch (event) {
      case UsbAoaConnected(:final role):
        setState(() => _role = role);
        _append('● connected as $role');
        // The host kicks off the round-trip with a PING.
        if (role == 'host') _sendPing();
      case UsbAoaClosed():
        setState(() => _role = null);
        _udp?.close();
        _udp = null;
        _tunnelIp = null;
        _append('○ link closed');
      case UsbAoaData(:final bytes):
        _onData(bytes);
      case UsbAoaTunnelUp(:final ip):
        setState(() => _tunnelIp = ip);
        _append('▲ tunnel up — local IP $ip');
        _startUdp(ip);
      case UsbAoaTunnelClosed():
        setState(() => _tunnelIp = null);
        _udp?.close();
        _udp = null;
        _append('▽ tunnel closed');
    }
  }

  Future<void> _startTunnel() async {
    _append('starting IP tunnel (may prompt for VPN consent)…');
    try {
      final ok = await UsbAoa.startTunnel();
      _append(ok ? 'startTunnel returned ok' : 'startTunnel returned false');
    } catch (e) {
      _append('startTunnel failed: $e');
    }
  }

  Future<void> _startUdp(String localIp) async {
    _udp?.close();
    try {
      final sock = await RawDatagramSocket.bind(InternetAddress(localIp), _udpPort);
      _udp = sock;
      sock.listen((event) {
        if (event != RawSocketEvent.read) return;
        final dg = sock.receive();
        if (dg == null) return;
        final msg = utf8.decode(dg.data, allowMalformed: true);
        if (msg.startsWith('TPING')) {
          sock.send(utf8.encode('TPONG${msg.substring(5)}'), dg.address, dg.port);
          _append('⇄ udp TPING from ${dg.address.address} → replied TPONG');
        } else if (msg.startsWith('TPONG')) {
          _append('✓ UDP round-trip OK over tunnel ($msg)');
        }
      });
      _append('udp bound on $localIp:$_udpPort');
      // Host kicks off the UDP probe to the peer's tunnel IP.
      if (_role == 'host') _sendUdpPing();
    } catch (e) {
      _append('udp bind failed: $e');
    }
  }

  void _sendUdpPing() {
    final sock = _udp;
    final ip = _tunnelIp;
    if (sock == null || ip == null) return;
    final peer = ip == UsbAoaTunnelIps.host
        ? UsbAoaTunnelIps.accessory
        : UsbAoaTunnelIps.host;
    sock.send(
      utf8.encode('TPING@${DateTime.now().millisecondsSinceEpoch}'),
      InternetAddress(peer),
      _udpPort,
    );
    _append('→ udp TPING to $peer:$_udpPort');
  }

  void _onData(Uint8List bytes) {
    final text = _previewText(bytes);
    _append('← recv ${bytes.length}B: $text');
    // Accessory echoes every payload straight back so the host can confirm the
    // full duplex path.
    if (_role == 'accessory') {
      final reply = utf8.encode('ECHO:$text');
      UsbAoa.send(Uint8List.fromList(reply));
      _append('→ echoed ${reply.length}B');
    }
  }

  Future<void> _connectHost() async {
    _append('connecting as host (AOA handshake)…');
    try {
      final ok = await UsbAoa.connectHost();
      _append(ok ? 'host connect returned ok' : 'host connect returned false');
    } catch (e) {
      _append('host connect failed: $e');
    }
  }

  Future<void> _connectAccessory() async {
    _append('opening accessory…');
    try {
      final ok = await UsbAoa.connectAccessory();
      _append(ok ? 'accessory open ok' : 'accessory open returned false');
    } catch (e) {
      _append('accessory open failed: $e');
    }
  }

  Future<void> _sendPing() async {
    _pingCounter += 1;
    final payload = utf8.encode('PING#$_pingCounter@${DateTime.now().millisecondsSinceEpoch}');
    final ok = await UsbAoa.send(Uint8List.fromList(payload));
    _append(ok ? '→ sent ${payload.length}B PING#$_pingCounter' : '→ send failed');
  }

  String _previewText(Uint8List bytes) {
    try {
      final s = utf8.decode(bytes, allowMalformed: true);
      return s.length > 64 ? '${s.substring(0, 64)}…' : s;
    } catch (_) {
      return '<${bytes.length} binary bytes>';
    }
  }

  void _append(String line) {
    if (!mounted) return;
    setState(() => _log.insert(0, line));
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      appBar: AppBar(title: const Text('USB cable spike (dev)')),
      body: Padding(
        padding: const EdgeInsets.all(16),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Text(
              _role == null
                  ? 'Not connected'
                  : 'Connected as $_role'
                        '${_tunnelIp != null ? ' · tunnel $_tunnelIp' : ''}',
              style: Theme.of(context).textTheme.titleMedium,
            ),
            const SizedBox(height: 12),
            Wrap(
              spacing: 8,
              runSpacing: 8,
              children: [
                FilledButton.icon(
                  onPressed: _connectHost,
                  icon: const Icon(Icons.usb_rounded),
                  label: const Text('Connect as host (sender)'),
                ),
                OutlinedButton.icon(
                  onPressed: _connectAccessory,
                  icon: const Icon(Icons.cable_rounded),
                  label: const Text('Connect as accessory (receiver)'),
                ),
                OutlinedButton.icon(
                  onPressed: _role == null ? null : _sendPing,
                  icon: const Icon(Icons.send_rounded),
                  label: const Text('Send PING'),
                ),
                FilledButton.icon(
                  onPressed: _role == null ? null : _startTunnel,
                  icon: const Icon(Icons.vpn_lock_rounded),
                  label: const Text('Start IP tunnel'),
                ),
                OutlinedButton.icon(
                  onPressed: _tunnelIp == null ? null : _sendUdpPing,
                  icon: const Icon(Icons.network_ping_rounded),
                  label: const Text('UDP ping over tunnel'),
                ),
                TextButton.icon(
                  onPressed: () {
                    UsbAoa.disconnect();
                    setState(() => _log.clear());
                  },
                  icon: const Icon(Icons.clear_rounded),
                  label: const Text('Disconnect / clear'),
                ),
              ],
            ),
            const Divider(height: 24),
            Expanded(
              child: ListView.builder(
                itemCount: _log.length,
                itemBuilder: (context, i) => Padding(
                  padding: const EdgeInsets.symmetric(vertical: 2),
                  child: Text(
                    _log[i],
                    style: const TextStyle(
                      fontFamily: 'monospace',
                      fontSize: 12.5,
                    ),
                  ),
                ),
              ),
            ),
          ],
        ),
      ),
    );
  }
}
