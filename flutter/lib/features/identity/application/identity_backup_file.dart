import 'dart:io';

import 'package:file_selector/file_selector.dart';
import 'package:path_provider/path_provider.dart';

import '../../../platform/android_media_store.dart';

/// Default filename suggested when exporting a backup to a file.
const String kBackupFileName = 'wisp-identity.wispkey';

/// A backup payload is ~150 chars, so a real `.wispkey` is well under a
/// kilobyte. We cap the read generously: anything past this is not a backup
/// (a wrong or huge file), and reading it whole would risk an out-of-memory
/// crash. Reject on size before touching the bytes.
const int _maxBackupFileBytes = 64 * 1024;

const _wispKeyTypeGroup = XTypeGroup(
  label: 'Wisp identity backup',
  extensions: <String>['wispkey'],
);

/// Thrown by [IdentityBackupFile.open] when the chosen file is too large to be
/// a Wisp identity backup, so the UI can show a friendly message instead of
/// letting a multi-megabyte read crash the app.
class IdentityBackupFileTooLargeException implements Exception {
  const IdentityBackupFileTooLargeException();

  @override
  String toString() => 'selected file is too large to be an identity backup';
}

/// Reads/writes the backup payload as a `.wispkey` text file.
///
/// The file simply contains the same `wisp-key:…` payload string the QR/code
/// carry, so no separate parsing is needed on import.
///
/// Platform split mirrors the rest of the app:
///   - Desktop: `file_selector` save/open dialogs, write/read the path directly.
///   - Android: read via `file_selector` (document picker); write via the
///     `dev.vigov5.wisp/file_picker` SAF channel into `Download/Wisp/` (the
///     same path received files use), because `file_selector` has no save
///     dialog on Android.
class IdentityBackupFile {
  const IdentityBackupFile();

  /// Whether a "Save to file" affordance should be offered on this platform.
  /// QR/code backup is always available; the file option is desktop + Android.
  static bool get isSupportedForSave =>
      Platform.isWindows ||
      Platform.isMacOS ||
      Platform.isLinux ||
      Platform.isAndroid;

  /// Writes [payload] to a user-chosen `.wispkey` file.
  ///
  /// Returns a human-readable destination description on success, or `null`
  /// when the user cancelled.
  Future<String?> save(String payload) async {
    if (Platform.isAndroid) {
      final tmp = await getTemporaryDirectory();
      final tmpFile = File('${tmp.path}/$kBackupFileName');
      await tmpFile.writeAsString(payload, flush: true);
      try {
        final saved = await AndroidMediaStore.saveToDownloads(
          tmpFile.path,
          kBackupFileName,
        );
        return saved == null ? null : 'Download/Wisp/$kBackupFileName';
      } finally {
        // The SAF copy is done; drop the plaintext temp copy promptly.
        if (await tmpFile.exists()) {
          await tmpFile.delete();
        }
      }
    }

    final location = await getSaveLocation(
      suggestedName: kBackupFileName,
      acceptedTypeGroups: const <XTypeGroup>[_wispKeyTypeGroup],
    );
    if (location == null) return null;
    await File(location.path).writeAsString(payload, flush: true);
    return location.path;
  }

  /// Opens a file picker and returns the file's text contents, or `null` when
  /// the user cancelled.
  ///
  /// Guards against a wrong or oversized pick: the size is checked before the
  /// bytes are read, so choosing (say) a video can't OOM the app. Throws
  /// [IdentityBackupFileTooLargeException] when the file exceeds
  /// [_maxBackupFileBytes].
  Future<String?> open() async {
    final file = await openFile(
      acceptedTypeGroups: const <XTypeGroup>[_wispKeyTypeGroup],
    );
    if (file == null) return null;
    if (await file.length() > _maxBackupFileBytes) {
      throw const IdentityBackupFileTooLargeException();
    }
    return file.readAsString();
  }
}
