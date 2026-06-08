import 'dart:async';
import 'dart:io';

import 'package:flutter/services.dart';

/// Bridges the Windows shell context-menu integration ("Send via Wisp" on files
/// and folders) to the native runner.  Registration lives in the registry under
/// HKCU (per-user, no admin); the native side owns the actual register /
/// unregister / status checks.  When a second launch forwards selected paths to
/// the running window (single-instance + WM_COPYDATA), they arrive here on
/// [onSendViaWisp].
///
/// Mirrors the shape of [AndroidShareIntent]: all methods are no-ops / safe
/// defaults off Windows so callers don't need to platform-guard every call.
class WindowsContextMenu {
  static const MethodChannel _channel = MethodChannel(
    'dev.vigov5.wisp/windows_integration',
  );

  static final StreamController<List<String>> _controller =
      StreamController<List<String>>.broadcast();

  static bool _wired = false;

  /// Stream of file/folder paths forwarded from a "Send via Wisp" click while
  /// the app is already running (warm start).  Cold-start paths arrive as
  /// process launch arguments instead (see `main.dart`).
  static Stream<List<String>> get onSendViaWisp {
    _ensureWired();
    return _controller.stream;
  }

  /// True when the context-menu verb is currently registered and points at this
  /// executable.  Always false off Windows.
  static Future<bool> isRegistered() async {
    if (!Platform.isWindows) return false;
    final result = await _channel.invokeMethod<bool>('isContextMenuRegistered');
    return result ?? false;
  }

  /// Adds the "Send via Wisp" verb for files and folders.  Returns true on
  /// success.  No-op (false) off Windows.
  static Future<bool> register() async {
    if (!Platform.isWindows) return false;
    final result = await _channel.invokeMethod<bool>('registerContextMenu');
    return result ?? false;
  }

  /// Removes the "Send via Wisp" verb.  Returns true on success.  No-op (false)
  /// off Windows.
  static Future<bool> unregister() async {
    if (!Platform.isWindows) return false;
    final result = await _channel.invokeMethod<bool>('unregisterContextMenu');
    return result ?? false;
  }

  static void _ensureWired() {
    if (_wired) return;
    _wired = true;
    if (!Platform.isWindows) return;
    _channel.setMethodCallHandler((call) async {
      if (call.method == 'onSendViaWisp') {
        final list = (call.arguments as List?)?.cast<String>() ?? const [];
        if (list.isNotEmpty) {
          _controller.add(list);
        }
      }
    });
  }
}
