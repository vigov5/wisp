import 'dart:async';

import 'package:flutter/material.dart';

import '../../../theme/wisp_theme.dart';

Future<void> showSendSelectionSourceSheet(
  BuildContext context, {
  required FutureOr<void> Function() onChooseFiles,
  required FutureOr<void> Function() onChooseFolder,
  FutureOr<void> Function()? onChooseText,
  FutureOr<void> Function()? onChooseClipboard,
}) {
  return showModalBottomSheet<void>(
    context: context,
    backgroundColor: Colors.transparent,
    builder: (context) {
      return SendSelectionSourceSheet(
        onChooseFiles: onChooseFiles,
        onChooseFolder: onChooseFolder,
        onChooseText: onChooseText,
        onChooseClipboard: onChooseClipboard,
      );
    },
  );
}

class SendSelectionSourceSheet extends StatelessWidget {
  const SendSelectionSourceSheet({
    super.key,
    required this.onChooseFiles,
    required this.onChooseFolder,
    this.onChooseText,
    this.onChooseClipboard,
  });

  final FutureOr<void> Function() onChooseFiles;
  final FutureOr<void> Function() onChooseFolder;
  final FutureOr<void> Function()? onChooseText;
  final FutureOr<void> Function()? onChooseClipboard;

  void _handleSelection(
    BuildContext context,
    FutureOr<void> Function() callback,
  ) {
    Navigator.of(context).pop();
    unawaited(Future<void>.sync(callback));
  }

  @override
  Widget build(BuildContext context) {
    return SafeArea(
      top: false,
      child: Padding(
        padding: const EdgeInsets.fromLTRB(16, 0, 16, 16),
        child: DecoratedBox(
          decoration: BoxDecoration(
            color: context.wc.surface,
            borderRadius: BorderRadius.circular(24),
            border: Border.all(color: context.wc.border),
            boxShadow: const [
              BoxShadow(
                color: Color(0x14000000),
                blurRadius: 24,
                offset: Offset(0, 14),
              ),
            ],
          ),
          child: Padding(
            padding: const EdgeInsets.fromLTRB(8, 10, 8, 8),
            child: Column(
              mainAxisSize: MainAxisSize.min,
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                Padding(
                  padding: const EdgeInsets.fromLTRB(12, 4, 12, 8),
                  child: Text(
                    'Select from',
                    style: wispSans(
                      fontSize: 13,
                      fontWeight: FontWeight.w700,
                      color: context.wc.muted,
                      letterSpacing: 0.1,
                    ),
                  ),
                ),
                _SelectionActionTile(
                  icon: Icons.insert_drive_file_outlined,
                  label: 'Files',
                  onTap: () => _handleSelection(context, onChooseFiles),
                ),
                _SelectionActionTile(
                  icon: Icons.folder_outlined,
                  label: 'Folder',
                  onTap: () => _handleSelection(context, onChooseFolder),
                ),
                if (onChooseText != null)
                  _SelectionActionTile(
                    icon: Icons.notes_rounded,
                    label: 'Text',
                    onTap: () => _handleSelection(context, onChooseText!),
                  ),
                if (onChooseClipboard != null)
                  _SelectionActionTile(
                    icon: Icons.content_paste_rounded,
                    label: 'Clipboard',
                    onTap: () => _handleSelection(context, onChooseClipboard!),
                  ),
              ],
            ),
          ),
        ),
      ),
    );
  }
}

class _SelectionActionTile extends StatelessWidget {
  const _SelectionActionTile({
    required this.icon,
    required this.label,
    required this.onTap,
  });

  final IconData icon;
  final String label;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    // The sheet wraps these tiles in a DecoratedBox with a background color,
    // which would otherwise mask the ListTile's ink splash. A transparent
    // Material satisfies the "Material ancestor" requirement without
    // overriding the parent's appearance.
    return Material(
      color: Colors.transparent,
      child: ListTile(
        shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(16)),
        leading: Icon(icon, color: context.wc.ink),
        title: Text(
          label,
          style: wispSans(
            fontSize: 15,
            fontWeight: FontWeight.w600,
            color: context.wc.ink,
          ),
        ),
        onTap: onTap,
      ),
    );
  }
}
