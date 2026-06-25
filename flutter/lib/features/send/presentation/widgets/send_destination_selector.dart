import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../../../platform/android/usb_tether_channel.dart';
import '../../../../src/rust/api/lan.dart' as rust_lan;
import '../../../../theme/wisp_theme.dart';
import '../../../receive/application/service.dart';
import '../../../receive/application/state.dart';
import '../../../saved_devices/application/device_display_name.dart';
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
  // Last-detected USB-tethering link (null = no cable link). Refreshed each
  // scan tick; a peer reached over the cable shows up in the Nearby list like
  // any other, so this only drives the cable-status banner + setup guidance.
  rust_lan.UsbLinkData? _usbLink;

  @override
  void initState() {
    super.initState();
    _refreshUsbLink();
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted) {
        unawaited(_startContinuousScan());
      }
    });
  }

  // Cheap, synchronous interface enumeration in Rust. Guarded so a platform
  // without the bridge (e.g. web) just reports "no cable" instead of throwing.
  void _refreshUsbLink() {
    rust_lan.UsbLinkData? link;
    try {
      link = rust_lan.detectUsbLink();
    } catch (_) {
      link = null;
    }
    if (!mounted) return;
    if (link?.localIp != _usbLink?.localIp || link?.isHost != _usbLink?.isHost) {
      setState(() => _usbLink = link);
    }
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

  /// Identity key used by [_runScanLoop] to dedupe nearby devices.  Prefers
  /// the iroh pubkey when known (so the same device behind two mDNS records,
  /// or after renaming, collapses to one tile).  Falls back to fullname for
  /// legacy peers that don't surface a pubkey yet.
  static String _identityKey(NearbyReceiver d) =>
      d.endpointId.isNotEmpty ? 'pk:${d.endpointId}' : 'fn:${d.fullname}';

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
      _refreshUsbLink();
      try {
        final devices = await ref
            .read(receiverServiceProvider.notifier)
            .scanNearby(timeout: const Duration(seconds: 3));
        if (!mounted || !_continuousMode) break;
        // Merge by pubkey when available (so two mDNS records for the same
        // device collapse into one tile, and a renamed device replaces the
        // cached entry with its fresh label).  Fall back to fullname when
        // pubkey is empty so legacy peers still get their own entry.
        final fresh = devices.map(_identityKey).toSet();
        final merged = <String, NearbyReceiver>{};
        for (final d in _nearbyDevices) {
          merged[_identityKey(d)] = d;
        }
        for (final d in devices) {
          // Fresh scan wins: overwrites any cached entry under the same
          // identity key, so a device that changed its mDNS label between
          // sessions shows its new name immediately.
          merged[_identityKey(d)] = d;
          _deviceMissCount.remove(_identityKey(d));
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

    final titleStyle = wispSans(
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
                style: wispSans(
                  fontSize: 13,
                  fontWeight: FontWeight.w500,
                  color: kAccentCyan,
                ),
              ),
              style: TextButton.styleFrom(
                padding: EdgeInsets.zero,
                minimumSize: Size.zero,
                tapTargetSize: MaterialTapTargetSize.shrinkWrap,
                foregroundColor: kAccentCyan,
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
            style: wispSans(fontSize: 12.5, color: kMuted, height: 1.4),
          ),
        const SizedBox(height: 18),
        if (recentDevices.isNotEmpty) ...[
          Text('Recent', style: titleStyle),
          const SizedBox(height: 12),
          SizedBox(
            height: 134,
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
            height: 134,
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
          style: wispSans(fontSize: 13.5, color: kMuted, height: 1.4),
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
        const SizedBox(height: 18),
        _UsbCableSection(
          link: _usbLink,
          titleStyle: titleStyle,
          onOpenTethering: _openTethering,
          onShowGuide: _showUsbGuide,
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
    // pubkey before we even dial.  Failure here means the QR isn't a wisp
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

  Future<void> _openTethering() async {
    final opened = await UsbTether.openTetherSettings();
    if (!mounted) return;
    if (!opened) {
      ScaffoldMessenger.of(context).showSnackBar(
        const SnackBar(
          content: Text(
            'Open Settings → Hotspot & tethering, then turn on USB tethering.',
          ),
          duration: Duration(seconds: 4),
        ),
      );
    }
    // Give the interface a moment to come up, then re-check.
    Future<void>.delayed(const Duration(seconds: 1), _refreshUsbLink);
  }

  void _showUsbGuide() {
    showModalBottomSheet<void>(
      context: context,
      backgroundColor: kBg,
      showDragHandle: true,
      isScrollControlled: true,
      shape: const RoundedRectangleBorder(
        borderRadius: BorderRadius.vertical(top: Radius.circular(24)),
      ),
      builder: (_) => _UsbGuideSheet(
        link: _usbLink,
        onOpenTethering: _openTethering,
      ),
    );
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
    const color = kAccentCyan;
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
          style: wispSans(
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
        style: wispSans(
          fontSize: 13,
          fontWeight: FontWeight.w500,
          color: color,
        ),
      ),
      style: style,
    );
  }
}

/// "Connect over USB cable" section. The cable peer appears in the Nearby list
/// like any other device, so this block's job is to (a) report cable-link
/// status and (b) walk the user through the one manual step Android requires —
/// turning on USB tethering. Positioned around the cases MTP handles poorly:
/// sending *to* a phone, bulk transfers, and when there's no shared Wi-Fi.
class _UsbCableSection extends StatelessWidget {
  const _UsbCableSection({
    required this.link,
    required this.titleStyle,
    required this.onOpenTethering,
    required this.onShowGuide,
  });

  final rust_lan.UsbLinkData? link;
  final TextStyle titleStyle;
  final Future<void> Function() onOpenTethering;
  final VoidCallback onShowGuide;

  @override
  Widget build(BuildContext context) {
    final link = this.link;
    final active = link != null;
    final isHost = link?.isHost ?? false;

    // Detection is a best-effort *positive* hint only. On Windows the RNDIS
    // adapter has no telltale name and the tether subnet varies by phone, so a
    // live cable often can't be detected — the inactive state must therefore
    // read as neutral guidance, never "no cable". The transfer works either
    // way: discovery broadcasts to every interface's subnet regardless.
    final String statusText;
    final IconData statusIcon;
    if (!active) {
      statusText = 'Connect a cable, then turn on USB tethering on the phone.';
      statusIcon = Icons.cable_rounded;
    } else if (isHost) {
      statusText = 'Tethering on — keep this screen open.';
      statusIcon = Icons.usb_rounded;
    } else {
      statusText = 'Cable link active · ${link.localIp}';
      statusIcon = Icons.check_circle_rounded;
    }
    final statusColor = active ? kAccentDirect : kMuted;

    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Row(
          children: [
            Text('USB cable', style: titleStyle),
            const SizedBox(width: 8),
            Icon(
              active ? Icons.usb_rounded : Icons.cable_rounded,
              size: 16,
              color: statusColor,
            ),
            const Spacer(),
            TextButton.icon(
              onPressed: onShowGuide,
              icon: const Icon(Icons.help_outline_rounded, size: 18),
              label: Text(
                'How it works',
                style: wispSans(
                  fontSize: 13,
                  fontWeight: FontWeight.w500,
                  color: kAccentCyan,
                ),
              ),
              style: TextButton.styleFrom(
                padding: EdgeInsets.zero,
                minimumSize: Size.zero,
                tapTargetSize: MaterialTapTargetSize.shrinkWrap,
                foregroundColor: kAccentCyan,
              ),
            ),
          ],
        ),
        const SizedBox(height: 6),
        Text(
          'No shared Wi-Fi or internet — a direct, encrypted link over the '
          'cable. Great for large transfers and sending to a phone.',
          style: wispSans(fontSize: 12.5, color: kMuted, height: 1.4),
        ),
        const SizedBox(height: 10),
        Container(
          padding: const EdgeInsets.symmetric(horizontal: 14, vertical: 12),
          decoration: BoxDecoration(
            color: kSurface,
            borderRadius: BorderRadius.circular(16),
            border: Border.all(
              color: active
                  ? kAccentDirect.withValues(alpha: 0.45)
                  : kBorder.withValues(alpha: 0.55),
            ),
          ),
          child: Row(
            children: [
              Icon(statusIcon, size: 20, color: statusColor),
              const SizedBox(width: 12),
              Expanded(
                child: Text(
                  statusText,
                  style: wispSans(
                    fontSize: 13.5,
                    fontWeight: FontWeight.w600,
                    color: active ? kInk : kMuted,
                  ),
                ),
              ),
            ],
          ),
        ),
        // Only offer the deep-link where it does something: on Android (the
        // device that actually toggles tethering) and only while no link is up
        // yet. On Windows/desktop the cable's host is the phone, so there's
        // nothing to turn on here; once a link is active the action is moot.
        // Sits outside the status card, right-aligned, matching the flat
        // "Add files / Add folders" action affordance.
        if (UsbTether.isSupported && !active) ...[
          const SizedBox(height: 8),
          Align(
            alignment: Alignment.centerRight,
            child: TextButton.icon(
              onPressed: onOpenTethering,
              icon: const Icon(Icons.settings_rounded, size: 18),
              label: Text(
                'Turn on tethering',
                style: wispSans(
                  fontSize: 13,
                  fontWeight: FontWeight.w500,
                  color: kAccentCyan,
                ),
              ),
              style: TextButton.styleFrom(
                padding: EdgeInsets.zero,
                minimumSize: Size.zero,
                tapTargetSize: MaterialTapTargetSize.shrinkWrap,
                foregroundColor: kAccentCyan,
              ),
            ),
          ),
        ],
      ],
    );
  }
}

/// Bottom-sheet walkthrough for the USB-cable flow. Steps are deliberately
/// short; the primary CTA jumps straight to the tethering settings screen.
class _UsbGuideSheet extends StatelessWidget {
  const _UsbGuideSheet({required this.link, required this.onOpenTethering});

  final rust_lan.UsbLinkData? link;
  final Future<void> Function() onOpenTethering;

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: EdgeInsets.fromLTRB(
        24,
        4,
        24,
        24 + MediaQuery.of(context).viewInsets.bottom,
      ),
      child: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Text(
            'Send over a USB cable',
            style: wispSans(
              fontSize: 18,
              fontWeight: FontWeight.w700,
              color: kInk,
            ),
          ),
          const SizedBox(height: 6),
          Text(
            'A direct, encrypted link over the cable — no shared Wi-Fi, no '
            'internet. Best for large transfers and sending files to a phone '
            '(where USB file transfer / MTP is slow or unreliable).',
            style: wispSans(fontSize: 13, color: kMuted, height: 1.45),
          ),
          const SizedBox(height: 18),
          const _UsbGuideStep(
            n: 1,
            text: 'Connect the two devices with a USB-C cable.',
          ),
          const _UsbGuideStep(
            n: 2,
            text:
                'On the Android phone, turn on USB tethering '
                '(Settings → Hotspot & tethering → USB tethering).',
          ),
          const _UsbGuideStep(
            n: 3,
            text:
                'The other device appears under "Nearby devices" — pick it '
                'and send as usual.',
            last: true,
          ),
          // Deep-link only on Android (where tethering is actually toggled).
          // On Windows/desktop the host is the phone, so there's no settings
          // screen to open here — the steps above already say to do it there.
          if (UsbTether.isSupported) ...[
            const SizedBox(height: 18),
            FilledButton.icon(
              onPressed: () async {
                await onOpenTethering();
                if (context.mounted) Navigator.of(context).pop();
              },
              icon: const Icon(Icons.settings_rounded, size: 18),
              label: const Text('Open tethering settings'),
              style: FilledButton.styleFrom(
                backgroundColor: kAccentCyanStrong,
                foregroundColor: Colors.white,
                padding: const EdgeInsets.symmetric(vertical: 14),
                shape: RoundedRectangleBorder(
                  borderRadius: BorderRadius.circular(14),
                ),
              ),
            ),
          ],
        ],
      ),
    );
  }
}

class _UsbGuideStep extends StatelessWidget {
  const _UsbGuideStep({
    required this.n,
    required this.text,
    this.last = false,
  });

  final int n;
  final String text;
  final bool last;

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: EdgeInsets.only(bottom: last ? 0 : 14),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Container(
            width: 26,
            height: 26,
            alignment: Alignment.center,
            decoration: BoxDecoration(
              color: kAccentCyanHover,
              borderRadius: BorderRadius.circular(8),
            ),
            child: Text(
              '$n',
              style: wispSans(
                fontSize: 13,
                fontWeight: FontWeight.w700,
                color: kAccentCyanStrong,
              ),
            ),
          ),
          const SizedBox(width: 12),
          Expanded(
            child: Padding(
              padding: const EdgeInsets.only(top: 3),
              child: Text(
                text,
                style: wispSans(fontSize: 13.5, color: kInk, height: 1.4),
              ),
            ),
          ),
        ],
      ),
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
        ? 'Make sure both devices are on the same Wi-Fi, or connected by a USB cable.'
        : 'Make sure both devices are on the same Wi-Fi, or connected by a USB cable. Local network access may be required.';

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
                  style: wispSans(
                    fontSize: 14,
                    fontWeight: FontWeight.w700,
                    color: kInk,
                  ),
                ),
                const SizedBox(height: 4),
                Text(
                  subtitle,
                  style: wispSans(
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

class _RecentDeviceTile extends ConsumerWidget {
  const _RecentDeviceTile({
    required this.device,
    required this.isSelected,
    required this.onTap,
  });

  final SavedDevice device;
  final bool isSelected;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final badgeColor = colorFromPubkey(device.endpointId);
    final name = resolveDeviceName(
      ref,
      endpointId: device.endpointId,
      broadcastLabel: device.label.isEmpty ? 'Saved device' : device.label,
    );
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
              color: isSelected ? kAccentCyan : kMuted,
            ),
            const SizedBox(height: 8),
            Text(
              name.primary,
              style: wispSans(
                fontSize: 12.5,
                fontWeight: FontWeight.w600,
                color: kInk,
                height: 1.18,
              ),
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              textAlign: TextAlign.center,
            ),
            if (name.broadcast != null) ...[
              const SizedBox(height: 2),
              Text(
                name.broadcast!,
                style: wispSans(
                  fontSize: 9.5,
                  fontWeight: FontWeight.w400,
                  color: kMuted,
                  height: 1.1,
                ),
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                textAlign: TextAlign.center,
              ),
            ],
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
                  style: wispSans(
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
              style: wispSans(
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

class _NearbyDeviceTile extends ConsumerWidget {
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
  Widget build(BuildContext context, WidgetRef ref) {
    final name = resolveDeviceName(
      ref,
      endpointId: receiver.endpointId,
      broadcastLabel: receiver.label,
    );
    return AnimatedOpacity(
      duration: const Duration(milliseconds: 300),
      opacity: isStale ? 0.45 : 1.0,
      child: InkWell(
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
              Icon(icon, size: 22, color: isSelected ? kAccentCyan : kMuted),
              const SizedBox(height: 8),
              Text(
                name.primary,
                style: wispSans(
                  fontSize: 12.5,
                  fontWeight: FontWeight.w600,
                  color: kInk,
                  height: 1.18,
                ),
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                textAlign: TextAlign.center,
              ),
              if (name.broadcast != null) ...[
                const SizedBox(height: 2),
                Text(
                  name.broadcast!,
                  style: wispSans(
                    fontSize: 9.5,
                    fontWeight: FontWeight.w400,
                    color: kMuted,
                    height: 1.1,
                  ),
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  textAlign: TextAlign.center,
                ),
              ],
              if (receiver.endpointId.isNotEmpty) ...[
                const SizedBox(height: 4),
                Tooltip(
                  message:
                      'Identity badge (from public key) — same color = same device.',
                  child: Container(
                    padding: const EdgeInsets.symmetric(
                      horizontal: 6,
                      vertical: 2,
                    ),
                    decoration: BoxDecoration(
                      color: colorFromPubkey(
                        receiver.endpointId,
                      ).withValues(alpha: 0.15),
                      borderRadius: BorderRadius.circular(6),
                      border: Border.all(
                        color: colorFromPubkey(
                          receiver.endpointId,
                        ).withValues(alpha: 0.45),
                        width: 0.8,
                      ),
                    ),
                    child: Text(
                      shortPubkey(receiver.endpointId),
                      style: wispSans(
                        fontSize: 11,
                        fontWeight: FontWeight.w700,
                        color: HSLColor.fromColor(
                          colorFromPubkey(receiver.endpointId),
                        ).withLightness(0.32).toColor(),
                        letterSpacing: 0.4,
                      ),
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
              color: isSelected ? kAccentCyan : kMuted,
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
                          style: wispSans(
                            fontSize: 13.5,
                            fontWeight: FontWeight.w700,
                            color: kInk,
                          ),
                        ),
                        TextSpan(
                          text: '  ·  Paired offline',
                          style: wispSans(
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
                        style: wispSans(
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
            const Icon(Icons.qr_code_rounded, size: 18, color: kAccentCyan),
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
