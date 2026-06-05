import 'package:flutter/foundation.dart';

@immutable
class AppSettings {
  const AppSettings({
    required this.deviceName,
    required this.downloadRoot,
    required this.discoverableByDefault,
    required this.discoveryServerUrl,
    this.skipClipboardConfirm = false,
  });

  final String deviceName;
  final String downloadRoot;
  final bool discoverableByDefault;
  final String? discoveryServerUrl;

  /// When true, "Share clipboard" skips the confirm/edit screen and jumps
  /// straight to device selection. Default false (always confirm first).
  final bool skipClipboardConfirm;

  AppSettings copyWith({
    String? deviceName,
    String? downloadRoot,
    bool? discoverableByDefault,
    String? discoveryServerUrl,
    bool clearDiscoveryServerUrl = false,
    bool? skipClipboardConfirm,
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
          skipClipboardConfirm == other.skipClipboardConfirm;

  @override
  int get hashCode => Object.hash(
    deviceName,
    downloadRoot,
    discoverableByDefault,
    discoveryServerUrl,
    skipClipboardConfirm,
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
