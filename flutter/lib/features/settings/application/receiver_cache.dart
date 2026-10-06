/// The receiver's scratch directory: where it is, how big it is, how to empty
/// it.
///
/// Two places ask — the Settings → Storage row and the startup warning above
/// the footer — and they must not disagree about the path or the size, so the
/// walking and the deleting live here once.
///
/// On Android, Rust always writes to `<tmpDir>/Download/Wisp/.wisp/` whatever
/// the user's download root or SAF configuration says, matching the logic in
/// `app_bootstrap.dart`. Everywhere else the cache is `<downloadRoot>/.wisp/`.
library;

import 'dart:async';
import 'dart:io';

import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../../app/app_bootstrap.dart';
import 'controller.dart';

/// Big enough to be worth mentioning unprompted at startup.
///
/// What is in there is leftovers — records and partial blobs from transfers
/// that already finished — so the only thing clearing it costs is the resume
/// state of a transfer still in flight.
const int kReceiverCacheWarningBytes = 500 * 1024 * 1024;

/// Returns the directory whose `.wisp/` sub-directory holds the cache, or null
/// when it cannot be determined (a SAF `content://` root off Android).
Future<String?> resolveReceiverCacheRoot(String downloadRoot) async {
  // Delegates to the function app_bootstrap uses, so the '/Download/Wisp'
  // suffix is never spelled out twice.
  final androidDir = await resolveAndroidReceiveCacheDir();
  if (androidDir != null) return androidDir;
  if (downloadRoot.startsWith('content://')) return null;
  final root = downloadRoot.trim();
  return root.isEmpty ? null : root;
}

Directory receiverCacheDirectory(String cacheRoot) =>
    Directory('$cacheRoot${Platform.pathSeparator}.wisp');

Future<int> receiverCacheSizeBytes(String cacheRoot) =>
    _walkDirSize(receiverCacheDirectory(cacheRoot));

Future<void> deleteReceiverCache(String cacheRoot) async {
  final dir = receiverCacheDirectory(cacheRoot);
  if (await dir.exists()) {
    await dir.delete(recursive: true);
  }
}

class ReceiverCacheState {
  const ReceiverCacheState({
    this.cacheRoot,
    this.sizeBytes,
    this.measured = false,
    this.clearing = false,
    this.dismissed = false,
  });

  /// Directory containing `.wisp/`, or null when it cannot be determined.
  final String? cacheRoot;

  /// Bytes under `<cacheRoot>/.wisp/`. Null until measured, and whenever the
  /// path cannot be walked.
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
      cacheRoot: cacheRoot,
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
    final root = await resolveReceiverCacheRoot(downloadRoot);
    final bytes = root == null ? null : await receiverCacheSizeBytes(root);
    if (generation != _generation) return;
    state = ReceiverCacheState(
      cacheRoot: root,
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

  /// Deletes `<cacheRoot>/.wisp/` and re-measures. Returns the failure text,
  /// or null when it worked.
  Future<String?> clear() async {
    final root = state.cacheRoot;
    if (root == null || state.clearing) return null;
    final generation = _generation;
    state = state.copyWith(clearing: true);

    String? failure;
    try {
      await deleteReceiverCache(root);
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
