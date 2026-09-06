import 'dart:async';
import 'dart:io';

import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';

import 'native_source.dart';

/// The outcome of one OS share: what it handed over, and what it could not.
@immutable
class SharedFiles {
  const SharedFiles({required this.sources, required this.rejected});

  static const SharedFiles empty = SharedFiles(
    sources: [],
    rejected: [],
  );

  /// Parses either shape the platforms send: iOS hands over a bare list of
  /// paths, Android a map that also carries the files it could not prepare.
  factory SharedFiles.parse(Object? raw) {
    if (raw is List) {
      return SharedFiles(sources: NativeSource.parseAll(raw), rejected: const []);
    }
    if (raw is! Map) return empty;
    final sources = raw['sources'];
    return SharedFiles(
      sources: NativeSource.parseAll(sources is List ? sources : null),
      rejected: SourceRejection.parseAll(raw['rejected']),
    );
  }

  final List<NativeSource> sources;
  final List<SourceRejection> rejected;

  bool get isEmpty => sources.isEmpty && rejected.isEmpty;
}

/// Channels OS-level "share to Wisp" hand-offs into Flutter as lists of
/// ready-to-send sources (or plain text).
///
/// Both mobile platforms feed the same `dev.vigov5.wisp/share_intent` channel
/// and the same cold-start/warm-start contract:
///   - Android: `ACTION_SEND` / `ACTION_SEND_MULTIPLE` intents.
///   - iOS: files opened into the app via the share sheet / "Open in Wisp"
///     (declared through `CFBundleDocumentTypes`), delivered as scene URL
///     contexts.
///
/// Either way the returned paths are ready to feed straight into a Send draft:
/// on Android usually a live descriptor onto the shared file, and only where
/// the platform leaves no alternative an app-owned cache copy.
class ShareIntent {
  static const MethodChannel _channel = MethodChannel(
    'dev.vigov5.wisp/share_intent',
  );

  static final StreamController<SharedFiles> _controller =
      StreamController<SharedFiles>.broadcast();

  static final StreamController<String> _textController =
      StreamController<String>.broadcast();

  static bool _wired = false;

  /// Whether this platform delivers shares through the native channel.
  static bool get isSupported => Platform.isAndroid || Platform.isIOS;

  /// Stream of newly-shared file lists arriving while the app is already
  /// running (warm start).  Cold-start shares are delivered via
  /// [getInitialSharedFiles] instead.
  static Stream<SharedFiles> get onSharedFiles {
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
  static Future<SharedFiles> getInitialSharedFiles() async {
    if (!isSupported) return SharedFiles.empty;
    _ensureWired();
    final result = await _channel.invokeMethod<Object?>(
      'getInitialSharedFiles',
    );
    return SharedFiles.parse(result);
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
        // Android sends a map of one entry per file (a descriptor path
        // plus the name it cannot carry itself) alongside the files it could
        // not prepare; iOS sends bare paths.  [SharedFiles.parse] takes
        // either.
        final shared = SharedFiles.parse(call.arguments);
        if (!shared.isEmpty) {
          _controller.add(shared);
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
