import 'package:flutter/material.dart';

import '../../../../theme/wisp_theme.dart';
import '../../domain/check_result.dart';

class CheckRow extends StatefulWidget {
  const CheckRow({super.key, required this.result, this.onAction});

  final CheckResult result;
  final void Function(CheckAction action)? onAction;

  @override
  State<CheckRow> createState() => _CheckRowState();
}

class _CheckRowState extends State<CheckRow> {
  bool _expanded = false;

  bool get _isExpandable {
    final r = widget.result;
    return r.hint != null || r.action != null;
  }

  @override
  Widget build(BuildContext context) {
    final r = widget.result;
    return InkWell(
      onTap: _isExpandable ? () => setState(() => _expanded = !_expanded) : null,
      borderRadius: BorderRadius.circular(8),
      child: Padding(
        padding: const EdgeInsets.symmetric(horizontal: 4, vertical: 8),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Row(
              crossAxisAlignment: CrossAxisAlignment.center,
              children: [
                _StatusIcon(status: r.status),
                const SizedBox(width: 10),
                Expanded(
                  child: Text(
                    r.label,
                    style: wispSans(
                      fontSize: 13.5,
                      fontWeight: FontWeight.w500,
                      color: r.status == CheckStatus.skipped ? kMuted : kInk,
                    ),
                  ),
                ),
                if (_isExpandable)
                  Padding(
                    padding: const EdgeInsets.only(left: 6),
                    child: Icon(
                      _expanded
                          ? Icons.keyboard_arrow_up_rounded
                          : Icons.keyboard_arrow_down_rounded,
                      size: 18,
                      color: kMuted,
                    ),
                  ),
              ],
            ),
            if (r.detail.isNotEmpty)
              Padding(
                padding: const EdgeInsets.only(left: 28, top: 2, right: 4),
                child: Text(
                  r.detail,
                  style: wispSans(
                    fontSize: 12,
                    fontWeight: FontWeight.w400,
                    color: kMuted,
                    height: 1.4,
                  ),
                  maxLines: _expanded ? 8 : 2,
                  overflow: TextOverflow.ellipsis,
                  softWrap: true,
                ),
              ),
            if (_expanded && _isExpandable) _expandedContent(r),
          ],
        ),
      ),
    );
  }

  Widget _expandedContent(CheckResult r) {
    return Padding(
      padding: const EdgeInsets.only(left: 28, top: 8, right: 4, bottom: 4),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          if (r.hint != null)
            Text(
              r.hint!,
              style: wispSans(
                fontSize: 12.5,
                fontWeight: FontWeight.w400,
                color: kInk,
                height: 1.45,
              ),
              softWrap: true,
            ),
          if (r.action != null) ...[
            const SizedBox(height: 10),
            OutlinedButton(
              onPressed: () => widget.onAction?.call(r.action!),
              child: Text(r.action!.label),
            ),
          ],
        ],
      ),
    );
  }
}

class _StatusIcon extends StatelessWidget {
  const _StatusIcon({required this.status});

  final CheckStatus status;

  @override
  Widget build(BuildContext context) {
    if (status == CheckStatus.running) {
      return SizedBox(
        width: 18,
        height: 18,
        child: CircularProgressIndicator(strokeWidth: 2, color: status.color),
      );
    }
    return Icon(status.icon ?? Icons.help_outline_rounded,
        color: status.color, size: 18);
  }
}
