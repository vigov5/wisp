import 'package:app/features/saved_devices/application/saved_devices_repository.dart';
import 'package:app/features/update/application/update_repository.dart';
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
