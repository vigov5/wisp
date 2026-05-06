import 'dart:async';
import 'dart:io';

import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';

/// Result from [AndroidMediaStore.pickSaveFolder].
class AndroidSaveFolder {
  const AndroidSaveFolder({required this.uri, required this.displayName});

  /// The SAF tree URI (e.g. `content://com.android.externalstorage…`).
  /// Persist this string; it carries its own read+write permission.
  final String uri;

  /// Human-readable folder name returned by the system picker.
  final String displayName;
}

/// Saves received files to a user-chosen folder (SAF) or the public
/// `Downloads/Drift/` folder (MediaStore default) on Android.
///
/// **Folder selection flow (settings page):**
/// 1. Call [pickSaveFolder] → user picks a folder → returns [AndroidSaveFolder]
/// 2. Store `folder.uri` in `settings.downloadRoot`.
/// 3. On transfer complete, [saveToSafUri] is called with that URI.
///
/// **MediaStore default (no folder chosen):**
/// - API 29+ → `MediaStore.Downloads` (`Downloads/Drift/`)
/// - API < 29 → app-specific external downloads
class AndroidMediaStore {
  static const _channel = MethodChannel('com.example.drift/file_picker');

  /// Opens the system folder picker so the user can choose where received
  /// files will be saved.  The chosen folder is granted persistent read+write
  /// permission automatically.
  ///
  /// Returns `null` if the user cancels.
  static Future<AndroidSaveFolder?> pickSaveFolder() async {
    if (!Platform.isAndroid) return null;
    try {
      final raw = await _channel.invokeMethod<Map>('pickSaveFolder');
      if (raw == null) return null;
      return AndroidSaveFolder(
        uri: raw['uri'] as String,
        displayName: raw['displayName'] as String? ?? 'Selected folder',
      );
    } on PlatformException catch (e) {
      debugPrint('[AndroidMediaStore] pickSaveFolder failed: ${e.message}');
      return null;
    }
  }

  /// Returns `true` if [value] looks like a SAF tree URI stored by
  /// [pickSaveFolder], i.e. it starts with `content://`.
  static bool isSafUri(String value) => value.startsWith('content://');

  /// Copies [srcAbsPath] into the user-chosen SAF folder at [treeUri],
  /// preserving [relativeFilePath] sub-directories under that folder root.
  ///
  /// Returns the saved DocumentFile URI on success, or `null` on failure.
  static Future<String?> saveToSafUri(
    String srcAbsPath,
    String relativeFilePath,
    String treeUri,
  ) async {
    if (!Platform.isAndroid) return null;
    try {
      final result = await _channel.invokeMethod<String>('saveToSafUri', {
        'srcPath': srcAbsPath,
        'relativeFilePath': relativeFilePath,
        'treeUri': treeUri,
      });
      return result;
    } on PlatformException catch (e) {
      debugPrint('[AndroidMediaStore] saveToSafUri failed: ${e.message}');
      return null;
    }
  }

  /// Copies [srcAbsPath] (absolute path inside the app cache) to
  /// `Downloads/Drift/<relativeFilePath>` on the device.
  ///
  /// On API 29+: uses `MediaStore.Downloads` — no storage permission needed.
  /// On API < 29: writes to the app-specific external downloads directory.
  ///
  /// Returns the saved URI / path on success, or `null` on failure.
  static Future<String?> saveToDownloads(
    String srcAbsPath,
    String relativeFilePath,
  ) async {
    if (!Platform.isAndroid) return null;
    final mimeType = _guessMimeType(relativeFilePath);
    try {
      final result = await _channel.invokeMethod<String>('saveToDownloads', {
        'srcPath': srcAbsPath,
        'relativeFilePath': relativeFilePath,
        'mimeType': mimeType,
      });
      return result;
    } on PlatformException catch (e) {
      debugPrint('[AndroidMediaStore] saveToDownloads failed: ${e.message}');
      return null;
    }
  }

  /// Deletes the temp receive cache directory at [cacheRoot].
  /// Errors are silently ignored (best-effort cleanup).
  static Future<void> cleanupReceiveCache(String cacheRoot) async {
    try {
      final dir = Directory(cacheRoot);
      if (await dir.exists()) {
        await dir.delete(recursive: true);
      }
    } catch (e) {
      debugPrint('[AndroidMediaStore] cleanup failed: $e');
    }
  }

  /// Returns a basic MIME type based on the file extension.
  static String _guessMimeType(String filePath) {
    final ext = filePath.split('.').last.toLowerCase();
    return _mimeMap[ext] ?? 'application/octet-stream';
  }

  static const _mimeMap = <String, String>{
    // Images
    'jpg': 'image/jpeg',
    'jpeg': 'image/jpeg',
    'png': 'image/png',
    'gif': 'image/gif',
    'webp': 'image/webp',
    'heic': 'image/heic',
    'heif': 'image/heif',
    'bmp': 'image/bmp',
    'svg': 'image/svg+xml',
    // Video
    'mp4': 'video/mp4',
    'mov': 'video/quicktime',
    'avi': 'video/x-msvideo',
    'mkv': 'video/x-matroska',
    'webm': 'video/webm',
    '3gp': 'video/3gpp',
    // Audio
    'mp3': 'audio/mpeg',
    'aac': 'audio/aac',
    'wav': 'audio/wav',
    'ogg': 'audio/ogg',
    'flac': 'audio/flac',
    'm4a': 'audio/mp4',
    // Docs
    'pdf': 'application/pdf',
    'doc': 'application/msword',
    'docx':
        'application/vnd.openxmlformats-officedocument.wordprocessingml.document',
    'xls': 'application/vnd.ms-excel',
    'xlsx': 'application/vnd.openxmlformats-officedocument.spreadsheetml.sheet',
    'ppt': 'application/vnd.ms-powerpoint',
    'pptx':
        'application/vnd.openxmlformats-officedocument.presentationml.presentation',
    'txt': 'text/plain',
    'csv': 'text/csv',
    'html': 'text/html',
    'htm': 'text/html',
    'xml': 'text/xml',
    'json': 'application/json',
    // Archives
    'zip': 'application/zip',
    'tar': 'application/x-tar',
    'gz': 'application/gzip',
    '7z': 'application/x-7z-compressed',
    'rar': 'application/vnd.rar',
    // Other
    'apk': 'application/vnd.android.package-archive',
  };
}
