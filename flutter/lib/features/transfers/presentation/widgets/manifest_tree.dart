import 'dart:convert';

import 'package:animated_tree_view/animated_tree_view.dart';
import 'package:flutter/material.dart';

import '../../../../theme/wisp_theme.dart';
import '../../application/manifest.dart';
import 'transfer_presentation_helpers.dart';

/// The folder tree behind a transfer's "Contents".
///
/// Stateful only to keep the built tree between rebuilds. `_buildTree` walks
/// every item and allocates a node — plus a base64 key — per path segment, and
/// the parents of this widget rebuild on every event they receive. Rebuilding
/// the tree there also discarded whatever the user had expanded, since the
/// expansion state lives on the nodes.
class ManifestTree extends StatefulWidget {
  const ManifestTree({
    super.key,
    required this.items,
    this.physics = const NeverScrollableScrollPhysics(),
    this.padding = const EdgeInsets.only(top: 2, bottom: 6),
  });

  final List<TransferManifestItem> items;

  /// Defaults to "never scrolls" for a caller that nests the tree in its own
  /// scroll view. A caller that can give it a **bounded** height should pass
  /// real physics instead and let it scroll itself: the shrink-wrapping
  /// viewport then lays out only the rows that fit. Inside an unbounded parent
  /// it lays out all of them, which on a 1911-file folder is ~15k widgets in
  /// one frame — several seconds of frozen UI, on the very screen that holds
  /// the Accept button.
  final ScrollPhysics physics;

  final EdgeInsetsGeometry padding;

  @override
  State<ManifestTree> createState() => _ManifestTreeState();
}

class _ManifestTreeState extends State<ManifestTree> {
  late TreeNode<_ManifestNodeData> _tree;

  @override
  void initState() {
    super.initState();
    _tree = _buildTree(widget.items);
  }

  @override
  void didUpdateWidget(ManifestTree oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (!_sameManifest(oldWidget.items, widget.items)) {
      _tree = _buildTree(widget.items);
    }
  }

  @override
  Widget build(BuildContext context) {
    if (_tree.children.isEmpty) {
      return Padding(
        padding: const EdgeInsets.symmetric(vertical: 12),
        child: Text('No files', style: Theme.of(context).textTheme.bodyMedium),
      );
    }

    return TreeView.simpleTyped<_ManifestNodeData, TreeNode<_ManifestNodeData>>(
      tree: _tree,
      showRootNode: false,
      shrinkWrap: true,
      primary: false,
      physics: widget.physics,
      focusToNewNode: false,
      expansionBehavior: ExpansionBehavior.none,
      expansionIndicatorBuilder: noExpansionIndicatorBuilder,
      padding: widget.padding,
      indentation: Indentation(
        width: 10,
        style: IndentStyle.squareJoint,
        thickness: 1,
        color: context.wc.border.withValues(alpha: 0.75),
      ),
      onTreeReady: (controller) {
        // Only expand the top-level items by default.
        controller.expandAllChildren(controller.tree, recursive: false);
      },
      builder: (context, node) {
        final data = node.data!;
        final isTopLevel = node.level == 1;

        return InkWell(
          onTap: data.isFolder
              ? () => node.expansionNotifier.value = !node.isExpanded
              : null,
          borderRadius: BorderRadius.circular(4),
          child: Padding(
            padding: EdgeInsets.symmetric(
              horizontal: 4,
              vertical: isTopLevel ? 4 : 2.5,
            ),
            child: Row(
              children: [
                SizedBox(
                  width: 26,
                  child: Icon(
                    data.isFolder
                        ? (isTopLevel
                              ? Icons.folder_rounded
                              : Icons.folder_outlined)
                        : Icons.insert_drive_file_outlined,
                    size: 18,
                    color: data.isFolder
                        ? (isTopLevel ? context.wc.muted : context.wc.subtle)
                        : context.wc.muted,
                  ),
                ),
                Expanded(
                  child: Tooltip(
                    message: data.fullPath,
                    child: Text(
                      data.label,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: wispSans(
                        fontSize: isTopLevel && data.isFolder
                            ? 14
                            : (data.isFolder ? 13.5 : 13),
                        fontWeight: isTopLevel && data.isFolder
                            ? FontWeight.w700
                            : (data.isFolder
                                  ? FontWeight.w600
                                  : FontWeight.w500),
                        color: context.wc.ink,
                      ),
                    ),
                  ),
                ),
                const SizedBox(width: 12),
                SizedBox(
                  width: 108,
                  child: Text(
                    formatBytes(data.sizeBytes),
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    textAlign: TextAlign.right,
                    style: wispSans(
                      fontSize: 12,
                      fontWeight: FontWeight.w500,
                      color: context.wc.muted,
                    ),
                  ),
                ),
              ],
            ),
          ),
        );
      },
    );
  }
}

class _ManifestNodeData {
  _ManifestNodeData.folder({
    required this.label,
    required this.fullPath,
    required this.sizeBytes,
  }) : isFolder = true;

  _ManifestNodeData.file({
    required this.label,
    required this.fullPath,
    required this.sizeBytes,
  }) : isFolder = false;

  final String label;
  final String fullPath;
  final bool isFolder;
  BigInt sizeBytes;
}

class _PathEntry {
  const _PathEntry({
    required this.segments,
    required this.sizeBytes,
    required this.fullPath,
  });

  final List<String> segments;
  final BigInt sizeBytes;
  final String fullPath;
}

/// Only paths and sizes shape the tree, so nothing else is worth comparing.
/// Identity alone would never match: the callers rebuild their item list from
/// scratch on every event.
bool _sameManifest(
  List<TransferManifestItem> before,
  List<TransferManifestItem> after,
) {
  if (identical(before, after)) return true;
  if (before.length != after.length) return false;
  for (var i = 0; i < before.length; i++) {
    if (before[i].path != after[i].path ||
        before[i].sizeBytes != after[i].sizeBytes) {
      return false;
    }
  }
  return true;
}

TreeNode<_ManifestNodeData> _buildTree(List<TransferManifestItem> items) {
  final root = TreeNode<_ManifestNodeData>.root(
    data: _ManifestNodeData.folder(
      label: '',
      fullPath: '',
      sizeBytes: BigInt.zero,
    ),
  );

  final entries = items
      .map(
        (item) => _PathEntry(
          segments: item.path
              .split('/')
              .where((segment) => segment.isNotEmpty)
              .toList(growable: false),
          sizeBytes: item.sizeBytes,
          fullPath: item.path,
        ),
      )
      .where((entry) => entry.segments.isNotEmpty)
      .toList(growable: false);

  root.addAll(_buildChildren(entries, prefix: const []));
  final rootChildren = root.children.values.cast<TreeNode<_ManifestNodeData>>();
  root.data!.sizeBytes = rootChildren.fold<BigInt>(
    BigInt.zero,
    (sum, node) => sum + node.data!.sizeBytes,
  );
  return root;
}

List<TreeNode<_ManifestNodeData>> _buildChildren(
  List<_PathEntry> entries, {
  required List<String> prefix,
}) {
  final folderGroups = <String, List<_PathEntry>>{};
  final fileEntries = <_PathEntry>[];

  for (final entry in entries) {
    final remaining = entry.segments
        .skip(prefix.length)
        .toList(growable: false);
    if (remaining.isEmpty) {
      continue;
    }

    if (remaining.length == 1) {
      fileEntries.add(entry);
    } else {
      folderGroups.putIfAbsent(remaining.first, () => []).add(entry);
    }
  }

  final children = <TreeNode<_ManifestNodeData>>[];

  final folderNames = folderGroups.keys.toList()..sort();
  for (final folderName in folderNames) {
    final childPrefix = [...prefix, folderName];
    final childEntries = folderGroups[folderName]!;
    final childNode = TreeNode<_ManifestNodeData>(
      key: _safeKey(childPrefix.join('/')),
      data: _ManifestNodeData.folder(
        label: folderName,
        fullPath: childPrefix.join('/'),
        sizeBytes: BigInt.zero,
      ),
    );
    final grandChildren = _buildChildren(childEntries, prefix: childPrefix);
    childNode.addAll(grandChildren);
    childNode.data!.sizeBytes = grandChildren.fold<BigInt>(
      BigInt.zero,
      (sum, node) => sum + node.data!.sizeBytes,
    );
    children.add(childNode);
  }

  fileEntries.sort((left, right) => left.fullPath.compareTo(right.fullPath));
  for (final entry in fileEntries) {
    final label = entry.segments.last;
    children.add(
      TreeNode<_ManifestNodeData>(
        key: _safeKey(entry.fullPath),
        data: _ManifestNodeData.file(
          label: label,
          fullPath: entry.fullPath,
          sizeBytes: entry.sizeBytes,
        ),
      ),
    );
  }

  return children;
}

String _safeKey(String value) => base64Url.encode(utf8.encode(value));
