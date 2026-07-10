import 'dart:io';

import 'package:app/app/app_bootstrap.dart';
import 'package:flutter/foundation.dart'
    show LicenseEntryWithLineBreaks, LicenseRegistry;
import 'package:flutter/material.dart';
import 'package:flutter/services.dart' show rootBundle;
import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:window_manager/window_manager.dart';

import 'app/app.dart';
import 'features/identity/identity_providers.dart';
import 'features/receive/feature.dart';
import 'features/saved_devices/application/saved_devices_controller.dart';
import 'features/transfers/feature.dart';
import 'features/settings/settings_providers.dart';
import 'features/update/application/update_providers.dart';
import 'platform/desktop_integration.dart';
import 'src/rust/frb_generated.dart';

// [args] carries the process launch arguments. On Windows the native runner
// forwards a "Send via Wisp" file/folder path here on cold start (warm-start
// paths arrive via the windows_integration method channel instead).
Future<void> main(List<String> args) async {
  WidgetsFlutterBinding.ensureInitialized();

  // A login launch carries the auto-start marker flag (registered by
  // DesktopIntegration). It starts quietly so it doesn't pop a window in the
  // user's face; a manual launch shows the window normally. File/folder paths
  // forwarded by the Windows "Send via Wisp" menu never start with '--', so
  // stripping our own flags leaves the send paths intact.
  final bool launchedAtStartup = args.contains(
    DesktopIntegration.autostartFlag,
  );
  final List<String> sendPaths = args
      .where((arg) => !arg.startsWith('--'))
      .toList(growable: false);

  // Register the SIL OFL 1.1 text for the bundled Noto fonts so it appears in
  // the standard Flutter licenses page (showLicensePage / AboutDialog). Both
  // Noto Sans and Noto Sans Mono are covered by the same OFL from the Noto
  // Project, so one entry lists both families.
  LicenseRegistry.addLicense(() async* {
    final license = await rootBundle.loadString('assets/fonts/OFL.txt');
    yield LicenseEntryWithLineBreaks(const [
      'Noto Sans',
      'Noto Sans Mono',
    ], license);
  });

  if (Platform.isMacOS || Platform.isWindows || Platform.isLinux) {
    await windowManager.ensureInitialized();
    await windowManager.setTitleBarStyle(TitleBarStyle.hidden);
  }
  await RustLib.init();

  const initialSize = Size(440, 840);
  final bootstrap = await loadAppBootstrap();
  final minimizeToTray = bootstrap.initialSettings.minimizeToTray;
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
        updateRepositoryProvider.overrideWithValue(bootstrap.updateRepository),
        identityStorageProvider.overrideWithValue(bootstrap.identityStorage),
      ],
      child: WispApp(initialSendPaths: sendPaths),
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
        if (launchedAtStartup) {
          // Started with the OS. Stay out of the way: when minimize-to-tray is
          // on, leave the window hidden (the tray icon, set up by init() just
          // below, is the entry point); otherwise minimize to the taskbar so
          // it's still reachable without stealing focus.
          if (!minimizeToTray) {
            await windowManager.minimize();
          }
          return;
        }
        await windowManager.show();
      },
    );
    // Wire the tray + window-close interception and apply the persisted
    // minimize-to-tray preference. Kept off the critical path (guarded) so a
    // tray/plugin failure never blocks a usable window.
    if (DesktopIntegration.isSupported) {
      await DesktopIntegration.instance.init(minimizeToTray: minimizeToTray);
    }
  }
}
