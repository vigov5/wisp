import 'dart:io';

import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../../platform/android_file_picker.dart';

abstract class DirectorySizeCalculator {
  Future<BigInt> sizeOfDirectory(String path);
}

final directorySizeCalculatorProvider = Provider<DirectorySizeCalculator>((_) {
  if (Platform.isAndroid) {
    return const AndroidDirectorySizeCalculator();
  }
  return const FileSystemDirectorySizeCalculator();
});

/// Android-specific calculator.
///
/// On Android, [Directory.list()] is blocked by scoped storage for paths
/// outside the app sandbox, so recursive stat-based enumeration always returns
/// 0 B for user-picked folders.  Instead, the size is computed on the native
/// side via the Storage Access Framework (DocumentFile) during the folder pick,
/// and cached in [AndroidFilePicker].  For paths not in the cache (e.g. picked
/// on a previous app session), falls back to 0 B rather than a wrong value.
class AndroidDirectorySizeCalculator implements DirectorySizeCalculator {
  const AndroidDirectorySizeCalculator();

  @override
  Future<BigInt> sizeOfDirectory(String path) async {
    return AndroidFilePicker.cachedSizeOf(path) ?? BigInt.zero;
  }
}

class FileSystemDirectorySizeCalculator implements DirectorySizeCalculator {
  const FileSystemDirectorySizeCalculator();

  @override
  Future<BigInt> sizeOfDirectory(String path) async {
    final directory = Directory(path);
    if (!await directory.exists()) {
      return BigInt.zero;
    }

    BigInt total = BigInt.zero;
    try {
      await for (final entity in directory.list(
        recursive: true,
        followLinks: false,
      )) {
        if (entity is File) {
          try {
            total += BigInt.from(await entity.length());
          } catch (_) {
            // Ignore files that disappear or become unreadable mid-scan.
          }
        }
      }
    } catch (_) {
      return total;
    }

    return total;
  }
}
