import 'dart:io';

import 'package:flutter/services.dart';
import 'package:path_provider/path_provider.dart';

/// Result of [AndroidFilePicker.pickFolder].
class AndroidFolderResult {
  const AndroidFolderResult({required this.path, required this.sizeBytes});

  final String path;

  /// Total size in bytes computed on the native side via the Storage Access
  /// Framework — works regardless of Android scoped-storage restrictions.
  final BigInt sizeBytes;
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

  /// Opens the system file picker and returns a list of absolute paths to
  /// copies of the selected files stored in the app cache directory.
  static Future<List<String>> pickFiles() async {
    final result = await _channel.invokeMethod<List<dynamic>>('pickFiles');
    return result?.cast<String>() ?? const [];
  }

  /// Opens the system folder picker via [ACTION_OPEN_DOCUMENT_TREE].
  /// The native side computes the total directory size using the Storage
  /// Access Framework, which works under scoped storage on all API levels.
  /// Returns null if the user cancels or the selected path cannot be resolved.
  static Future<AndroidFolderResult?> pickFolder() async {
    final result = await _channel.invokeMethod<Map<dynamic, dynamic>>(
      'pickFolder',
    );
    if (result == null) return null;

    final path = result['path'] as String?;
    if (path == null || path.isEmpty) return null;

    final rawSize = result['sizeBytes'];
    final sizeBytes = switch (rawSize) {
      int v => BigInt.from(v),
      _ => BigInt.zero,
    };

    _folderSizeCache[path] = sizeBytes;
    return AndroidFolderResult(path: path, sizeBytes: sizeBytes);
  }

  /// Returns the cached size for [path] previously set by [pickFolder].
  static BigInt? cachedSizeOf(String path) => _folderSizeCache[path];

  /// Deletes all files copied into the app cache by [pickFiles] and
  /// [pickFolder]. Call this when the draft is cleared so picked copies do
  /// not accumulate indefinitely. Safe to call multiple times.
  static Future<void> clearPickedCache() async {
    _folderSizeCache.clear();
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
