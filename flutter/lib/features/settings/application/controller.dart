import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart' show ThemeMode;
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../settings_providers.dart';
import 'state.dart';
import '../../receive/application/service.dart';
import '../../../platform/desktop_integration.dart';

final settingsControllerProvider =
    NotifierProvider<SettingsController, SettingsState>(SettingsController.new);

class SettingsController extends Notifier<SettingsState> {
  int _saveRequestSerial = 0;

  @override
  SettingsState build() {
    return SettingsState(settings: ref.watch(initialAppSettingsProvider));
  }

  Future<void> saveSettings({
    required String deviceName,
    required String downloadRoot,
    required String serverUrl,
    required bool discoverableByDefault,
    required bool skipClipboardConfirm,
  }) async {
    final repository = ref.read(settingsRepositoryProvider);
    final receiverSource = ref.read(receiverServiceSourceProvider);
    final requestSerial = ++_saveRequestSerial;
    final baseSettings = state.settings;
    final nextSettings = state.settings.copyWith(
      deviceName: _normalizeDeviceName(deviceName, baseSettings),
      downloadRoot: downloadRoot.trim(),
      discoverableByDefault: discoverableByDefault,
      discoveryServerUrl: _normalizeServerUrl(serverUrl),
      skipClipboardConfirm: skipClipboardConfirm,
    );

    debugPrint(
      '[settings] save requested '
      'device="${nextSettings.deviceName}" '
      'downloadRoot="${nextSettings.downloadRoot}" '
      'serverUrl="${nextSettings.discoveryServerUrl ?? ""}" '
      'discoverable=${nextSettings.discoverableByDefault}',
    );
    state = state.copyWith(isSaving: true, clearErrorMessage: true);
    try {
      await repository.save(nextSettings);
      if (!_isLatestSave(requestSerial)) {
        return;
      }
      final identityChanged =
          nextSettings.deviceName != baseSettings.deviceName ||
          nextSettings.downloadRoot != baseSettings.downloadRoot ||
          nextSettings.discoveryServerUrl != baseSettings.discoveryServerUrl;
      String? syncError;
      if (identityChanged) {
        debugPrint(
          '[settings] live receiver update '
          'device="${nextSettings.deviceName}" '
          'downloadRoot="${nextSettings.downloadRoot}" '
          'serverUrl="${nextSettings.discoveryServerUrl ?? ""}"',
        );
        try {
          await receiverSource.updateIdentity(
            deviceName: nextSettings.deviceName,
            downloadRoot: nextSettings.downloadRoot,
            serverUrl: nextSettings.discoveryServerUrl,
          );
          debugPrint('[settings] live receiver update complete');
        } catch (error) {
          syncError = error.toString();
        }
      } else {
        debugPrint('[settings] live receiver unchanged; skipped rebuild');
      }
      if (!_isLatestSave(requestSerial)) {
        return;
      }
      state = state.copyWith(
        settings: nextSettings,
        isSaving: false,
        errorMessage: syncError,
        clearErrorMessage: syncError == null,
      );
    } catch (error) {
      if (!_isLatestSave(requestSerial)) {
        return;
      }
      state = state.copyWith(isSaving: false, errorMessage: error.toString());
    }
  }

  /// Applies and persists the light/dark/system appearance immediately, without
  /// waiting for the Save button. The theme is a pure UI preference (no receiver
  /// identity sync), so it lives outside [saveSettings] and updates live — the
  /// same live-apply pattern used by the update-check and context-menu toggles.
  Future<void> setThemeMode(ThemeMode mode) async {
    if (state.settings.themeMode == mode) return;
    final next = state.settings.copyWith(themeMode: mode);
    state = state.copyWith(settings: next);
    await ref.read(settingsRepositoryProvider).save(next);
  }

  /// Android only. Whether to hold the screen awake for the duration of a
  /// transfer. Live-applied like [setThemeMode]: nothing is registered with the
  /// OS at set time, the flag is only read when a transfer starts, so there is
  /// no external state that could drift from the stored value.
  Future<void> setKeepScreenOnDuringTransfer(bool enabled) async {
    if (state.settings.keepScreenOnDuringTransfer == enabled) return;
    final next = state.settings.copyWith(keepScreenOnDuringTransfer: enabled);
    state = state.copyWith(settings: next);
    await ref.read(settingsRepositoryProvider).save(next);
  }

  /// Desktop only. Turns minimize-to-tray on/off, persists it, and applies the
  /// window/tray behaviour immediately (live-apply, like [setThemeMode]).
  Future<void> setMinimizeToTray(bool enabled) async {
    if (state.settings.minimizeToTray == enabled) return;
    final next = state.settings.copyWith(minimizeToTray: enabled);
    state = state.copyWith(settings: next);
    await ref.read(settingsRepositoryProvider).save(next);
    await DesktopIntegration.instance.applyMinimizeToTray(enabled);
  }

  /// Desktop only. Registers/unregisters OS launch-at-startup, persisting the
  /// resulting real state (the OS is the source of truth, so the stored flag is
  /// reconciled to whatever the OS actually reports back).
  ///
  /// Returns null when the OS accepted the change, or the reason it didn't.
  /// The caller owns telling the user — this is not routed through
  /// [SettingsState.errorMessage], which belongs to [saveSettings] and whose
  /// presence suppresses the Save page's baseline reset.
  Future<String?> setLaunchAtStartup(bool enabled) async {
    // Deliberately no "already at this value" early return: the OS
    // registration is the source of truth and drifts from the stored flag on
    // its own (Task Manager / Settings can disable the entry, and an update or
    // a move leaves it pointing at an executable that no longer exists). If we
    // skipped the apply whenever the stored flag already matched, a user whose
    // registration had been disabled outside the app would tick the toggle,
    // Save, and get no registry write at all — with no way to re-register from
    // the UI. Re-applying is idempotent, so just always do it.
    // Optimistic UI update; corrected below if the OS disagrees.
    state = state.copyWith(
      settings: state.settings.copyWith(launchAtStartup: enabled),
    );
    final result = await DesktopIntegration.instance.applyLaunchAtStartup(
      enabled,
    );
    final next = state.settings.copyWith(launchAtStartup: result.enabled);
    state = state.copyWith(settings: next);
    await ref.read(settingsRepositoryProvider).save(next);
    return result.error;
  }

  /// Desktop only. Records the *real* OS launch-at-startup registration in
  /// state and prefs without touching the registration itself. The settings
  /// page queries the OS when it opens; without writing that answer back here
  /// the stored flag would stay stale forever, and every later comparison
  /// against it (in the UI or in [setLaunchAtStartup]) would be made against a
  /// value the OS had already contradicted.
  Future<void> reconcileLaunchAtStartup(bool actual) async {
    if (state.settings.launchAtStartup == actual) return;
    final next = state.settings.copyWith(launchAtStartup: actual);
    state = state.copyWith(settings: next);
    await ref.read(settingsRepositoryProvider).save(next);
  }

  /// Persists a device name recovered from an identity backup and reflects it
  /// in state. Like the restored secret key, this only fully takes effect after
  /// the relaunch the import screen prompts for — the running receiver keeps
  /// its current name until then — so we persist + update state without the
  /// live receiver rebuild [saveSettings] performs.
  Future<void> applyRestoredDeviceName(String name) async {
    final trimmed = name.trim();
    if (trimmed.isEmpty || trimmed == state.settings.deviceName) return;
    final next = state.settings.copyWith(deviceName: trimmed);
    state = state.copyWith(settings: next);
    await ref.read(settingsRepositoryProvider).save(next);
  }

  bool _isLatestSave(int serial) => serial == _saveRequestSerial;

  String _normalizeDeviceName(String value, AppSettings fallback) {
    final trimmed = value.trim();
    return trimmed.isEmpty ? fallback.deviceName : trimmed;
  }

  String? _normalizeServerUrl(String value) {
    final trimmed = value.trim();
    return trimmed.isEmpty ? null : trimmed;
  }
}
