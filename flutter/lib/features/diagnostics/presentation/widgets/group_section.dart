import 'package:flutter/material.dart';

import '../../../../theme/drift_theme.dart';
import '../../domain/check_result.dart';
import 'check_row.dart';

class GroupSection extends StatefulWidget {
  const GroupSection({
    super.key,
    required this.group,
    required this.groupStatus,
    required this.results,
    this.onAction,
  });

  final CheckGroup group;
  final CheckStatus groupStatus;
  final List<CheckResult> results;
  final void Function(CheckAction action)? onAction;

  @override
  State<GroupSection> createState() => _GroupSectionState();
}

class _GroupSectionState extends State<GroupSection> {
  bool? _expandedOverride;

  bool get _expanded {
    final override = _expandedOverride;
    if (override != null) return override;
    return widget.groupStatus == CheckStatus.fail ||
        widget.groupStatus == CheckStatus.warn ||
        widget.groupStatus == CheckStatus.running;
  }

  @override
  Widget build(BuildContext context) {
    return Container(
      decoration: BoxDecoration(
        color: kSurface,
        borderRadius: BorderRadius.circular(12),
        border: Border.all(color: kBorder),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          InkWell(
            onTap: () => setState(() => _expandedOverride = !_expanded),
            borderRadius: BorderRadius.circular(12),
            child: Padding(
              padding:
                  const EdgeInsets.symmetric(horizontal: 14, vertical: 12),
              child: Row(
                children: [
                  _GroupStatusDot(status: widget.groupStatus),
                  const SizedBox(width: 10),
                  Expanded(
                    child: Text(
                      widget.group.label,
                      style: driftSans(
                        fontSize: 14,
                        fontWeight: FontWeight.w700,
                        color: kInk,
                      ),
                    ),
                  ),
                  Icon(
                    _expanded
                        ? Icons.keyboard_arrow_up_rounded
                        : Icons.keyboard_arrow_down_rounded,
                    color: kMuted,
                  ),
                ],
              ),
            ),
          ),
          if (_expanded) ...[
            const Divider(height: 1, color: kBorder),
            Padding(
              padding:
                  const EdgeInsets.symmetric(horizontal: 10, vertical: 4),
              child: Column(
                children: [
                  for (final r in widget.results)
                    CheckRow(result: r, onAction: widget.onAction),
                ],
              ),
            ),
          ],
        ],
      ),
    );
  }
}

class _GroupStatusDot extends StatelessWidget {
  const _GroupStatusDot({required this.status});

  final CheckStatus status;

  @override
  Widget build(BuildContext context) {
    if (status == CheckStatus.running) {
      return SizedBox(
        width: 14,
        height: 14,
        child: CircularProgressIndicator(strokeWidth: 2, color: status.color),
      );
    }
    return Container(
      width: 10,
      height: 10,
      decoration: BoxDecoration(color: status.color, shape: BoxShape.circle),
    );
  }
}
