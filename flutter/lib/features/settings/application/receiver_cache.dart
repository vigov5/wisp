/// The receiver's scratch space: where it is, how big it is, how to empty it,
/// and when emptying it is safe.
///
/// Two places ask — the Settings → Storage row and the startup warning above
/// the footer — and they must not disagree about the folder, the number, or
/// the answer to "can I delete this right now", so all of it lives here once.
library;

import 'dart:async';
import 'dart:io';

import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../../app/app_bootstrap.dart';
import '../../transfers/application/controller.dart';
import '../../transfers/application/state.dart';
import 'controller.dart';

/// Big enough to be worth mentioning unprompted at startup.
const int kReceiverCacheWarningBytes = 500 * 1024 * 1024;

/// Where the receiver's scratch space is, and how much of it is ours.
class ReceiverCacheLocation {
  const ReceiverCacheLocation({required this.root, required this.wholeRoot});

  /// On Android, the fixed `<tmpDir>/Download/Wisp` the Rust receiver is
  /// always pointed at, whatever the user's download root or SAF choice says.
  /// Elsewhere, the user's own download root.
  final String root;

  /// Android stages whole received files in [root] itself and only moves them
  /// into MediaStore once the transfer finishes, so everything under it is
  /// ours — including the gigabytes a failed save leaves behind, which is
  /// exactly the part worth reporting. Everywhere else [root] is the folder
  /// the user keeps their files in, and only `.wisp/` is ours to touch.
  final bool wholeRoot;

  Directory get directory => wholeRoot
      ? Directory(root)
      : Directory('$root${Platform.pathSeparator}.wisp');
}

/// Returns where the cache lives, or null when it cannot be walked (a SAF
/// `content://` root off Android).
Future<ReceiverCacheLocation?> resolveReceiverCacheLocation(
  String downloadRoot,
) async {
  // Delegates to the function app_bootstrap uses, so the '/Download/Wisp'
  // suffix is never spelled out twice.
  final androidDir = await resolveAndroidReceiveCacheDir();
  if (androidDir != null) {
    return ReceiverCacheLocation(root: androidDir, wholeRoot: true);
  }
  if (downloadRoot.startsWith('content://')) return null;
  final root = downloadRoot.trim();
  if (root.isEmpty) return null;
  return ReceiverCacheLocation(root: root, wholeRoot: false);
}

Future<int> receiverCacheSizeBytes(ReceiverCacheLocation location) =>
    _walkDirSize(location.directory);

Future<void> deleteReceiverCache(ReceiverCacheLocation location) async {
  final dir = location.directory;
  if (await dir.exists()) {
    await dir.delete(recursive: true);
  }
}

/// Whether the receiver is still using the cache.
///
/// Clearing it mid-transfer deletes the transfer out from under itself: the
/// resume state always lives there, and on Android so do the received files
/// themselves, until the background save has moved them into MediaStore —
/// which is still running while the finished screen is up.
bool receiverCacheIsBusy(TransferSessionPhase phase) => switch (phase) {
  TransferSessionPhase.idle ||
  TransferSessionPhase.cancelled ||
  TransferSessionPhase.failed => false,
  _ => true,
};

final receiverCacheBusyProvider = Provider<bool>((ref) {
  return receiverCacheIsBusy(ref.watch(transfersViewStateProvider).phase);
});

class ReceiverCacheState {
  const ReceiverCacheState({
    this.location,
    this.sizeBytes,
    this.measured = false,
    this.clearing = false,
    this.dismissed = false,
  });

  /// Null when the cache cannot be located.
  final ReceiverCacheLocation? location;

  /// Bytes under [ReceiverCacheLocation.directory]. Null until measured, and
  /// whenever the path cannot be walked.
  final int? sizeBytes;

  /// False until the first walk finishes, so an unmeasured cache never looks
  /// like an empty one.
  final bool measured;

  final bool clearing;
  final bool dismissed;

  bool get isOversized => (sizeBytes ?? 0) >= kReceiverCacheWarningBytes;

  /// The startup warning appears only for a cache that has actually been
  /// measured and is genuinely large, and stops the moment it is cleared or
  /// waved away.
  bool get shouldWarn => measured && isOversized && !dismissed;

  ReceiverCacheState copyWith({
    int? sizeBytes,
    bool? measured,
    bool? clearing,
    bool? dismissed,
  }) {
    return ReceiverCacheState(
      location: location,
      sizeBytes: sizeBytes ?? this.sizeBytes,
      measured: measured ?? this.measured,
      clearing: clearing ?? this.clearing,
      dismissed: dismissed ?? this.dismissed,
    );
  }
}

final receiverCacheProvider =
    NotifierProvider<ReceiverCacheNotifier, ReceiverCacheState>(
      ReceiverCacheNotifier.new,
    );

class ReceiverCacheNotifier extends Notifier<ReceiverCacheState> {
  /// Bumped on every rebuild and on dispose. A walk still running when the
  /// download root changes compares it and drops its result rather than
  /// writing a stale size over a newer one — or writing to a dead notifier.
  int _generation = 0;

  @override
  ReceiverCacheState build() {
    final downloadRoot = ref.watch(
      settingsControllerProvider.select((state) => state.settings.downloadRoot),
    );
    final generation = ++_generation;
    ref.onDispose(() => _generation++);
    unawaited(_measure(downloadRoot, generation));
    return const ReceiverCacheState();
  }

  Future<void> _measure(String downloadRoot, int generation) async {
    final location = await resolveReceiverCacheLocation(downloadRoot);
    final bytes = location == null
        ? null
        : await receiverCacheSizeBytes(location);
    if (generation != _generation) return;
    state = ReceiverCacheState(
      location: location,
      sizeBytes: bytes,
      measured: true,
      dismissed: state.dismissed,
    );
  }

  /// Re-walks the tree — after the Settings page clears the cache, so the
  /// banner and the Storage row never show different numbers.
  Future<void> refresh() {
    final settings = ref.read(settingsControllerProvider).settings;
    return _measure(settings.downloadRoot, _generation);
  }

  void dismiss() {
    state = state.copyWith(dismissed: true);
  }

  /// Deletes the cache and re-measures. Returns the failure text, or null
  /// when it worked.
  Future<String?> clear() async {
    final location = state.location;
    if (location == null || state.clearing) return null;
    if (ref.read(receiverCacheBusyProvider)) {
      return 'a transfer is still using it';
    }
    final generation = _generation;
    state = state.copyWith(clearing: true);

    String? failure;
    try {
      await deleteReceiverCache(location);
    } catch (error) {
      failure = '$error';
    }
    if (generation != _generation) return failure;
    state = state.copyWith(clearing: false);
    await _measure(
      ref.read(settingsControllerProvider).settings.downloadRoot,
      generation,
    );
    return failure;
  }
}

Future<int> _walkDirSize(Directory dir) async {
  if (!await dir.exists()) return 0;
  var total = 0;
  try {
    await for (final entity in dir.list(recursive: true, followLinks: false)) {
      if (entity is File) {
        try {
          total += await entity.length();
        } catch (_) {
          // best-effort: file deleted mid-walk, permission denied
        }
      }
    }
  } catch (_) {
    // Best-effort: top-level list error
  }
  return total;
}

String formatCacheBytes(int bytes) {
  if (bytes < 1024) return '$bytes B';
  const units = ['KB', 'MB', 'GB', 'TB'];
  var value = bytes / 1024.0;
  var idx = 0;
  while (value >= 1024 && idx < units.length - 1) {
    value /= 1024;
    idx++;
  }
  final fixed = value < 10
      ? value.toStringAsFixed(1)
      : value.toStringAsFixed(0);
  return '$fixed ${units[idx]}';
}
