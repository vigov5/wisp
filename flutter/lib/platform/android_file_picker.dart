import 'dart:async';
import 'dart:io';

import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:path_provider/path_provider.dart';

import 'native_source.dart';

/// Files selected through Android's Storage Access Framework.
///
/// Most of them are never copied anywhere: the native side opens the SAF URI
/// and hands over the descriptor path, which the core reads through the
/// descriptor itself.  [bytesCopied] counts the sources that had to fall back
/// to a cache copy — a provider answering with a pipe, or a folder tree the
/// descriptor budget could not cover.  [copyElapsed] covers native
/// metadata reads and that copying; time spent by the user in the system
/// picker is deliberately excluded.
class AndroidFilePickResult {
  const AndroidFilePickResult({
    required this.sources,
    required this.bytesCopied,
    required this.copyElapsed,
  });

  final List<NativeSource> sources;
  final BigInt bytesCopied;
  final Duration copyElapsed;
}

/// Result of [AndroidFilePicker.pickFolder].
///
/// A SAF tree has no filesystem path and no single handle to open, so the
/// native side resolves it to one source per file — normally a live
/// descriptor, and a cache copy only for what the descriptor budget could not
/// cover.  [bytesCopied] counts just that remainder.
class AndroidFolderResult {
  const AndroidFolderResult({
    required this.identity,
    required this.name,
    required this.sources,
    required this.sizeBytes,
    required this.bytesCopied,
    required this.copyElapsed,
  });

  /// The tree URI. Not openable by the core — it is what the draft keys this
  /// folder by, in place of a path.
  final String identity;

  /// The picked folder's own name, and the root of every source's transfer
  /// path.
  final String name;

  /// One entry per file in the folder, each carrying its path within it.
  final List<NativeSource> sources;

  /// Total size in bytes computed on the native side via the Storage Access
  /// Framework — works regardless of Android scoped-storage restrictions.
  final BigInt sizeBytes;

  /// Of [sizeBytes], how much had to be copied into the app cache.
  final BigInt bytesCopied;

  /// Native SAF traversal + any copying, excluding the system picker UI.
  final Duration copyElapsed;
}

/// Progress of the native URI → app-cache copy that backs [AndroidFilePicker.
/// pickFiles] / [AndroidFilePicker.pickFolder]. Emitted while a multi-GB pick
/// streams so the UI can show a progress bar instead of appearing frozen.
class AndroidPickProgress {
  const AndroidPickProgress({
    required this.bytesCopied,
    required this.totalBytes,
    required this.index,
    required this.count,
  });

  /// Bytes copied so far across the whole selection.
  final int bytesCopied;

  /// Total bytes to copy, or 0 when unknown (folder picks) — [fraction] is
  /// then null and the UI should render an indeterminate indicator.
  final int totalBytes;

  /// Index of the file currently being copied (0-based).
  final int index;

  /// Number of items in the selection.
  final int count;

  /// True when this describes files being opened rather than bytes copied.
  ///
  /// The two phases of a pick report differently because they cost
  /// differently: copying is bounded by bytes, opening a descriptor is one
  /// binder round trip per file whatever its size.
  bool get countsFiles => totalBytes <= 0 && count > 0 && bytesCopied == 0;

  /// Progress in [0, 1], or null when neither bytes nor a file count are
  /// known.
  ///
  /// Falls back to files: a folder pick reports no total byte count, so
  /// [totalBytes] alone left every folder on an indeterminate bar — including
  /// the 1911-file folder that spends half a minute here.
  double? get fraction {
    if (totalBytes > 0) {
      return (bytesCopied / totalBytes).clamp(0.0, 1.0);
    }
    if (count > 0 && countsFiles) {
      return (index / count).clamp(0.0, 1.0);
    }
    return null;
  }
}

/// Calls a native Android [MethodChannel] that bypasses two [file_selector_android]
/// limitations:
///
/// 1. **OOM on large file picks** (versions ≤ 0.5.2+x): the plugin reads the
///    entire picked file into a [ByteArrayOutputStream] and encodes it through
///    Flutter's [StandardMessageCodec] platform channel. For files ≥ ~195 MB
///    this exhausts Android's heap before any Dart code runs
///    (see https://github.com/flutter/flutter/issues/141002). Our [pickFiles]
///    implementation streams files to the app cache directory in 64 KB chunks
///    and sends only the resulting path — no bytes cross the channel.
///
/// 2. **0 B directory size under scoped storage** (Android 10+): Dart's
///    [Directory.list] uses direct syscalls that are blocked by scoped storage
///    for paths outside the app sandbox, so recursive stat-based enumeration
///    always returns 0 B. Our [pickFolder] implementation computes the size on
///    the native side via [DocumentFile] (Storage Access Framework), which
///    respects the URI grant the user approved in the system picker.
class AndroidFilePicker {
  static const MethodChannel _channel = MethodChannel(
    'dev.vigov5.wisp/file_picker',
  );

  // Cache of sizes returned by the native pickFolder call, keyed by path.
  // Used by [AndroidDirectorySizeCalculator] to serve size lookups without
  // re-traversing the directory tree.
  static final Map<String, BigInt> _folderSizeCache = {};

  /// Live progress of the in-flight copy, or null when idle. Watch this with a
  /// [ValueListenableBuilder] to drive a progress dialog while [pickFiles] /
  /// [pickFolder] are streaming a large selection into the cache.
  static final ValueNotifier<AndroidPickProgress?> pickProgress =
      ValueNotifier<AndroidPickProgress?>(null);

  /// Files the last pick could not prepare.  Emitted rather than returned
  /// because a rejection is not a picked file: the pick still succeeds, with
  /// fewer items than the user chose, and something has to say why.
  static Stream<List<SourceRejection>> get onRejected => _rejected.stream;

  static final StreamController<List<SourceRejection>> _rejected =
      StreamController<List<SourceRejection>>.broadcast();

  static void _reportRejections(Object? raw) {
    final rejections = SourceRejection.parseAll(raw);
    if (rejections.isNotEmpty) _rejected.add(rejections);
  }

  static bool _wired = false;

  static void _ensureWired() {
    if (_wired || !Platform.isAndroid) return;
    _wired = true;
    _channel.setMethodCallHandler((call) async {
      if (call.method == 'onPickProgress') {
        final args = (call.arguments as Map).cast<dynamic, dynamic>();
        pickProgress.value = AndroidPickProgress(
          bytesCopied: (args['bytesCopied'] as num?)?.toInt() ?? 0,
          totalBytes: (args['totalBytes'] as num?)?.toInt() ?? 0,
          index: (args['index'] as num?)?.toInt() ?? 0,
          count: (args['count'] as num?)?.toInt() ?? 0,
        );
      }
    });
  }

  /// Opens the system file picker and returns one [NativeSource] per selected
  /// file, each already openable by the core — normally as a live descriptor
  /// path, and only as a cache copy where the platform left no alternative.
  static Future<AndroidFilePickResult> pickFiles() async {
    _ensureWired();
    pickProgress.value = null;
    try {
      final result = await _channel.invokeMethod<Map<dynamic, dynamic>>(
        'pickFiles',
      );
      final raw = result?['sources'];
      _reportRejections(result?['rejected']);
      return AndroidFilePickResult(
        sources: NativeSource.parseAll(raw is List ? raw : null),
        bytesCopied: BigInt.from(_nonNegativeInt(result?['bytesCopied'])),
        copyElapsed: Duration(
          microseconds: _nonNegativeInt(result?['copyElapsedMicros']),
        ),
      );
    } finally {
      pickProgress.value = null;
    }
  }

  /// Opens the system folder picker via [ACTION_OPEN_DOCUMENT_TREE].
  /// The native side computes the total directory size using the Storage
  /// Access Framework, which works under scoped storage on all API levels.
  /// Returns null if the user cancels or the selected path cannot be resolved.
  static Future<AndroidFolderResult?> pickFolder() async {
    _ensureWired();
    pickProgress.value = null;
    final Map<dynamic, dynamic>? result;
    try {
      result = await _channel.invokeMethod<Map<dynamic, dynamic>>('pickFolder');
    } finally {
      pickProgress.value = null;
    }
    if (result == null) return null;
    _reportRejections(result['rejected']);

    final identity = result['identity'] as String?;
    if (identity == null || identity.isEmpty) return null;

    final rawSources = result['sources'];
    final sources = NativeSource.parseAll(
      rawSources is List ? rawSources : null,
    );
    if (sources.isEmpty) return null;

    final sizeBytes = switch (result['sizeBytes']) {
      int v when v >= 0 => BigInt.from(v),
      _ => BigInt.zero,
    };

    _folderSizeCache[identity] = sizeBytes;
    return AndroidFolderResult(
      identity: identity,
      name: (result['name'] as String?)?.trim().isNotEmpty == true
          ? result['name'] as String
          : 'folder',
      sources: sources,
      sizeBytes: sizeBytes,
      bytesCopied: BigInt.from(_nonNegativeInt(result['bytesCopied'])),
      copyElapsed: Duration(
        microseconds: _nonNegativeInt(result['copyElapsedMicros']),
      ),
    );
  }

  static int _nonNegativeInt(Object? value) {
    if (value is! num || !value.isFinite || value < 0) return 0;
    return value.toInt();
  }

  /// Returns the cached size for [path] previously set by [pickFolder].
  static BigInt? cachedSizeOf(String path) => _folderSizeCache[path];

  /// Releases everything a pick is holding: the descriptors kept open for
  /// copy-free sends, and the files that did get copied into the app cache by
  /// [pickFiles] / [pickFolder]. Call this when the draft is cleared. Safe to
  /// call multiple times.
  static Future<void> clearPickedCache() async {
    _folderSizeCache.clear();
    if (Platform.isAndroid) {
      try {
        await _channel.invokeMethod<void>('releaseSendSources');
      } catch (_) {
        // Best-effort — the descriptors go with the process anyway.
      }
    }
    try {
      final tmp = await getTemporaryDirectory();
      final dir = Directory('${tmp.path}/wisp_picked');
      if (await dir.exists()) {
        await dir.delete(recursive: true);
      }
    } catch (_) {
      // Best-effort — ignore errors (e.g. files still in use).
    }
  }
}
