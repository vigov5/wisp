import 'package:flutter/material.dart';
import 'package:app/theme/wisp_theme.dart';
import '../../application/manifest.dart';
import 'manifest_tree.dart';
import 'transfer_presentation_helpers.dart';

class ManifestTreeCard extends StatefulWidget {
  const ManifestTreeCard({
    super.key,
    required this.items,
    this.initiallyExpanded = false,
  });

  final List<TransferManifestItem> items;
  final bool initiallyExpanded;

  @override
  State<ManifestTreeCard> createState() => _ManifestTreeCardState();
}

/// Above this many files the card lists paths flat instead of as a tree.
///
/// Not a rendering budget — the tree culls rows correctly — but an insertion
/// one: animated_tree_view expands a folder by inserting its children one at a
/// time, each with its own 300 ms animation, into a list it re-indexes on every
/// insert. Expanding a 1911-file folder is therefore ~1911 animated insertions
/// over ~18 frames that each lay out a viewport's worth of half-grown rows,
/// which is seconds of frozen UI on the screen that holds Accept. A flat
/// ListView.builder has neither cost, and a tree of hundreds of siblings was
/// not readable as a tree anyway. The card header keeps the exact count and
/// total either way.
const int _maxTreeItems = 250;

class _ManifestTreeCardState extends State<ManifestTreeCard> {
  late bool _isExpanded;

  @override
  void initState() {
    super.initState();
    _isExpanded = widget.initiallyExpanded;
  }

  @override
  Widget build(BuildContext context) {
    final totalSize = widget.items.fold(
      BigInt.zero,
      (sum, item) => sum + item.sizeBytes,
    );
    final summary =
        '${fileCountLabel(widget.items.length)} · ${formatBytes(totalSize)}';

    final isSingleFile = widget.items.length == 1;

    return Container(
      decoration: BoxDecoration(
        color: context.wc.surface,
        borderRadius: BorderRadius.circular(12),
        border: Border.all(color: context.wc.border),
        boxShadow: [
          BoxShadow(
            color: Colors.black.withValues(alpha: 0.03),
            blurRadius: 8,
            offset: const Offset(0, 2),
          ),
        ],
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          InkWell(
            onTap: () {
              setState(() {
                _isExpanded = !_isExpanded;
              });
            },
            borderRadius: BorderRadius.circular(12),
            child: Padding(
              padding: EdgeInsets.symmetric(
                horizontal: 12,
                vertical: isSingleFile ? 10 : 12,
              ),
              child: Row(
                children: [
                  Icon(
                    isSingleFile
                        ? Icons.insert_drive_file_rounded
                        : Icons.copy_all_rounded,
                    color: context.wc.muted,
                    size: 18,
                  ),
                  const SizedBox(width: 12),
                  Expanded(
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        if (!isSingleFile)
                          Text(
                            'Contents',
                            style: wispSans(
                              fontSize: 10,
                              fontWeight: FontWeight.w800,
                              color: context.wc.muted,
                              letterSpacing: 0.4,
                            ),
                          ),
                        Text(
                          isSingleFile
                              ? widget.items.first.path.split('/').last
                              : summary,
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                          style: wispSans(
                            fontSize: 13,
                            fontWeight: isSingleFile
                                ? FontWeight.w600
                                : FontWeight.w700,
                            color: context.wc.ink,
                          ),
                        ),
                        if (isSingleFile)
                          Text(
                            formatBytes(widget.items.first.sizeBytes),
                            style: wispSans(
                              fontSize: 11,
                              fontWeight: FontWeight.w500,
                              color: context.wc.muted,
                            ),
                          ),
                      ],
                    ),
                  ),
                  if (!isSingleFile)
                    Icon(
                      _isExpanded
                          ? Icons.keyboard_arrow_up_rounded
                          : Icons.keyboard_arrow_down_rounded,
                      color: context.wc.subtle,
                      size: 20,
                    ),
                ],
              ),
            ),
          ),
          if (_isExpanded && !isSingleFile) ...[
            const Divider(height: 1),
            // Both branches do their own scrolling inside this bound. The
            // tree used to sit in a SingleChildScrollView, which offers an
            // unbounded height, so the shrink-wrapping list laid out every
            // row just to measure itself — all 1911 of them for a large
            // folder, to fill 200 logical pixels.
            ConstrainedBox(
              constraints: const BoxConstraints(maxHeight: 200),
              child: widget.items.length > _maxTreeItems
                  ? _FlatManifestList(items: widget.items)
                  : ManifestTree(
                      items: widget.items,
                      physics: const BouncingScrollPhysics(),
                      padding: const EdgeInsets.fromLTRB(8, 6, 8, 10),
                    ),
            ),
          ],
        ],
      ),
    );
  }
}

/// A lazily built, flat listing of every path in a large manifest.
///
/// Rows mirror [ManifestTree]'s: a fixed icon slot, the path, and the size
/// right-aligned in the same column width, so the two branches of the card
/// look like one widget.
class _FlatManifestList extends StatelessWidget {
  const _FlatManifestList({required this.items});

  final List<TransferManifestItem> items;

  @override
  Widget build(BuildContext context) {
    return ListView.builder(
      shrinkWrap: true,
      primary: false,
      physics: const BouncingScrollPhysics(),
      padding: const EdgeInsets.fromLTRB(8, 6, 8, 10),
      itemCount: items.length,
      itemExtent: 22,
      itemBuilder: (context, index) {
        final item = items[index];
        return Row(
          children: [
            SizedBox(
              width: 26,
              child: Icon(
                Icons.insert_drive_file_outlined,
                size: 16,
                color: context.wc.muted,
              ),
            ),
            Expanded(
              child: Text(
                item.path,
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                style: wispSans(
                  fontSize: 13,
                  fontWeight: FontWeight.w500,
                  color: context.wc.ink,
                ),
              ),
            ),
            const SizedBox(width: 12),
            SizedBox(
              width: 108,
              child: Text(
                formatBytes(item.sizeBytes),
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
        );
      },
    );
  }
}
