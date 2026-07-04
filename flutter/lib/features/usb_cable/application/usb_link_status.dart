import 'dart:async';

import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../../platform/android/usb_tether_channel.dart';
import '../../../src/rust/api/lan.dart' as rust_lan;
import 'usb_cable_controller.dart';

/// Cheap, synchronous interface enumeration in Rust. Guarded so a platform
/// without the bridge (e.g. web / widget tests) reports "no cable" rather than
/// throwing.
rust_lan.UsbLinkData? detectUsbTetherLink() {
  try {
    return rust_lan.detectUsbLink();
  } catch (_) {
    return null;
  }
}

/// True when [link] is a real phone↔computer USB-tether link, not the
/// point-to-point AOA tunnel (10.42.0.0/30). `detectUsbLink()` also matches the
/// AOA tunnel addresses, so the tether mode must exclude them — that link is
/// owned by [usbCableControllerProvider], not by tethering.
bool isTetherLink(rust_lan.UsbLinkData? link) =>
    link != null && !link.localIp.startsWith('10.42.0.');

/// Polls [detectUsbTetherLink] so the USB setup page + home icon can react to a
/// tether cable coming up/going down without each widget owning a timer. Emits
/// the current link (or null) immediately, then every 2s.
final usbTetherLinkProvider = StreamProvider<rust_lan.UsbLinkData?>((
  ref,
) async* {
  yield detectUsbTetherLink();
  yield* Stream<rust_lan.UsbLinkData?>.periodic(
    const Duration(seconds: 2),
    (_) => detectUsbTetherLink(),
  );
});

/// Whether a USB cable is physically plugged in, independent of tethering.
/// Lets the tether checklist tick "Connect the cable" before the user turns
/// tethering on. Emits immediately, then polls every 2s. False off-Android.
final usbCablePluggedProvider = StreamProvider<bool>((ref) async* {
  yield await UsbTether.isCableConnected();
  yield* Stream<Future<bool>>.periodic(
    const Duration(seconds: 2),
    (_) => UsbTether.isCableConnected(),
  ).asyncMap((f) => f);
});

/// Whether *any* USB link is up — either the direct phone↔phone AOA tunnel or a
/// phone↔computer tether. Drives the lit/unlit state of the home USB icon and
/// the compact status entries on the Send/Receive screens.
final usbConnectedProvider = Provider<bool>((ref) {
  final aoaUp = ref.watch(usbCableControllerProvider.select((s) => s.tunnelUp));
  final tether = ref.watch(usbTetherLinkProvider).value;
  return aoaUp || isTetherLink(tether);
});
