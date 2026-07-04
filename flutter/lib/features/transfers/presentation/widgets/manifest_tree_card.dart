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
            ConstrainedBox(
              constraints: const BoxConstraints(maxHeight: 200),
              child: SingleChildScrollView(
                physics: const BouncingScrollPhysics(),
                padding: const EdgeInsets.symmetric(vertical: 4, horizontal: 8),
                child: ManifestTree(items: widget.items),
              ),
            ),
          ],
        ],
      ),
    );
  }
}
