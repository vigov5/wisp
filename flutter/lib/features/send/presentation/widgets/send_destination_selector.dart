import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../../../src/rust/api/lan.dart' as rust_lan;
import '../../../../theme/drift_theme.dart';
import '../../../receive/application/service.dart';
import '../../../receive/application/state.dart';
import '../../../saved_devices/application/saved_device.dart';
import '../../../saved_devices/application/saved_devices_controller.dart';
import '../../../transfers/application/pubkey_visual.dart';
import '../../application/controller.dart';
import '../../application/model.dart';
import '../../application/state.dart';
import '../qr_scan_page.dart';
import '../receive_code_field.dart';

class SendDestinationSelector extends ConsumerStatefulWidget {
  const SendDestinationSelector({super.key, required this.controller});

  final SendController controller;

  @override
  ConsumerState<SendDestinationSelector> createState() =>
      _SendDestinationSelectorState();
}

class _SendDestinationSelectorState
    extends ConsumerState<SendDestinationSelector> {
  List<NearbyReceiver> _nearbyDevices = const [];
  final Map<String, int> _deviceMissCount = {};
  bool _isScanningNearby = false;
  bool _nearbyScanCompletedOnce = false;
  bool _continuousMode = false;
  Timer? _scanThrottleTimer;
  Completer<void>? _scanThrottleCompleter;
  // Receiver paired via QR scan. Sticky in the UI until the user dismisses
  // it or scans a different one — gives the user a visible target so they
  // know what they're sending to (Nearby/Recent won't show this peer).
  NearbyReceiver? _qrPairedReceiver;

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted) {
        unawaited(_startContinuousScan());
      }
    });
  }

  @override
  void dispose() {
    _continuousMode = false;
    _scanThrottleTimer?.cancel();
    _scanThrottleTimer = null;
    if (_scanThrottleCompleter != null &&
        !_scanThrottleCompleter!.isCompleted) {
      _scanThrottleCompleter!.complete();
    }
    _scanThrottleCompleter = null;
    super.dispose();
  }

  Future<void> _startContinuousScan() async {
    if (_isScanningNearby) return;
    setState(() {
      _continuousMode = true;
      _isScanningNearby = true;
    });
    await _runScanLoop();
  }

  void _stopScan() {
    _deviceMissCount.clear();
    setState(() {
      _continuousMode = false;
      _isScanningNearby = false;
    });
  }

  void _selectSavedDevice(SavedDevice device) {
    final ticket = device.lastTicket;
    if (ticket == null || ticket.isEmpty) {
      // No cached ticket: tell the user we can't fast-path. Once we add a
      // pkarr-only connect path (synthesize EndpointAddr from EndpointId),
      // this branch goes away.
      ScaffoldMessenger.of(context).showSnackBar(
        SnackBar(
          content: Text(
            'No cached connection info for ${device.label}. '
            'Find them on Nearby or use a code.',
          ),
        ),
      );
      return;
    }
    widget.controller.selectNearbyReceiver(
      NearbyReceiver(
        fullname: 'saved-${device.endpointId}',
        label: device.label,
        deviceType: device.deviceType,
        code: '',
        ticket: ticket,
      ),
    );
  }

  /// Minimum wall time per loop iteration. Prevents the loop from spinning the
  /// microtask queue when [scanNearby] returns instantly (e.g. in tests with a
  /// fake source) — without this, `pumpAndSettle` never settles. In production
  /// the Rust scan already takes ~3 s, so this gate adds no latency.
  static const Duration _minLoopInterval = Duration(seconds: 1);

  Future<void> _runScanLoop() async {
    final iteration = Stopwatch();
    while (mounted && _continuousMode) {
      iteration
        ..reset()
        ..start();
      try {
        final devices = await ref
            .read(receiverServiceProvider.notifier)
            .scanNearby(timeout: const Duration(seconds: 3));
        if (!mounted || !_continuousMode) break;
        // Merge results: add/refresh seen devices, tolerate up to 2 consecutive
        // missed rounds before removing (prevents flicker on intermittent scans).
        final fresh = devices.map((d) => d.fullname).toSet();
        final merged = <String, NearbyReceiver>{};
        for (final d in _nearbyDevices) {
          merged[d.fullname] = d;
        }
        for (final d in devices) {
          merged[d.fullname] = d;
          _deviceMissCount.remove(d.fullname); // seen this round → reset
        }
        for (final key in merged.keys.toList()) {
          if (!fresh.contains(key)) {
            final misses = (_deviceMissCount[key] ?? 0) + 1;
            _deviceMissCount[key] = misses;
            if (misses >= 2) {
              merged.remove(key);
              _deviceMissCount.remove(key);
            }
          }
        }
        setState(() {
          _nearbyDevices = merged.values.toList();
          _nearbyScanCompletedOnce = true;
        });
      } catch (_) {
        if (!mounted || !_continuousMode) break;
        setState(() {
          _nearbyScanCompletedOnce = true;
        });
      }
      if (!mounted || !_continuousMode) break;
      final remaining = _minLoopInterval - iteration.elapsed;
      if (remaining > Duration.zero) {
        final completer = Completer<void>();
        _scanThrottleCompleter = completer;
        _scanThrottleTimer = Timer(remaining, () {
          if (!completer.isCompleted) {
            completer.complete();
          }
        });
        await completer.future;
        _scanThrottleTimer?.cancel();
        _scanThrottleTimer = null;
        _scanThrottleCompleter = null;
      }
    }
    if (mounted) {
      setState(() {
        _isScanningNearby = false;
      });
    }
  }

  @override
  Widget build(BuildContext context) {
    final state = ref.watch(sendControllerProvider);
    final destination = switch (state) {
      SendStateDrafting(:final destination) => destination,
      SendStateTransferring(:final destination) => destination,
      SendStateResult(:final destination) => destination,
      SendStateIdle() => const SendDestinationState.none(),
    };

    final titleStyle = driftSans(
      fontSize: 17,
      fontWeight: FontWeight.w700,
      color: kInk,
    );

    // Recent = saved devices NOT currently visible on the LAN scan.
    // (When a saved device is also nearby, we only show the live Nearby tile to
    // avoid duplicates.)
    final savedDevices = ref.watch(savedDevicesProvider);
    final nearbyTickets = _nearbyDevices.map((d) => d.ticket).toSet();
    final recentDevices = savedDevices
        .where((d) => !nearbyTickets.contains(d.lastTicket ?? ''))
        .toList(growable: false);

    final qrPaired = _qrPairedReceiver;
    final qrSelected =
        qrPaired != null &&
        destination.mode == SendDestinationMode.nearby &&
        destination.ticket == qrPaired.ticket;

    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Row(
          children: [
            Text('Pair via QR code', style: titleStyle),
            const Spacer(),
            TextButton.icon(
              onPressed: _scanQr,
              icon: const Icon(Icons.qr_code_scanner_rounded, size: 18),
              label: Text(
                qrPaired == null ? 'Scan QR' : 'Re-scan',
                style: driftSans(
                  fontSize: 13,
                  fontWeight: FontWeight.w500,
                  color: const Color(0xFF7AAFC9),
                ),
              ),
              style: TextButton.styleFrom(
                padding: EdgeInsets.zero,
                minimumSize: Size.zero,
                tapTargetSize: MaterialTapTargetSize.shrinkWrap,
                foregroundColor: const Color(0xFF7AAFC9),
              ),
            ),
          ],
        ),
        const SizedBox(height: 12),
        if (qrPaired != null)
          _QrPairedTile(
            receiver: qrPaired,
            isSelected: qrSelected,
            onTap: () => widget.controller.selectNearbyReceiver(qrPaired),
            onDismiss: _clearQrPaired,
          )
        else
          Text(
            'Scan a receiver\'s QR code to pair offline.',
            style: driftSans(fontSize: 12.5, color: kMuted, height: 1.4),
          ),
        const SizedBox(height: 18),
        if (recentDevices.isNotEmpty) ...[
          Text('Recent', style: titleStyle),
          const SizedBox(height: 12),
          SizedBox(
            height: 120,
            child: ListView.separated(
              scrollDirection: Axis.horizontal,
              physics: const BouncingScrollPhysics(),
              itemCount: recentDevices.length,
              separatorBuilder: (_, _) => const SizedBox(width: 12),
              itemBuilder: (context, index) {
                final saved = recentDevices[index];
                final selected =
                    destination.mode == SendDestinationMode.nearby &&
                    (saved.lastTicket?.isNotEmpty ?? false) &&
                    destination.ticket == saved.lastTicket;
                return _RecentDeviceTile(
                  device: saved,
                  isSelected: selected,
                  onTap: () => _selectSavedDevice(saved),
                );
              },
            ),
          ),
          const SizedBox(height: 18),
        ],
        Row(
          children: [
            Text('Nearby devices', style: titleStyle),
            const Spacer(),
            _ScanAction(
              isScanning: _isScanningNearby,
              isContinuous: _continuousMode,
              onStart: _startContinuousScan,
              onStop: _stopScan,
            ),
          ],
        ),
        const SizedBox(height: 12),
        if (_nearbyDevices.isEmpty)
          _NearbyStatusCard(
            isScanning: _isScanningNearby && !_nearbyScanCompletedOnce,
          )
        else
          SizedBox(
            height: 124,
            child: ListView.separated(
              scrollDirection: Axis.horizontal,
              physics: const BouncingScrollPhysics(),
              itemCount: _nearbyDevices.length,
              separatorBuilder: (_, _) => const SizedBox(width: 12),
              itemBuilder: (context, index) {
                final receiver = _nearbyDevices[index];
                final selected =
                    destination.mode == SendDestinationMode.nearby &&
                    destination.ticket == receiver.ticket;
                return _NearbyDeviceTile(
                  receiver: receiver,
                  isSelected: selected,
                  isStale: _deviceMissCount.containsKey(receiver.fullname),
                  icon: _deviceIconForType(receiver.deviceType),
                  onTap: () => widget.controller.selectNearbyReceiver(receiver),
                );
              },
            ),
          ),
        const SizedBox(height: 18),
        Text('Send with code', style: titleStyle),
        const SizedBox(height: 6),
        Text(
          'Use the 6 characters shown on the receiver.',
          style: driftSans(fontSize: 13.5, color: kMuted, height: 1.4),
        ),
        const SizedBox(height: 16),
        ReceiveCodeField(
          code: destination.mode == SendDestinationMode.code
              ? destination.code ?? ''
              : '',
          onChanged: widget.controller.updateDestinationCode,
          hintText: 'AB12CD',
          understated: true,
        ),
      ],
    );
  }

  Future<void> _scanQr() async {
    final ticket = await Navigator.of(context).push<String>(
      MaterialPageRoute<String>(builder: (_) => const QrScanPage()),
    );
    if (!mounted || ticket == null || ticket.isEmpty) return;

    // Decode the ticket so the tile can show the receiver's name + type +
    // pubkey before we even dial.  Failure here means the QR isn't a drift
    // ticket — surface the error and bail.
    final rust_lan.DecodedTicketData decoded;
    try {
      decoded = rust_lan.decodeTicketInfo(ticket: ticket);
    } catch (e) {
      if (!mounted) return;
      ScaffoldMessenger.of(context).showSnackBar(
        SnackBar(
          content: Text("Couldn't read QR: ${e.toString()}"),
          duration: const Duration(seconds: 3),
        ),
      );
      return;
    }

    final label = decoded.deviceName.trim().isEmpty
        ? 'From QR code'
        : decoded.deviceName.trim();
    final deviceType = decoded.deviceType.trim().isEmpty
        ? 'phone'
        : decoded.deviceType.trim();

    final receiver = NearbyReceiver(
      fullname: 'qr-${ticket.hashCode}',
      label: label,
      deviceType: deviceType,
      code: '',
      ticket: ticket,
      endpointId: decoded.endpointId,
    );
    setState(() => _qrPairedReceiver = receiver);
    widget.controller.selectNearbyReceiver(receiver);
  }

  void _clearQrPaired() {
    setState(() => _qrPairedReceiver = null);
  }
}

class _ScanAction extends StatelessWidget {
  const _ScanAction({
    required this.isScanning,
    required this.isContinuous,
    required this.onStart,
    required this.onStop,
  });

  final bool isScanning;
  final bool isContinuous;
  final VoidCallback onStart;
  final VoidCallback onStop;

  @override
  Widget build(BuildContext context) {
    const color = Color(0xFF7AAFC9);
    const style = ButtonStyle(
      padding: WidgetStatePropertyAll(EdgeInsets.zero),
      minimumSize: WidgetStatePropertyAll(Size.zero),
      tapTargetSize: MaterialTapTargetSize.shrinkWrap,
      foregroundColor: WidgetStatePropertyAll(color),
    );

    if (isScanning && isContinuous) {
      return TextButton.icon(
        onPressed: onStop,
        icon: const Icon(Icons.stop_rounded, size: 18),
        label: Text(
          'Stop',
          style: driftSans(
            fontSize: 13,
            fontWeight: FontWeight.w500,
            color: color,
          ),
        ),
        style: style,
      );
    }

    if (isScanning) {
      return const SizedBox(
        width: 20,
        height: 20,
        child: CircularProgressIndicator(
          strokeWidth: 2,
          valueColor: AlwaysStoppedAnimation<Color>(color),
        ),
      );
    }

    return TextButton.icon(
      onPressed: onStart,
      icon: const Icon(Icons.refresh_rounded, size: 18),
      label: Text(
        'Rescan',
        style: driftSans(
          fontSize: 13,
          fontWeight: FontWeight.w500,
          color: color,
        ),
      ),
      style: style,
    );
  }
}

class _NearbyStatusCard extends StatelessWidget {
  const _NearbyStatusCard({required this.isScanning});

  final bool isScanning;

  @override
  Widget build(BuildContext context) {
    final title = isScanning
        ? 'Scanning for nearby receivers...'
        : 'No nearby devices found';
    final subtitle = isScanning
        ? 'Make sure both devices are on the same Wi-Fi.'
        : 'Make sure both devices are on the same Wi-Fi. Local network access may be required.';

    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 14),
      decoration: BoxDecoration(
        color: kSurface,
        borderRadius: BorderRadius.circular(16),
        border: Border.all(color: kBorder.withValues(alpha: 0.55)),
      ),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Padding(
            padding: const EdgeInsets.only(top: 2),
            child: SizedBox(
              width: 22,
              height: 22,
              child: Icon(
                isScanning ? Icons.radar_rounded : Icons.wifi_off_rounded,
                size: 20,
                color: const Color(0xFF8E8E8E),
              ),
            ),
          ),
          const SizedBox(width: 12),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(
                  title,
                  style: driftSans(
                    fontSize: 14,
                    fontWeight: FontWeight.w700,
                    color: kInk,
                  ),
                ),
                const SizedBox(height: 4),
                Text(
                  subtitle,
                  style: driftSans(
                    fontSize: 13,
                    fontWeight: FontWeight.w400,
                    color: kMuted,
                    height: 1.35,
                  ),
                ),
              ],
            ),
          ),
        ],
      ),
    );
  }
}

IconData _deviceIconForType(String deviceType) {
  return deviceType.toLowerCase() == 'phone'
      ? Icons.smartphone_rounded
      : Icons.laptop_mac_rounded;
}

String _relativeTime(DateTime t) {
  final delta = DateTime.now().toUtc().difference(t.toUtc());
  if (delta.inMinutes < 1) return 'just now';
  if (delta.inHours < 1) return '${delta.inMinutes}m ago';
  if (delta.inDays < 1) return '${delta.inHours}h ago';
  if (delta.inDays < 7) return '${delta.inDays}d ago';
  if (delta.inDays < 30) return '${(delta.inDays / 7).floor()}w ago';
  return '${(delta.inDays / 30).floor()}mo ago';
}

class _RecentDeviceTile extends StatelessWidget {
  const _RecentDeviceTile({
    required this.device,
    required this.isSelected,
    required this.onTap,
  });

  final SavedDevice device;
  final bool isSelected;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final badgeColor = colorFromPubkey(device.endpointId);
    return InkWell(
      onTap: onTap,
      borderRadius: BorderRadius.circular(20),
      child: AnimatedContainer(
        duration: const Duration(milliseconds: 200),
        width: 116,
        padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 12),
        decoration: BoxDecoration(
          color: isSelected ? const Color(0xFFF4F8FA) : kSurface,
          borderRadius: BorderRadius.circular(16),
          border: Border.all(
            color: isSelected ? const Color(0xFF8DBED4) : kBorder,
          ),
        ),
        child: Column(
          mainAxisAlignment: MainAxisAlignment.center,
          children: [
            Icon(
              _deviceIconForType(device.deviceType),
              size: 22,
              color: isSelected ? const Color(0xFF7AAFC9) : kMuted,
            ),
            const SizedBox(height: 8),
            Text(
              device.label.isEmpty ? 'Saved device' : device.label,
              style: driftSans(
                fontSize: 12.5,
                fontWeight: FontWeight.w600,
                color: kInk,
                height: 1.18,
              ),
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              textAlign: TextAlign.center,
            ),
            const SizedBox(height: 4),
            Tooltip(
              message:
                  'Identity badge (from public key) — same color = same device.',
              child: Container(
                padding: const EdgeInsets.symmetric(horizontal: 6, vertical: 2),
                decoration: BoxDecoration(
                  color: badgeColor.withValues(alpha: 0.15),
                  borderRadius: BorderRadius.circular(6),
                  border: Border.all(
                    color: badgeColor.withValues(alpha: 0.45),
                    width: 0.8,
                  ),
                ),
                child: Text(
                  shortPubkey(device.endpointId),
                  style: driftSans(
                    fontSize: 11,
                    fontWeight: FontWeight.w700,
                    color: HSLColor.fromColor(
                      badgeColor,
                    ).withLightness(0.32).toColor(),
                    letterSpacing: 0.4,
                  ),
                ),
              ),
            ),
            const SizedBox(height: 2),
            Text(
              _relativeTime(device.lastSeenAt),
              style: driftSans(
                fontSize: 9.5,
                fontWeight: FontWeight.w400,
                color: kMuted,
              ),
            ),
          ],
        ),
      ),
    );
  }
}

class _NearbyDeviceTile extends StatelessWidget {
  const _NearbyDeviceTile({
    required this.receiver,
    required this.isSelected,
    required this.isStale,
    required this.icon,
    required this.onTap,
  });

  final NearbyReceiver receiver;
  final bool isSelected;
  final bool isStale;
  final IconData icon;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    return AnimatedOpacity(
      duration: const Duration(milliseconds: 300),
      opacity: isStale ? 0.45 : 1.0,
      child: InkWell(
        onTap: onTap,
        borderRadius: BorderRadius.circular(20),
        child: AnimatedContainer(
          duration: const Duration(milliseconds: 200),
          width: 106,
          padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 12),
          decoration: BoxDecoration(
            color: isSelected ? const Color(0xFFF4F8FA) : kSurface,
            borderRadius: BorderRadius.circular(16),
            border: Border.all(
              color: isSelected
                  ? const Color(0xFF8DBED4)
                  : isStale
                  ? kBorder.withValues(alpha: 0.5)
                  : kBorder,
            ),
          ),
          child: Column(
            mainAxisAlignment: MainAxisAlignment.center,
            children: [
              Icon(
                icon,
                size: 20,
                color: isSelected ? const Color(0xFF7AAFC9) : kMuted,
              ),
              const SizedBox(height: 8),
              Text(
                receiver.label,
                style: driftSans(
                  fontSize: 12.5,
                  fontWeight: FontWeight.w600,
                  color: kInk,
                  height: 1.18,
                ),
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                textAlign: TextAlign.center,
              ),
              if (receiver.endpointId.isNotEmpty) ...[
                const SizedBox(height: 4),
                Container(
                  padding: const EdgeInsets.symmetric(
                    horizontal: 6,
                    vertical: 1,
                  ),
                  decoration: BoxDecoration(
                    color: colorFromPubkey(
                      receiver.endpointId,
                    ).withValues(alpha: 0.15),
                    borderRadius: BorderRadius.circular(5),
                    border: Border.all(
                      color: colorFromPubkey(
                        receiver.endpointId,
                      ).withValues(alpha: 0.45),
                      width: 0.6,
                    ),
                  ),
                  child: Text(
                    shortPubkey(receiver.endpointId),
                    style: driftSans(
                      fontSize: 9.5,
                      fontWeight: FontWeight.w700,
                      color: HSLColor.fromColor(
                        colorFromPubkey(receiver.endpointId),
                      ).withLightness(0.32).toColor(),
                      letterSpacing: 0.3,
                    ),
                  ),
                ),
              ],
            ],
          ),
        ),
      ),
    );
  }
}

class _QrPairedTile extends StatelessWidget {
  const _QrPairedTile({
    required this.receiver,
    required this.isSelected,
    required this.onTap,
    required this.onDismiss,
  });

  final NearbyReceiver receiver;
  final bool isSelected;
  final VoidCallback onTap;
  final VoidCallback onDismiss;

  @override
  Widget build(BuildContext context) {
    final pubkey = receiver.endpointId;
    final badgeColor = pubkey.isEmpty ? kMuted : colorFromPubkey(pubkey);
    final badgeText = pubkey.isEmpty
        ? null
        : HSLColor.fromColor(badgeColor).withLightness(0.32).toColor();

    return InkWell(
      onTap: onTap,
      borderRadius: BorderRadius.circular(16),
      child: AnimatedContainer(
        duration: const Duration(milliseconds: 200),
        padding: const EdgeInsets.symmetric(horizontal: 14, vertical: 12),
        decoration: BoxDecoration(
          color: isSelected ? const Color(0xFFF4F8FA) : kSurface,
          borderRadius: BorderRadius.circular(16),
          border: Border.all(
            color: isSelected ? const Color(0xFF8DBED4) : kBorder,
          ),
        ),
        child: Row(
          children: [
            Icon(
              _deviceIconForType(receiver.deviceType),
              size: 24,
              color: isSelected ? const Color(0xFF7AAFC9) : kMuted,
            ),
            const SizedBox(width: 14),
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text.rich(
                    TextSpan(
                      children: [
                        TextSpan(
                          text: receiver.label,
                          style: driftSans(
                            fontSize: 13.5,
                            fontWeight: FontWeight.w700,
                            color: kInk,
                          ),
                        ),
                        TextSpan(
                          text: '  ·  Paired offline',
                          style: driftSans(
                            fontSize: 11.5,
                            fontWeight: FontWeight.w400,
                            color: kMuted,
                          ),
                        ),
                      ],
                    ),
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                  ),
                  if (pubkey.isNotEmpty) ...[
                    const SizedBox(height: 6),
                    Container(
                      padding: const EdgeInsets.symmetric(
                        horizontal: 6,
                        vertical: 2,
                      ),
                      decoration: BoxDecoration(
                        color: badgeColor.withValues(alpha: 0.15),
                        borderRadius: BorderRadius.circular(6),
                        border: Border.all(
                          color: badgeColor.withValues(alpha: 0.45),
                          width: 0.8,
                        ),
                      ),
                      child: Text(
                        shortPubkey(pubkey),
                        style: driftSans(
                          fontSize: 10.5,
                          fontWeight: FontWeight.w700,
                          color: badgeText,
                          letterSpacing: 0.4,
                        ),
                      ),
                    ),
                  ],
                ],
              ),
            ),
            const SizedBox(width: 8),
            const Icon(
              Icons.qr_code_rounded,
              size: 18,
              color: Color(0xFF7AAFC9),
            ),
            const SizedBox(width: 4),
            IconButton(
              icon: const Icon(Icons.close_rounded, size: 16),
              color: kMuted,
              tooltip: 'Dismiss',
              visualDensity: VisualDensity.compact,
              padding: EdgeInsets.zero,
              constraints: const BoxConstraints(minWidth: 28, minHeight: 28),
              onPressed: onDismiss,
            ),
          ],
        ),
      ),
    );
  }
}
