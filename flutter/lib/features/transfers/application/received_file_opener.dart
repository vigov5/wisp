import 'dart:io';

import '../../../platform/android_media_store.dart';
import '../../../platform/rust/receiver/rust_source.dart';
import '../../../platform/rust/receiver/source.dart';

/// Outcome of trying to open a single received file from the finish screen.
enum ReceivedFileOpenStatus {
  /// Handed off to the OS / default app successfully.
  opened,

  /// Android only: the file finished downloading but the background
  /// MediaStore/SAF save hasn't landed it at its final URI yet. The caller
  /// should ask the user to retry in a moment.
  notReady,

  /// The file isn't where we expected (e.g. deleted, or save failed).
  notFound,

  /// No app on the device can open this file type.
  noHandler,

  /// The current platform can't open individual files.
  unsupported,

  /// An unexpected error while launching the opener.
  error,
}

/// Opens a single received file, identified by its transfer-relative [relativePath]
/// (e.g. `photos/trip/a.jpg`).
///
/// - **Desktop** (Windows/macOS/Linux): the file lives at `downloadRoot/relativePath`
///   — Rust exports the whole collection there before emitting `completed`, so it's
///   always on disk by the time the finish screen shows. Opened with the default app.
/// - **Android**: files are copied out to the public `Download/Wisp/` folder (or the
///   user's chosen SAF folder) in the background *after* `completed`. We open the exact
///   `content://` URI captured at save time (immune to MediaStore collision renames).
///   If the save for this file hasn't completed yet we report [ReceivedFileOpenStatus.notReady].
Future<ReceivedFileOpenStatus> openReceivedFile({
  required String relativePath,
  required String downloadRoot,
  required ReceiverServiceSource source,
}) async {
  final normalized = relativePath.replaceAll('\\', '/');

  if (Platform.isAndroid) {
    final uri = source is RustReceiverServiceSource
        ? source.savedReceivedFileUri(normalized)
        : null;
    if (uri == null || uri.isEmpty) {
      return ReceivedFileOpenStatus.notReady;
    }
    final opened = await AndroidMediaStore.openFileUri(
      uri,
      AndroidMediaStore.guessMimeType(normalized),
    );
    return opened
        ? ReceivedFileOpenStatus.opened
        : ReceivedFileOpenStatus.noHandler;
  }

  return _openDesktopFile(downloadRoot: downloadRoot, relativePath: normalized);
}

Future<ReceivedFileOpenStatus> _openDesktopFile({
  required String downloadRoot,
  required String relativePath,
}) async {
  final sep = Platform.pathSeparator;
  final root = downloadRoot.endsWith(sep)
      ? downloadRoot.substring(0, downloadRoot.length - 1)
      : downloadRoot;
  final relNative = relativePath.replaceAll('/', sep);
  final absolutePath = '$root$sep$relNative';

  if (!await File(absolutePath).exists()) {
    return ReceivedFileOpenStatus.notFound;
  }

  try {
    if (Platform.isMacOS) {
      await Process.start('open', [absolutePath]);
    } else if (Platform.isWindows) {
      // explorer.exe on a file launches it with its default app (and handles
      // spaces in the path without extra quoting) — same dependency the
      // "Show in Explorer" folder button already relies on.
      await Process.start('explorer', [absolutePath]);
    } else if (Platform.isLinux) {
      await Process.start('xdg-open', [absolutePath]);
    } else {
      return ReceivedFileOpenStatus.unsupported;
    }
    return ReceivedFileOpenStatus.opened;
  } catch (_) {
    return ReceivedFileOpenStatus.error;
  }
}
