import 'dart:async';
import 'dart:io';

import 'package:flutter/services.dart';

/// Channels OS-level "share to Wisp" hand-offs into Flutter as lists of cached
/// file paths (or plain text).
///
/// Both mobile platforms feed the same `dev.vigov5.wisp/share_intent` channel
/// and the same cold-start/warm-start contract:
///   - Android: `ACTION_SEND` / `ACTION_SEND_MULTIPLE` intents.
///   - iOS: files opened into the app via the share sheet / "Open in Wisp"
///     (declared through `CFBundleDocumentTypes`), delivered as scene URL
///     contexts.
///
/// In both cases the native side copies each shared item into an app-owned
/// cache directory first, so the returned paths are ready to feed straight
/// into a Send draft without further bridging.
class ShareIntent {
  static const MethodChannel _channel = MethodChannel(
    'dev.vigov5.wisp/share_intent',
  );

  static final StreamController<List<String>> _controller =
      StreamController<List<String>>.broadcast();

  static final StreamController<String> _textController =
      StreamController<String>.broadcast();

  static bool _wired = false;

  /// Whether this platform delivers shares through the native channel.
  static bool get isSupported => Platform.isAndroid || Platform.isIOS;

  /// Stream of newly-shared file-path lists arriving while the app is
  /// already running (warm start).  Cold-start shares are delivered via
  /// [getInitialSharedFiles] instead.
  static Stream<List<String>> get onSharedFiles {
    _ensureWired();
    return _controller.stream;
  }

  /// Stream of newly-shared plain text arriving while the app is already
  /// running (warm start).  Cold-start text is delivered via
  /// [getInitialSharedText] instead.
  static Stream<String> get onSharedText {
    _ensureWired();
    return _textController.stream;
  }

  /// Returns the files attached to the share that launched the app, or an
  /// empty list when launched normally.  The native side hands the cold-start
  /// stash over only once — subsequent calls return an empty list.
  static Future<List<String>> getInitialSharedFiles() async {
    if (!isSupported) return const [];
    _ensureWired();
    final result = await _channel.invokeMethod<List<dynamic>>(
      'getInitialSharedFiles',
    );
    return result?.cast<String>() ?? const [];
  }

  /// Returns the plain text attached to the share that launched the app, or
  /// null when launched normally.  The native side hands the cold-start stash
  /// over only once.
  static Future<String?> getInitialSharedText() async {
    if (!isSupported) return null;
    _ensureWired();
    return _channel.invokeMethod<String>('getInitialSharedText');
  }

  static void _ensureWired() {
    if (_wired) return;
    _wired = true;
    if (!isSupported) return;
    _channel.setMethodCallHandler((call) async {
      if (call.method == 'onSharedFiles') {
        final list = (call.arguments as List?)?.cast<String>() ?? const [];
        if (list.isNotEmpty) {
          _controller.add(list);
        }
      } else if (call.method == 'onSharedText') {
        final text = call.arguments as String?;
        if (text != null && text.isNotEmpty) {
          _textController.add(text);
        }
      }
    });
  }
}
