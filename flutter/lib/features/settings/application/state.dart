import 'package:flutter/material.dart' show ThemeMode;
import 'package:flutter/foundation.dart';

@immutable
class AppSettings {
  const AppSettings({
    required this.deviceName,
    required this.downloadRoot,
    required this.discoverableByDefault,
    required this.discoveryServerUrl,
    this.skipClipboardConfirm = false,
    this.themeMode = ThemeMode.system,
    this.minimizeToTray = false,
    this.launchAtStartup = false,
    this.keepScreenOnDuringTransfer = true,
  });

  final String deviceName;
  final String downloadRoot;
  final bool discoverableByDefault;
  final String? discoveryServerUrl;

  /// When true, "Share clipboard" skips the confirm/edit screen and jumps
  /// straight to device selection. Default false (always confirm first).
  final bool skipClipboardConfirm;

  /// Light / dark / follow-the-system appearance. Defaults to
  /// [ThemeMode.system] so a fresh install matches the OS.
  final ThemeMode themeMode;

  /// Desktop only. When true, the minimize/close buttons hide Wisp to the
  /// system tray instead of minimizing to the taskbar / quitting. Default off.
  final bool minimizeToTray;

  /// Desktop only. When true, Wisp is registered to auto-start when the user
  /// logs in. Mirrors the OS-level state (reconciled at Settings open).
  final bool launchAtStartup;

  /// Android only. Holds the screen awake for the duration of a transfer.
  ///
  /// Defaults **on**, because letting the screen sleep is not a cosmetic
  /// choice: it drops Wi-Fi into power-save. Measured on the test rig with
  /// plain TCP (no Wisp code in the path), 4 GiB over one link: 59-62 MiB/s
  /// with the screen on, 13-18 MiB/s with it off, and ~40 s to climb back
  /// after it wakes. That recovery lag is why the symptom is confusing -
  /// turning the screen on *to read the speed* still shows the low number.
  ///
  /// Neither the Wi-Fi lock nor the partial wake lock avoids this: the
  /// platform only lifts the restriction while the screen is on, so keeping
  /// it awake is the one lever an app actually has. Costs battery, hence the
  /// switch.
  final bool keepScreenOnDuringTransfer;

  AppSettings copyWith({
    String? deviceName,
    String? downloadRoot,
    bool? discoverableByDefault,
    String? discoveryServerUrl,
    bool clearDiscoveryServerUrl = false,
    bool? skipClipboardConfirm,
    ThemeMode? themeMode,
    bool? minimizeToTray,
    bool? launchAtStartup,
    bool? keepScreenOnDuringTransfer,
  }) {
    return AppSettings(
      deviceName: deviceName ?? this.deviceName,
      downloadRoot: downloadRoot ?? this.downloadRoot,
      discoverableByDefault:
          discoverableByDefault ?? this.discoverableByDefault,
      discoveryServerUrl: clearDiscoveryServerUrl
          ? null
          : (discoveryServerUrl ?? this.discoveryServerUrl),
      skipClipboardConfirm: skipClipboardConfirm ?? this.skipClipboardConfirm,
      themeMode: themeMode ?? this.themeMode,
      minimizeToTray: minimizeToTray ?? this.minimizeToTray,
      launchAtStartup: launchAtStartup ?? this.launchAtStartup,
      keepScreenOnDuringTransfer:
          keepScreenOnDuringTransfer ?? this.keepScreenOnDuringTransfer,
    );
  }

  @override
  bool operator ==(Object other) =>
      identical(this, other) ||
      other is AppSettings &&
          runtimeType == other.runtimeType &&
          deviceName == other.deviceName &&
          downloadRoot == other.downloadRoot &&
          discoverableByDefault == other.discoverableByDefault &&
          discoveryServerUrl == other.discoveryServerUrl &&
          skipClipboardConfirm == other.skipClipboardConfirm &&
          themeMode == other.themeMode &&
          minimizeToTray == other.minimizeToTray &&
          launchAtStartup == other.launchAtStartup &&
          keepScreenOnDuringTransfer == other.keepScreenOnDuringTransfer;

  @override
  int get hashCode => Object.hash(
    deviceName,
    downloadRoot,
    discoverableByDefault,
    discoveryServerUrl,
    skipClipboardConfirm,
    themeMode,
    minimizeToTray,
    launchAtStartup,
    keepScreenOnDuringTransfer,
  );
}

@immutable
class SettingsState {
  const SettingsState({
    required this.settings,
    this.isSaving = false,
    this.errorMessage,
  });

  final AppSettings settings;
  final bool isSaving;
  final String? errorMessage;

  SettingsState copyWith({
    AppSettings? settings,
    bool? isSaving,
    String? errorMessage,
    bool clearErrorMessage = false,
  }) {
    return SettingsState(
      settings: settings ?? this.settings,
      isSaving: isSaving ?? this.isSaving,
      errorMessage: clearErrorMessage
          ? null
          : (errorMessage ?? this.errorMessage),
    );
  }
}
