import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../../../theme/wisp_theme.dart';
import '../../application/receiver_cache.dart';

/// Settings → Storage section.
///
/// Shows the receiver cache size with a Clear button. The path, the walk and
/// the delete come from `receiver_cache.dart`, which the startup warning above
/// the footer also reads.
///
/// The size shown follows the *draft* download root — the folder currently in
/// the form — so picking a different one updates it before Save. The shared
/// notifier tracks the saved root instead, and is refreshed after a clear.
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
    final dir = await resolveReceiverCacheRoot(widget.downloadRoot);
    if (!mounted) return;
    setState(() => _effectiveCacheDir = dir);
    await _refreshCacheSize();
  }

  Future<void> _refreshCacheSize() async {
    final dir = _effectiveCacheDir;
    if (dir == null) {
      setState(() => _cacheSizeBytes = null);
      return;
    }
    final size = await receiverCacheSizeBytes(dir);
    if (!mounted) return;
    setState(() => _cacheSizeBytes = size);
  }

  Future<void> _clearCache() async {
    if (_clearing) return;
    final dir = _effectiveCacheDir;
    if (dir == null) return;
    setState(() => _clearing = true);
    try {
      await deleteReceiverCache(dir);
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
      // The startup warning measures the saved root on its own; tell it the
      // bytes are gone so it doesn't keep warning about a cache that isn't
      // there any more.
      unawaited(ref.read(receiverCacheProvider.notifier).refresh());
    }
  }

  @override
  Widget build(BuildContext context) {
    final canClear =
        _effectiveCacheDir != null && (_cacheSizeBytes ?? 0) > 0 && !_clearing;
    final sizeText = _effectiveCacheDir == null
        ? '—'
        : (_cacheSizeBytes == null ? '…' : formatCacheBytes(_cacheSizeBytes!));

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
                  color: context.wc.ink,
                ),
              ),
              const SizedBox(height: 4),
              Text.rich(
                TextSpan(
                  children: [
                    TextSpan(
                      text: 'Received Cache: ',
                      style: wispSans(fontSize: 11.5, color: context.wc.muted),
                    ),
                    TextSpan(
                      text: sizeText,
                      style: wispSans(
                        fontSize: 11.5,
                        fontWeight: FontWeight.w500,
                        color: context.wc.ink,
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
              disabledForegroundColor: context.wc.subtle,
            ),
          ),
        ),
      ],
    );
  }
}
