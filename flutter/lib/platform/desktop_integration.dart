import 'dart:async';
import 'dart:io';

import 'package:flutter/foundation.dart';
import 'package:launch_at_startup/launch_at_startup.dart';
import 'package:local_notifier/local_notifier.dart';
import 'package:package_info_plus/package_info_plus.dart';
import 'package:tray_manager/tray_manager.dart';
import 'package:url_launcher/url_launcher.dart';
import 'package:win32_registry/win32_registry.dart';
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
  // Separate from _activeNotification so a user-fired test toast never
  // interferes with (or gets torn down by) a real incoming-transfer toast.
  LocalNotification? _testNotification;

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

  /// Enables or disables OS launch-at-startup.
  ///
  /// Returns the state the OS actually ends up in, plus the failure reason when
  /// the registration could not be written (a locked-down registry, a denied
  /// LaunchAgent). Callers must surface a non-null `error`: without it the
  /// toggle just springs back to its old position and the user is left to guess
  /// why their choice didn't stick.
  Future<({bool enabled, String? error})> applyLaunchAtStartup(
    bool enabled,
  ) async {
    if (!isSupported) return (enabled: false, error: null);
    await _ensureStartupConfigured();
    String? error;
    try {
      if (enabled) {
        await launchAtStartup.enable();
      } else {
        await launchAtStartup.disable();
      }
    } catch (failure) {
      debugPrint('[desktop] launch-at-startup toggle failed: $failure');
      error = failure.toString();
    }
    final actual = await isLaunchAtStartupEnabled();
    // A silent no-op counts as a failure too: `enable()` can return without
    // throwing and still leave nothing the OS will act on.
    if (error == null && actual != enabled) {
      error = enabled
          ? 'the registration was not accepted by the system'
          : 'the registration could not be removed';
    }
    return (enabled: actual, error: error);
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

  /// Tidies away the on-screen incoming-transfer toast once the offer has been
  /// settled in-app, so a stale banner doesn't linger. This is best-effort: a
  /// toast the OS surfaces late (or parks in the Action Center) may outlive it,
  /// which is harmless because the toast's Accept/Decline are guarded against a
  /// no-longer-pending offer at the callback (see app.dart). Safe no-op off
  /// desktop.
  Future<void> dismissIncomingTransfer() async {
    if (!isSupported) return;
    final notification = _activeNotification;
    _activeNotification = null;
    if (notification == null) return;
    try {
      await notification.destroy();
    } catch (error) {
      debugPrint('[desktop] notify dismiss failed: $error');
    }
  }

  /// Fires a sample OS toast so the user can confirm notifications actually
  /// reach them. On Windows this doubles as registration: the OS only lists an
  /// app under Settings → Notifications once it has shown a toast, and the
  /// first toast after a fresh install can be dropped while the app's
  /// AppUserModelID shortcut is still being registered — so firing one here
  /// both verifies and "primes" delivery for the real incoming-transfer toast.
  ///
  /// Returns true if the toast was handed to the OS without error (which is not
  /// a guarantee it was displayed — the user may have muted the app or Focus
  /// Assist / Do Not Disturb may be swallowing it). Safe no-op off desktop.
  Future<bool> sendTestNotification() async {
    if (!isSupported) return false;
    await _ensureNotificationsSetup();
    if (!_notificationsReady) return false;
    try {
      await _testNotification?.destroy();
      final notification = LocalNotification(
        title: 'Notifications are working',
        body:
            "This is a test. You'll get an alert like this when a transfer "
            'arrives while Wisp is in the background.',
      );
      notification.onClick = () => unawaited(_restoreWindow());
      _testNotification = notification;
      await notification.show();
      return true;
    } catch (error) {
      debugPrint('[desktop] test notification failed: $error');
      return false;
    }
  }

  /// Opens the OS's notification settings so the user can (re-)enable Wisp's
  /// alerts. Windows deep-links straight to the notifications page; macOS opens
  /// the Notifications preference pane. Returns false where there's no usable
  /// deep link (e.g. most Linux desktops) or the launch fails. Safe no-op off
  /// desktop.
  Future<bool> openNotificationSettings() async {
    if (!isSupported) return false;
    final String? target = Platform.isWindows
        ? 'ms-settings:notifications'
        : Platform.isMacOS
        ? 'x-apple.systempreferences:com.apple.preference.notifications'
        : null;
    if (target == null) return false;
    try {
      return await launchUrl(Uri.parse(target));
    } catch (error) {
      debugPrint('[desktop] open notification settings failed: $error');
      return false;
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
    final exePath = Platform.resolvedExecutable;
    // On the plain-registry Windows path, launch_at_startup joins the path and
    // the args into one unquoted string ("C:\Program Files\Wisp\Wisp.exe
    // --autostart"), leaving whoever reads it to guess where the path ends.
    // CreateProcess does eventually find the exe by trying each space-delimited
    // prefix, but Task Manager's and Settings' Startup lists take only the
    // first token — so an install under "Program Files" shows up there as a
    // nameless entry with a blank icon, which reads as broken. Hand the plugin
    // a pre-quoted path so every reader agrees on it.
    final bool msix = Platform.isWindows && _isMsixBuild(info.packageName);
    launchAtStartup.setup(
      appName: info.appName,
      // The MSIX branch writes the path into a shortcut's TargetPath, which
      // takes a bare path — only the registry branch wants quotes.
      appPath: Platform.isWindows && !msix ? '"$exePath"' : exePath,
      packageName: info.packageName,
      // The marker flag lets a login launch start hidden (see main.dart).
      args: const [autostartFlag],
    );
    if (Platform.isWindows && !msix) {
      _migrateUnquotedRunEntry(appName: info.appName, exePath: exePath);
    }
    _startupConfigured = true;
  }

  /// Mirrors launch_at_startup's own MSIX detection (`isRunningInMsix`, which
  /// the package doesn't export): a packaged build runs out of
  /// `WindowsApps\<packageName>...`. Kept in step with the package so we quote
  /// exactly the path it writes into the registry, and no other.
  static bool _isMsixBuild(String packageName) {
    final exePath = Platform.resolvedExecutable;
    return exePath.contains('WindowsApps') && exePath.contains(packageName);
  }

  /// Rewrites a Run entry written before the quoting above into the quoted
  /// form, for this same executable.
  ///
  /// [isLaunchAtStartupEnabled] asks launch_at_startup, which compares the
  /// stored string against the one it would write today. Adding the quotes
  /// changes that string, so without this an existing registration would stop
  /// being recognised on the first launch after an update: the user's toggle
  /// would silently read as off while the (still working) entry stayed behind
  /// in the registry.
  ///
  /// Only the Run value is touched. `StartupApproved\Run` — where Task Manager
  /// records whether the entry is allowed to run — is deliberately left alone,
  /// so an entry the user disabled there stays disabled.
  void _migrateUnquotedRunEntry({
    required String appName,
    required String exePath,
  }) {
    RegistryKey? key;
    try {
      key = Registry.openPath(
        RegistryHive.currentUser,
        path: r'Software\Microsoft\Windows\CurrentVersion\Run',
        desiredAccessRights: AccessRights.allAccess,
      );
      final current = key.getStringValue(appName);
      if (current != '$exePath $autostartFlag') return;
      key.createValue(
        RegistryValue.string(appName, '"$exePath" $autostartFlag'),
      );
      debugPrint('[desktop] quoted the legacy launch-at-startup Run entry');
    } catch (error) {
      // Nothing to recover: the toggle reads as off and re-ticking it writes a
      // fresh (quoted) entry.
      debugPrint('[desktop] launch-at-startup migration failed: $error');
    } finally {
      key?.close();
    }
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
