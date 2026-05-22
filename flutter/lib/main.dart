import 'dart:io';

import 'package:app/app/app_bootstrap.dart';
import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:window_manager/window_manager.dart';

import 'app/app.dart';
import 'features/receive/feature.dart';
import 'features/saved_devices/application/saved_devices_controller.dart';
import 'features/transfers/feature.dart';
import 'features/settings/settings_providers.dart';
import 'src/rust/frb_generated.dart';

Future<void> main() async {
  WidgetsFlutterBinding.ensureInitialized();
  if (Platform.isMacOS || Platform.isWindows || Platform.isLinux) {
    await windowManager.ensureInitialized();
    await windowManager.setTitleBarStyle(TitleBarStyle.hidden);
  }
  await RustLib.init();

  const initialSize = Size(440, 840);
  final bootstrap = await loadAppBootstrap();
  runApp(
    ProviderScope(
      overrides: [
        settingsRepositoryProvider.overrideWithValue(
          bootstrap.settingsRepository,
        ),
        initialAppSettingsProvider.overrideWithValue(bootstrap.initialSettings),
        receiverServiceSourceProvider.overrideWithValue(
          bootstrap.receiverSource,
        ),
        transfersServiceSourceProvider.overrideWithValue(
          bootstrap.receiverSource,
        ),
        savedDevicesRepositoryProvider.overrideWithValue(
          bootstrap.savedDevicesRepository,
        ),
      ],
      child: const WispApp(),
    ),
  );
  if (Platform.isMacOS || Platform.isWindows || Platform.isLinux) {
    await windowManager.waitUntilReadyToShow(
      WindowOptions(
        size: initialSize,
        minimumSize: initialSize,
        maximumSize: initialSize,
        center: true,
        title: 'Wisp',
      ),
      () async {
        await windowManager.show();
      },
    );
  }
}
