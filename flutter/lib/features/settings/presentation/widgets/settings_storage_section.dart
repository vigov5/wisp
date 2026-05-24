import 'dart:async';
import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../../../app/app_router.dart';
import '../../../../theme/wisp_theme.dart';
import '../../../saved_devices/application/saved_devices_controller.dart';
import 'settings_section_field.dart';

/// Settings → Storage section.
///
/// Surfaces three figures the user might want at a glance + an action
/// per row:
///
/// - **Receiver cache** — total size of `<downloadRoot>/.wisp/`
///   (per-transfer resume state from completed-and-not-GC'd or
///   failed-pending-retry transfers).  Has a "Clear" button that
///   deletes the directory.
/// - **Saved devices** — count from [savedDevicesProvider] with a
///   chevron to the existing manage page.
/// - **Download folder** — read-only display of where files land.
///
/// On Android with a SAF folder selected, dart:io can't walk the
/// content:// URI, so the cache row shows "—" with a note pointing
/// at device Settings → Apps → Wisp → Storage.
class SettingsStorageSection extends ConsumerStatefulWidget {
  const SettingsStorageSection({
    super.key,
    required this.downloadRoot,
    required this.downloadFolderLabel,
  });

  /// Raw value of `settings.downloadRoot` (path or SAF URI).
  final String downloadRoot;

  /// User-facing label for the download folder (already formatted via
  /// `_downloadRootDisplayText`).
  final String downloadFolderLabel;

  @override
  ConsumerState<SettingsStorageSection> createState() =>
      _SettingsStorageSectionState();
}

class _SettingsStorageSectionState
    extends ConsumerState<SettingsStorageSection> {
  int? _cacheSizeBytes;
  bool _clearing = false;

  @override
  void initState() {
    super.initState();
    _refreshCacheSize();
  }

  @override
  void didUpdateWidget(covariant SettingsStorageSection oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.downloadRoot != widget.downloadRoot) {
      _refreshCacheSize();
    }
  }

  bool get _isSafUri => widget.downloadRoot.startsWith('content://');

  Future<void> _refreshCacheSize() async {
    if (_isSafUri) {
      setState(() => _cacheSizeBytes = null);
      return;
    }
    final root = widget.downloadRoot.trim();
    if (root.isEmpty) {
      setState(() => _cacheSizeBytes = 0);
      return;
    }
    final size = await _walkDirSize(
      Directory('$root${Platform.pathSeparator}.wisp'),
    );
    if (!mounted) return;
    setState(() => _cacheSizeBytes = size);
  }

  Future<void> _clearCache() async {
    if (_clearing || _isSafUri) return;
    setState(() => _clearing = true);
    final dir = Directory(
      '${widget.downloadRoot}${Platform.pathSeparator}.wisp',
    );
    try {
      if (await dir.exists()) {
        await dir.delete(recursive: true);
      }
    } catch (e) {
      if (mounted) {
        ScaffoldMessenger.of(context).showSnackBar(
          SnackBar(content: Text('Couldn\'t clear cache: $e')),
        );
      }
    } finally {
      if (mounted) {
        setState(() {
          _clearing = false;
          _cacheSizeBytes = 0;
        });
        ScaffoldMessenger.of(context).showSnackBar(
          const SnackBar(content: Text('Receiver cache cleared')),
        );
      }
    }
  }

  @override
  Widget build(BuildContext context) {
    final savedDevices = ref.watch(savedDevicesProvider);
    final savedCount = savedDevices.length;

    return SettingsSectionField(
      label: 'Storage',
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          _StorageRow(
            label: 'Receiver cache',
            valueText: _cacheSizeText(),
            trailing: _isSafUri
                ? null
                : TextButton(
                    onPressed: (_cacheSizeBytes ?? 0) > 0 && !_clearing
                        ? _clearCache
                        : null,
                    child: Text(_clearing ? 'Clearing…' : 'Clear'),
                  ),
            footnote: _isSafUri
                ? 'Files land in your chosen SAF folder via the system '
                      'picker. Clear via device Settings → Apps → Wisp '
                      '→ Storage.'
                : null,
          ),
          const SizedBox(height: 10),
          _StorageRow(
            label: 'Saved devices',
            valueText: savedCount.toString(),
            trailing: IconButton(
              icon: const Icon(Icons.chevron_right_rounded, color: kMuted),
              tooltip: 'Manage',
              onPressed: () => context.pushSavedDevices(),
            ),
          ),
          const SizedBox(height: 10),
          _StorageRow(
            label: 'Download folder',
            valueText: widget.downloadFolderLabel,
            trailing: null,
          ),
        ],
      ),
    );
  }

  String _cacheSizeText() {
    if (_isSafUri) return '—';
    final bytes = _cacheSizeBytes;
    if (bytes == null) return '…';
    return _formatBytes(bytes);
  }
}

class _StorageRow extends StatelessWidget {
  const _StorageRow({
    required this.label,
    required this.valueText,
    this.trailing,
    this.footnote,
  });

  final String label;
  final String valueText;
  final Widget? trailing;
  final String? footnote;

  @override
  Widget build(BuildContext context) {
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 14, vertical: 12),
      decoration: BoxDecoration(
        color: kSurface,
        borderRadius: BorderRadius.circular(12),
        border: Border.all(color: kBorder),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Row(
            children: [
              Expanded(
                child: Text(
                  label,
                  style: wispSans(
                    fontSize: 13,
                    fontWeight: FontWeight.w500,
                    color: kInk,
                  ),
                ),
              ),
              Text(
                valueText,
                style: wispMono(fontSize: 13, color: kInk),
              ),
              if (trailing != null) ...[
                const SizedBox(width: 4),
                trailing!,
              ],
            ],
          ),
          if (footnote != null) ...[
            const SizedBox(height: 6),
            Text(
              footnote!,
              style: wispSans(
                fontSize: 11.5,
                color: kMuted,
                height: 1.45,
              ),
            ),
          ],
        ],
      ),
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
  final fixed = value < 10 ? value.toStringAsFixed(1) : value.toStringAsFixed(0);
  return '$fixed ${units[idx]}';
}
