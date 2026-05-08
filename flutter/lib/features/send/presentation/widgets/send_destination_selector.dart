import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../../../theme/drift_theme.dart';
import '../../../receive/application/service.dart';
import '../../../receive/application/state.dart';
import '../../application/controller.dart';
import '../../application/model.dart';
import '../../application/state.dart';
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

    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
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
            height: 110,
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
              const SizedBox(height: 10),
              Text(
                receiver.label,
                style: driftSans(
                  fontSize: 12.5,
                  fontWeight: FontWeight.w600,
                  color: kInk,
                  height: 1.18,
                ),
                maxLines: 2,
                overflow: TextOverflow.ellipsis,
                textAlign: TextAlign.center,
              ),
            ],
          ),
        ),
      ),
    );
  }
}
