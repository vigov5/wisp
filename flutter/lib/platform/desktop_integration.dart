import 'dart:async';
import 'dart:io';

import 'package:flutter/foundation.dart';
import 'package:launch_at_startup/launch_at_startup.dart';
import 'package:local_notifier/local_notifier.dart';
import 'package:package_info_plus/package_info_plus.dart';
import 'package:tray_manager/tray_manager.dart';
import 'package:window_manager/window_manager.dart';

/// Desktop-only window/tray/startup integration for Wisp.
///
/// Owns two user preferences that only make sense on desktop:
///  * **Minimize to tray** — the minimize (–) and close (X) buttons hide the
///    window into a system-tray icon instead of minimizing to the taskbar or
///    quitting. The app keeps running (so it can still receive files); the user
///    restores it from the tray icon or quits from the tray menu.
///  * **Launch at startup** — registers Wisp to auto-start when the user logs
///    in (HKCU Run key on Windows, LaunchAgents on macOS, autostart .desktop on
///    Linux — all handled by `launch_at_startup`).
///
/// A single process-wide instance holds the window/tray listeners. All methods
/// are safe no-ops off desktop so callers don't have to platform-guard.
class DesktopIntegration with WindowListener, TrayListener {
  DesktopIntegration._();

  static final DesktopIntegration instance = DesktopIntegration._();

  /// Command-line flag baked into the OS auto-launch entry so a login launch
  /// can be told apart from a manual one. A login launch starts quietly (hidden
  /// in the tray, or minimized); a manual launch shows the window. See
  /// `main.dart`, which reads this off the process args.
  static const String autostartFlag = '--autostart';

  /// True on the three desktop platforms window_manager/tray_manager support.
  static bool get isSupported =>
      !kIsWeb && (Platform.isWindows || Platform.isMacOS || Platform.isLinux);

  bool _initialized = false;
  bool _minimizeToTray = false;
  bool _trayVisible = false;
  // Set just before we intentionally quit so onWindowClose lets the close
  // through instead of re-hiding to the tray.
  bool _quitting = false;
  bool _startupConfigured = false;
  bool _notificationsReady = false;
  // Kept so the previous toast is torn down before the next one shows (each
  // LocalNotification registers a global listener in its constructor).
  LocalNotification? _activeNotification;

  /// Wires the window + tray listeners once and applies the persisted
  /// minimize-to-tray preference. Call once at startup after
  /// `windowManager.ensureInitialized()`.
  Future<void> init({required bool minimizeToTray}) async {
    if (!isSupported || _initialized) return;
    _initialized = true;
    windowManager.addListener(this);
    trayManager.addListener(this);
    await _ensureNotificationsSetup();
    await applyMinimizeToTray(minimizeToTray);
  }

  /// Turns the minimize-to-tray behaviour on or off. When on, the tray icon is
  /// shown and the window's native close is intercepted (prevent-close) so the
  /// X can hide instead of quit. When off, the tray icon is removed and the
  /// window behaves normally.
  Future<void> applyMinimizeToTray(bool enabled) async {
    if (!isSupported) return;
    _minimizeToTray = enabled;
    try {
      await windowManager.setPreventClose(enabled);
    } catch (error) {
      debugPrint('[desktop] setPreventClose failed: $error');
    }
    if (enabled) {
      await _showTray();
    } else {
      await _hideTray();
    }
  }

  /// Enables or disables OS launch-at-startup. Returns the resulting state.
  Future<bool> applyLaunchAtStartup(bool enabled) async {
    if (!isSupported) return false;
    await _ensureStartupConfigured();
    try {
      if (enabled) {
        await launchAtStartup.enable();
      } else {
        await launchAtStartup.disable();
      }
    } catch (error) {
      debugPrint('[desktop] launch-at-startup toggle failed: $error');
    }
    return isLaunchAtStartupEnabled();
  }

  /// The real OS-level launch-at-startup state (the registry/LaunchAgent entry
  /// is the source of truth, so the Settings toggle reconciles against this).
  Future<bool> isLaunchAtStartupEnabled() async {
    if (!isSupported) return false;
    await _ensureStartupConfigured();
    try {
      return await launchAtStartup.isEnabled();
    } catch (error) {
      debugPrint('[desktop] launch-at-startup query failed: $error');
      return false;
    }
  }

  // --- Incoming-transfer notifications --------------------------------------

  /// Shows a native OS toast for an incoming transfer, but only when the window
  /// isn't already focused (if the user is looking at Wisp the in-app confirm
  /// prompt is enough). Clicking the toast body brings the window back to the
  /// front — essential when the window is hidden in the tray or minimized.
  ///
  /// When both [onAccept] and [onDecline] are supplied, the toast also carries
  /// Accept / Decline buttons so the user can respond without opening the
  /// window (accepting also brings the window forward to show progress).
  /// Safe no-op off desktop.
  Future<void> notifyIncomingTransfer({
    required String title,
    required String body,
    VoidCallback? onAccept,
    VoidCallback? onDecline,
  }) async {
    if (!isSupported) return;
    try {
      // A focused, visible window already shows the confirm prompt.
      if (await windowManager.isFocused()) return;
    } catch (_) {
      // If focus can't be queried, err on the side of notifying.
    }
    await _ensureNotificationsSetup();
    if (!_notificationsReady) return;
    try {
      // Tear down the prior toast so its listener doesn't leak.
      await _activeNotification?.destroy();
      final hasActions = onAccept != null && onDecline != null;
      final notification = LocalNotification(
        title: title,
        body: body,
        actions: hasActions
            ? [
                LocalNotificationAction(text: 'Accept'),
                LocalNotificationAction(text: 'Decline'),
              ]
            : null,
      );
      notification.onClick = () => unawaited(_restoreWindow());
      if (hasActions) {
        notification.onClickAction = (index) {
          // Action order mirrors the `actions` list above: 0 = Accept, which
          // also restores the window so the user sees transfer progress;
          // 1 = Decline, which leaves the window as-is.
          if (index == 0) {
            onAccept();
            unawaited(_restoreWindow());
          } else if (index == 1) {
            onDecline();
          }
        };
      }
      _activeNotification = notification;
      await notification.show();
    } catch (error) {
      debugPrint('[desktop] notify failed: $error');
    }
  }

  Future<void> _ensureNotificationsSetup() async {
    if (_notificationsReady) return;
    try {
      await localNotifier.setup(appName: 'Wisp');
      _notificationsReady = true;
    } catch (error) {
      debugPrint('[desktop] notification setup failed: $error');
    }
  }

  // --- Tray lifecycle -------------------------------------------------------

  Future<void> _showTray() async {
    if (_trayVisible) return;
    try {
      await trayManager.setIcon(_trayIconPath());
      await trayManager.setToolTip('Wisp');
      await trayManager.setContextMenu(_buildTrayMenu());
      _trayVisible = true;
    } catch (error) {
      debugPrint('[desktop] tray setup failed: $error');
    }
  }

  Future<void> _hideTray() async {
    if (!_trayVisible) return;
    try {
      await trayManager.destroy();
    } catch (error) {
      debugPrint('[desktop] tray destroy failed: $error');
    }
    _trayVisible = false;
  }

  Menu _buildTrayMenu() {
    return Menu(
      items: [
        MenuItem(key: 'show', label: 'Show Wisp'),
        MenuItem.separator(),
        MenuItem(key: 'quit', label: 'Quit'),
      ],
    );
  }

  // Windows needs an .ico; macOS/Linux take a PNG. Paths are asset keys
  // resolved by tray_manager against the bundled flutter_assets.
  String _trayIconPath() {
    if (Platform.isWindows) return 'assets/tray_icon.ico';
    return 'assets/wisp_square_logo.png';
  }

  // --- Window show / hide / quit -------------------------------------------

  /// Brings the window back to the front. Used when the Windows "Send via Wisp"
  /// menu forwards a path to the already-running instance: the draft is opened
  /// in Dart, but the window may be minimized to the taskbar or hidden in the
  /// tray, so surface it here through window_manager (keeping its tracked state
  /// in sync). Safe no-op off desktop.
  Future<void> bringToFront() async {
    if (!isSupported) return;
    await _restoreWindow();
  }

  Future<void> _hideToTray() async {
    await _showTray();
    try {
      await windowManager.hide();
    } catch (error) {
      debugPrint('[desktop] window hide failed: $error');
    }
  }

  Future<void> _restoreWindow() async {
    try {
      await windowManager.show();
      // The window may have been minimized before it was hidden (minimize
      // path), so un-minimize before focusing or it comes back minimized.
      if (await windowManager.isMinimized()) {
        await windowManager.restore();
      }
      await windowManager.focus();
    } catch (error) {
      debugPrint('[desktop] window restore failed: $error');
    }
  }

  Future<void> _quit() async {
    _quitting = true;
    await _hideTray();
    try {
      await windowManager.setPreventClose(false);
      await windowManager.destroy();
    } catch (error) {
      debugPrint('[desktop] window destroy failed: $error');
      _quitting = false;
    }
  }

  Future<void> _ensureStartupConfigured() async {
    if (_startupConfigured) return;
    final info = await PackageInfo.fromPlatform();
    launchAtStartup.setup(
      appName: info.appName,
      appPath: Platform.resolvedExecutable,
      packageName: info.packageName,
      // The marker flag lets a login launch start hidden (see main.dart).
      args: const [autostartFlag],
    );
    _startupConfigured = true;
  }

  // --- WindowListener -------------------------------------------------------

  @override
  void onWindowClose() {
    if (_minimizeToTray && !_quitting) {
      unawaited(_hideToTray());
    }
  }

  @override
  void onWindowMinimize() {
    if (_minimizeToTray) {
      unawaited(_hideToTray());
    }
  }

  // --- TrayListener ---------------------------------------------------------

  @override
  void onTrayIconMouseDown() {
    unawaited(_restoreWindow());
  }

  @override
  void onTrayIconRightMouseDown() {
    unawaited(trayManager.popUpContextMenu());
  }

  @override
  void onTrayMenuItemClick(MenuItem menuItem) {
    switch (menuItem.key) {
      case 'show':
        unawaited(_restoreWindow());
      case 'quit':
        unawaited(_quit());
    }
  }
}
