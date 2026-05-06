import 'dart:io';

import 'package:file_selector/file_selector.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../../platform/android_file_picker.dart';
import 'model.dart';

abstract class SendSelectionPicker {
  Future<List<SendPickedFile>> pickFiles();

  Future<List<SendPickedFile>> pickFolder();
}

final sendSelectionPickerProvider = Provider<SendSelectionPicker>((_) {
  return FileSelectorSendSelectionPicker();
});

class FileSelectorSendSelectionPicker implements SendSelectionPicker {
  @override
  Future<List<SendPickedFile>> pickFiles() async {
    // On Android, file_selector_android <= 0.5.2+x reads the entire file into
    // a ByteArrayOutputStream and sends it through the platform channel.
    // For large files this triggers an OutOfMemoryError.  Use a custom
    // MethodChannel that copies files to the app cache via streaming instead.
    if (Platform.isAndroid) {
      final paths = await AndroidFilePicker.pickFiles();
      return paths
          .map((path) {
            BigInt? sizeBytes;
            try {
              sizeBytes = BigInt.from(File(path).lengthSync());
            } catch (_) {
              sizeBytes = null;
            }
            return SendPickedFile(
              path: path,
              name: SendPickedFile.fromPath(path).name,
              kind: SendPickedFileKind.file,
              sizeBytes: sizeBytes,
            );
          })
          .toList(growable: false);
    }

    final pickedFiles = await openFiles();
    return Future.wait(
      pickedFiles.map((file) async {
        final path = file.path.isNotEmpty ? file.path : file.name;
        BigInt? sizeBytes;
        try {
          sizeBytes = BigInt.from(await file.length());
        } catch (_) {
          sizeBytes = null;
        }
        return SendPickedFile(
          path: path,
          name: file.name.trim().isEmpty
              ? SendPickedFile.fromPath(path).name
              : file.name,
          kind: SendPickedFileKind.file,
          sizeBytes: sizeBytes,
        );
      }),
    );
  }

  @override
  Future<List<SendPickedFile>> pickFolder() async {
    // On Android, ACTION_OPEN_DOCUMENT_TREE returns a SAF tree URI. The native
    // side resolves it to a filesystem path and computes the directory size via
    // DocumentFile, which works under scoped storage. We cache the size so
    // DirectorySizeCalculator can serve it without re-traversing the tree.
    if (Platform.isAndroid) {
      final result = await AndroidFilePicker.pickFolder();
      if (result == null) return const [];
      return [
        SendPickedFile(
          path: result.path,
          name: SendPickedFile.directory(result.path).name,
          kind: SendPickedFileKind.directory,
          sizeBytes: result.sizeBytes,
        ),
      ];
    }

    final path = await getDirectoryPath();
    if (path == null || path.isEmpty) {
      return const [];
    }

    return [
      SendPickedFile(
        path: path,
        name: SendPickedFile.directory(path).name,
        kind: SendPickedFileKind.directory,
      ),
    ];
  }
}
