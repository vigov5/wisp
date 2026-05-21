import 'dart:async';
import 'dart:io';

import 'package:flutter/services.dart';

/// Channels Android ACTION_SEND / ACTION_SEND_MULTIPLE intents into Flutter
/// as lists of cached file paths.  The native side copies each shared URI
/// into the app cache (same `wisp_picked` tree used by the file picker),
/// so the returned paths are ready to feed directly into a Send draft
/// without any further bridging.
class AndroidShareIntent {
  static const MethodChannel _channel = MethodChannel(
    'dev.vigov5.wisp/share_intent',
  );

  static final StreamController<List<String>> _controller =
      StreamController<List<String>>.broadcast();

  static bool _wired = false;

  /// Stream of newly-shared file-path lists arriving while the app is
  /// already running (warm-start ACTION_SEND / ACTION_SEND_MULTIPLE).
  /// Cold-start intents are delivered via [getInitialSharedFiles] instead.
  static Stream<List<String>> get onSharedFiles {
    _ensureWired();
    return _controller.stream;
  }

  /// Returns the list of files attached to the Android intent that
  /// launched the app, or an empty list when launched normally.  The
  /// native side hands the cold-start stash over only once — subsequent
  /// calls return an empty list.
  static Future<List<String>> getInitialSharedFiles() async {
    if (!Platform.isAndroid) return const [];
    _ensureWired();
    final result = await _channel.invokeMethod<List<dynamic>>(
      'getInitialSharedFiles',
    );
    return result?.cast<String>() ?? const [];
  }

  static void _ensureWired() {
    if (_wired) return;
    _wired = true;
    if (!Platform.isAndroid) return;
    _channel.setMethodCallHandler((call) async {
      if (call.method == 'onSharedFiles') {
        final list = (call.arguments as List?)?.cast<String>() ?? const [];
        if (list.isNotEmpty) {
          _controller.add(list);
        }
      }
    });
  }
}
