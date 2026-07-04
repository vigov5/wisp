import 'package:flutter/material.dart';

import '../../../../theme/wisp_theme.dart';
import '../../application/manifest.dart';
import 'manifest_tree.dart';

class PreviewTable extends StatelessWidget {
  const PreviewTable({
    super.key,
    required this.items,
    required this.footerSummary,
  });

  final List<TransferManifestItem> items;
  final String footerSummary;

  Widget _divider(BuildContext context) => Divider(
    height: 1,
    thickness: 1,
    color: context.wc.border.withValues(alpha: 0.55),
  );

  @override
  Widget build(BuildContext context) {
    if (items.isEmpty) {
      return Padding(
        padding: const EdgeInsets.symmetric(vertical: 12),
        child: Text('No files', style: Theme.of(context).textTheme.bodyMedium),
      );
    }

    final headerStyle = wispSans(
      fontSize: 11,
      fontWeight: FontWeight.w700,
      color: context.wc.ink.withValues(alpha: 0.8),
      letterSpacing: 0.15,
    );

    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Padding(
          padding: const EdgeInsets.only(top: 4, bottom: 8),
          child: Row(
            crossAxisAlignment: CrossAxisAlignment.baseline,
            textBaseline: TextBaseline.alphabetic,
            children: [
              const SizedBox(width: 28),
              Expanded(child: Text('Name', style: headerStyle)),
              SizedBox(
                width: 76,
                child: Text(
                  'Size',
                  textAlign: TextAlign.right,
                  style: headerStyle,
                ),
              ),
            ],
          ),
        ),
        _divider(context),
        const SizedBox(height: 12),
        Padding(
          padding: const EdgeInsets.symmetric(horizontal: 10),
          child: ManifestTree(items: items),
        ),
        if (items.length > 1) ...[
          _divider(context),
          Padding(
            padding: const EdgeInsets.only(top: 10, bottom: 4),
            child: Row(
              children: [
                const SizedBox(width: 24),
                Expanded(
                  child: Text(
                    footerSummary,
                    textAlign: TextAlign.right,
                    maxLines: 2,
                    overflow: TextOverflow.ellipsis,
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
        ],
      ],
    );
  }
}
