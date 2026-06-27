import 'package:app/features/saved_devices/application/saved_devices_repository.dart';
import 'package:app/features/update/application/update_repository.dart';
import 'package:app/features/usb_cable/application/usb_cable_controller.dart';
import 'package:app/features/usb_cable/application/usb_link_status.dart';
import 'package:app/src/rust/api/lan.dart' as rust_lan;
import 'package:flutter_test/flutter_test.dart';
import 'package:shared_preferences/shared_preferences.dart';

/// Returns an in-memory [SavedDevicesRepository] backed by mocked
/// SharedPreferences.  Use in test setUp + spread into a `ProviderScope`'s
/// `overrides:`:
///
/// ```dart
/// final repo = await mockSavedDevicesRepo();
/// ProviderScope(
///   overrides: [
///     savedDevicesRepositoryProvider.overrideWithValue(repo),
///     ...
///   ],
///   ...
/// );
/// ```
///
/// Pair with `SharedPreferences.setMockInitialValues({})` in `setUp`.
Future<SavedDevicesRepository> mockSavedDevicesRepo() async {
  final prefs = await SharedPreferences.getInstance();
  return SavedDevicesRepository(prefs: prefs);
}

/// In-memory [UpdateRepository] for tests that pump a screen reading the
/// update-checker preferences (e.g. the Settings page). Spread its override
/// into the `ProviderScope`. Pair with `SharedPreferences.setMockInitialValues`.
Future<UpdateRepository> mockUpdateRepo() async {
  final prefs = await SharedPreferences.getInstance();
  return UpdateRepository(prefs: prefs);
}

/// No-op [UsbCableController] that reports "unsupported" and starts no
/// liveness-poll timer. The real controller's `build()` runs a never-ending
/// `Timer.periodic`, which fails widget tests with "Pending timers" whenever a
/// pumped screen embeds `UsbStatusEntry`.
class _StubUsbCableController extends UsbCableController {
  @override
  UsbCableState build() => UsbCableState.unsupported;
}

/// Overrides for the USB cable + tether providers so any screen embedding
/// `UsbStatusEntry` (the Send/Receive draft screens) doesn't spin up real
/// periodic timers/streams in tests. Spread into a `ProviderScope`/
/// `ProviderContainer`'s `overrides:`.
final List usbTestOverrides = [
  usbCableControllerProvider.overrideWith(_StubUsbCableController.new),
  usbTetherLinkProvider.overrideWith(
    (ref) => Stream<rust_lan.UsbLinkData?>.value(null),
  ),
];

/// Finite-duration replacement for [WidgetTester.pumpAndSettle].
///
/// `pumpAndSettle()` waits for *all* animations to stop. Pages with
/// `TextField`s have an internal cursor-blink `AnimationController.repeat()`
/// once a field is focused (including the auto-focus that fires when a
/// route activates), which never settles. A finite pump completes all
/// finite animations (route push, dialog open, AnimatedSwitcher) without
/// hanging on the cursor blink.
Future<void> pumpFinite(
  WidgetTester tester, {
  Duration duration = const Duration(milliseconds: 500),
}) async {
  await tester.pump();
  await tester.pump(duration);
}
