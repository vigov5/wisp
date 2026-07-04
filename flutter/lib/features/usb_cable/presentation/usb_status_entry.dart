import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../../app/app_router.dart';
import '../../../src/rust/api/lan.dart' as rust_lan;
import '../../../theme/wisp_theme.dart';
import '../application/usb_cable_controller.dart';
import '../application/usb_link_status.dart';

/// Compact, tappable USB status row for the Send/Receive screens. Surfaces the
/// current link state at a glance and routes to the full [UsbSetupPage] for
/// setup — keeping the heavy step UI off the transfer screens while still
/// letting either side confirm the cable is up before sending. Android-only;
/// renders nothing where the direct link isn't supported and no tether is up.
class UsbStatusEntry extends ConsumerWidget {
  const UsbStatusEntry({super.key});

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final cable = ref.watch(usbCableControllerProvider);
    final tether = ref.watch(usbTetherLinkProvider).value;
    final tetherUp = isTetherLink(tether);

    // Nothing to show on platforms without the direct link and no tether.
    if (!cable.supported && !tetherUp) return const SizedBox.shrink();

    final (icon, title, subtitle, color) = _describe(cable, tether, tetherUp);

    return InkWell(
      onTap: () => context.pushUsbSetup(),
      borderRadius: BorderRadius.circular(16),
      child: Container(
        padding: const EdgeInsets.symmetric(horizontal: 14, vertical: 12),
        decoration: BoxDecoration(
          color: context.wc.surface,
          borderRadius: BorderRadius.circular(16),
          border: Border.all(
            color: color == kAccentDirect
                ? kAccentDirect.withValues(alpha: 0.45)
                : context.wc.border.withValues(alpha: 0.6),
          ),
        ),
        child: Row(
          children: [
            Icon(icon, size: 20, color: color),
            const SizedBox(width: 12),
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(
                    title,
                    style: wispSans(
                      fontSize: 13.5,
                      fontWeight: FontWeight.w600,
                      color: context.wc.ink,
                    ),
                  ),
                  const SizedBox(height: 2),
                  Text(
                    subtitle,
                    style: wispSans(
                      fontSize: 11.5,
                      color: context.wc.muted,
                      height: 1.3,
                    ),
                  ),
                ],
              ),
            ),
            Icon(
              Icons.chevron_right_rounded,
              size: 20,
              color: context.wc.muted,
            ),
          ],
        ),
      ),
    );
  }

  (IconData, String, String, Color) _describe(
    UsbCableState cable,
    rust_lan.UsbLinkData? tether,
    bool tetherUp,
  ) {
    if (cable.tunnelUp) {
      return (
        Icons.usb_rounded,
        'USB cable connected',
        'Phone-to-phone link is up${cable.localIp != null ? ' · ${cable.localIp}' : ''}',
        kAccentDirect,
      );
    }
    if (cable.phase == UsbCablePhase.connecting ||
        cable.phase == UsbCablePhase.linkUp) {
      return (
        Icons.usb_rounded,
        'Connecting over USB…',
        'Setting up the phone-to-phone link.',
        kAccentCyan,
      );
    }
    if (cable.phase == UsbCablePhase.detected) {
      return (
        Icons.usb_rounded,
        'USB cable detected',
        'Tap to set up — press Start on both phones.',
        kAccentCyan,
      );
    }
    if (tetherUp) {
      return (
        Icons.laptop_mac_rounded,
        'USB cable connected',
        'USB tethering active${tether != null ? ' · ${tether.localIp}' : ''}',
        kAccentDirect,
      );
    }
    return (
      Icons.cable_rounded,
      'USB transfer',
      'Tap to set up a direct cable link.',
      kMuted,
    );
  }
}
