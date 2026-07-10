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
          launchAtStartup == other.launchAtStartup;

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
