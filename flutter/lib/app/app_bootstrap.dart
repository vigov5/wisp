import 'dart:io';

import 'package:flutter/foundation.dart';
import 'package:path_provider/path_provider.dart';
import 'package:shared_preferences/shared_preferences.dart';

import '../features/receive/application/pairing_cache.dart';
import '../features/saved_devices/application/saved_devices_repository.dart';
import '../features/settings/application/repository.dart';
import '../features/settings/application/state.dart';
import '../platform/android_media_store.dart';
import '../platform/identity_storage.dart';
import '../platform/rust/receiver/rust_source.dart';
import '../src/rust/api/device.dart' as rust_device;
import '../src/rust/api/simple.dart' as rust_simple;

class AppBootstrap {
  const AppBootstrap({
    required this.settingsRepository,
    required this.initialSettings,
    required this.receiverSource,
    required this.savedDevicesRepository,
  });

  final SettingsRepository settingsRepository;
  final AppSettings initialSettings;
  final RustReceiverServiceSource receiverSource;
  final SavedDevicesRepository savedDevicesRepository;
}

Future<AppBootstrap> loadAppBootstrap({
  String Function()? randomDeviceName,
  String? defaultDownloadRoot,
  void Function(List<int> secretKeyBytes)? installAppIdentity,
}) async {
  final prefs = await SharedPreferences.getInstance();

  // On Android, Rust writes to a temp cache; files are moved to the public
  // Downloads/Drift/ folder via MediaStore after each transfer completes.
  // The user-configured downloadRoot is ignored on Android.
  String? androidReceiveCacheDir;
  if (Platform.isAndroid) {
    final tmpDir = await getTemporaryDirectory();
    androidReceiveCacheDir = '${tmpDir.path}/Download/Drift';
  }

  final repository = SettingsRepository(
    prefs: prefs,
    randomDeviceName: randomDeviceName ?? rust_device.randomDeviceName,
    defaultDownloadRoot:
        defaultDownloadRoot ?? await resolvePreferredReceiveDownloadRoot(),
  );
  final initialSettings = await repository.loadOrCreate();
  final pairingCache = PairingCacheRepository(prefs: prefs);

  // First-install hardening for Android. The Rust receiver's `create_dir_all`
  // runs at its own startup, but the receiver only starts when the UI opens
  // the Receive tab — by which time the sender may already be dialling.
  // Creating both the Rust-side cache and the user-facing default upfront
  // means a first-launch user can accept a fresh transfer immediately
  // without racing with lazy mkdir (one suspected cause of "sender stuck on
  // Waiting, receiver never shows the confirm screen" on a fresh install).
  if (Platform.isAndroid && androidReceiveCacheDir != null) {
    await _ensureDirExists(androidReceiveCacheDir);
    if (!AndroidMediaStore.isSafUri(initialSettings.downloadRoot)) {
      await _ensureDirExists(initialSettings.downloadRoot);
    }
  }

  // Install the persistent app identity so iroh sees a stable EndpointId
  // across launches. Must run before any sender/receiver session starts.
  // The installer hook is overridable so unit tests can stub it without
  // initializing the native bridge.
  final identity = IdentityStorage(prefs: prefs);
  final secretKey = await identity.loadOrCreate();
  final install =
      installAppIdentity ??
      ((bytes) => rust_simple.setAppIdentity(secretKeyBytes: bytes));
  install(secretKey);

  return AppBootstrap(
    settingsRepository: repository,
    initialSettings: initialSettings,
    savedDevicesRepository: SavedDevicesRepository(prefs: prefs),
    receiverSource:
        RustReceiverServiceSource(
            deviceName: initialSettings.deviceName,
            downloadRoot:
                androidReceiveCacheDir ?? initialSettings.downloadRoot,
            serverUrl: initialSettings.discoveryServerUrl,
            androidReceiveCacheDir: androidReceiveCacheDir,
            pairingCache: pairingCache,
          )
          ..androidSaveUri =
              (androidReceiveCacheDir != null &&
                  AndroidMediaStore.isSafUri(initialSettings.downloadRoot))
              ? initialSettings.downloadRoot
              : null,
  );
}

Future<String> resolvePreferredReceiveDownloadRoot() async {
  if (Platform.isAndroid) {
    final downloadsDir = await getDownloadsDirectory();
    if (downloadsDir != null) {
      return '${downloadsDir.path}${Platform.pathSeparator}Drift';
    }
    final externalDirs = await getExternalStorageDirectories(
      type: StorageDirectory.downloads,
    );
    final externalDir = externalDirs != null && externalDirs.isNotEmpty
        ? externalDirs.first
        : null;
    if (externalDir != null) {
      return '${externalDir.path}${Platform.pathSeparator}Drift';
    }
    final docsDir = await getApplicationDocumentsDirectory();
    return '${docsDir.path}${Platform.pathSeparator}Drift';
  }

  if (Platform.isIOS) {
    final docsDir = await getApplicationDocumentsDirectory();
    return '${docsDir.path}${Platform.pathSeparator}Drift';
  }

  final downloadsDir = await getDownloadsDirectory();
  if (downloadsDir != null) {
    return '${downloadsDir.path}${Platform.pathSeparator}Drift';
  }

  final home = _userHomeDirectory();
  if (home != null && home.isNotEmpty) {
    return '$home${Platform.pathSeparator}Downloads${Platform.pathSeparator}Drift';
  }

  return '${Directory.systemTemp.path}${Platform.pathSeparator}Drift';
}

String? _userHomeDirectory() {
  if (Platform.isWindows) {
    return Platform.environment['USERPROFILE'];
  }
  return Platform.environment['HOME'];
}

Future<void> _ensureDirExists(String path) async {
  try {
    await Directory(path).create(recursive: true);
  } catch (error) {
    debugPrint('[bootstrap] could not create download dir $path: $error');
  }
}
