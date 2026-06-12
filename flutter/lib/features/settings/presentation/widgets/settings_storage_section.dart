import 'dart:async';
import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../../../app/app_bootstrap.dart';
import '../../../../theme/wisp_theme.dart';

/// Settings → Storage section.
///
/// Shows the receiver cache size with a Clear button.
///
/// On Android, Rust always writes its temporary cache to
/// `<tmpDir>/Download/Wisp/.wisp/` regardless of the user's download root
/// or SAF configuration, matching the logic in `app_bootstrap.dart`.
/// On other platforms the cache lives at `<downloadRoot>/.wisp/`.
class SettingsStorageSection extends ConsumerStatefulWidget {
  const SettingsStorageSection({super.key, required this.downloadRoot});

  /// Raw value of `settings.downloadRoot` (path or SAF URI).
  final String downloadRoot;

  @override
  ConsumerState<SettingsStorageSection> createState() =>
      _SettingsStorageSectionState();
}

class _SettingsStorageSectionState
    extends ConsumerState<SettingsStorageSection> {
  int? _cacheSizeBytes;
  bool _clearing = false;

  /// The resolved directory that actually contains `.wisp/`.
  /// Null means the path cannot be walked (will show '—').
  String? _effectiveCacheDir;

  @override
  void initState() {
    super.initState();
    _resolveAndRefresh();
  }

  @override
  void didUpdateWidget(covariant SettingsStorageSection oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.downloadRoot != widget.downloadRoot) {
      _resolveAndRefresh();
    }
  }

  Future<void> _resolveAndRefresh() async {
    final dir = await _computeCacheDir();
    if (!mounted) return;
    setState(() => _effectiveCacheDir = dir);
    await _refreshCacheSize();
  }

  /// Returns the directory whose `.wisp/` sub-directory holds the cache,
  /// or null if it cannot be determined (e.g. non-Android SAF URI).
  Future<String?> _computeCacheDir() async {
    // Delegates to the same function used by app_bootstrap.dart so the path
    // is always in sync — no duplicated '/Download/Wisp' suffix here.
    final androidDir = await resolveAndroidReceiveCacheDir();
    if (androidDir != null) return androidDir;
    // SAF URIs are Android-only; on other platforms fall back to downloadRoot.
    if (widget.downloadRoot.startsWith('content://')) return null;
    final root = widget.downloadRoot.trim();
    return root.isEmpty ? null : root;
  }

  Future<void> _refreshCacheSize() async {
    final dir = _effectiveCacheDir;
    if (dir == null) {
      setState(() => _cacheSizeBytes = null);
      return;
    }
    final size = await _walkDirSize(
      Directory('$dir${Platform.pathSeparator}.wisp'),
    );
    if (!mounted) return;
    setState(() => _cacheSizeBytes = size);
  }

  Future<void> _clearCache() async {
    if (_clearing) return;
    final dir = _effectiveCacheDir;
    if (dir == null) return;
    setState(() => _clearing = true);
    final cacheDir = Directory('$dir${Platform.pathSeparator}.wisp');
    try {
      if (await cacheDir.exists()) {
        await cacheDir.delete(recursive: true);
      }
    } catch (e) {
      if (mounted) {
        ScaffoldMessenger.of(
          context,
        ).showSnackBar(SnackBar(content: Text('Couldn\'t clear cache: $e')));
      }
    } finally {
      if (mounted) {
        setState(() {
          _clearing = false;
          _cacheSizeBytes = 0;
        });
        ScaffoldMessenger.of(
          context,
        ).showSnackBar(const SnackBar(content: Text('Receiver cache cleared')));
      }
    }
  }

  @override
  Widget build(BuildContext context) {
    final canClear =
        _effectiveCacheDir != null && (_cacheSizeBytes ?? 0) > 0 && !_clearing;
    final sizeText = _effectiveCacheDir == null
        ? '—'
        : (_cacheSizeBytes == null ? '…' : _formatBytes(_cacheSizeBytes!));

    return Row(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Expanded(
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Text(
                'Storage',
                style: wispSans(
                  fontSize: 13.5,
                  fontWeight: FontWeight.w600,
                  color: kInk,
                ),
              ),
              const SizedBox(height: 4),
              Text.rich(
                TextSpan(
                  children: [
                    TextSpan(
                      text: 'Received Cache: ',
                      style: wispSans(fontSize: 11.5, color: kMuted),
                    ),
                    TextSpan(
                      text: sizeText,
                      style: wispSans(
                        fontSize: 11.5,
                        fontWeight: FontWeight.w500,
                        color: kInk,
                      ),
                    ),
                  ],
                ),
              ),
            ],
          ),
        ),
        const SizedBox(width: 12),
        Padding(
          padding: const EdgeInsets.only(top: 2),
          child: TextButton.icon(
            onPressed: canClear ? _clearCache : null,
            icon: const Icon(Icons.cleaning_services_rounded, size: 18),
            label: Text(
              _clearing ? 'Clearing…' : 'Clear',
              style: wispSans(fontSize: 13, fontWeight: FontWeight.w500),
            ),
            style: TextButton.styleFrom(
              foregroundColor: kAccentCyan,
              disabledForegroundColor: kSubtle,
            ),
          ),
        ),
      ],
    );
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

String _formatBytes(int bytes) {
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
