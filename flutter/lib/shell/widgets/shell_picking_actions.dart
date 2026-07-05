import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../app/app_router.dart';
import '../../features/send/application/controller.dart';
import '../../features/send/application/model.dart';
import '../../features/send/application/send_selection_picker.dart';
import '../../features/send/presentation/pick_progress_dialog.dart';
import '../../features/send/presentation/send_text_editor_page.dart';
import '../../features/settings/application/controller.dart';
import '../../platform/android_file_picker.dart';

mixin ShellPickingActions {
  Future<void> openSelectedFiles(
    BuildContext context,
    List<SendPickedFile> files,
  ) async {
    context.goSendDraft(files: files);
  }

  /// Start a text draft, then jump to the draft/destination screen.  Passing
  /// no files keeps the text draft the controller just set (the draft route
  /// only re-seeds when files are present).
  void _openTextDraft(BuildContext context, WidgetRef ref, String text) {
    ref.read(sendControllerProvider.notifier).beginTextDraft(text);
    context.goSendDraft(files: const []);
  }

  /// "Share text": open a blank editor; on continue, start a text draft.
  Future<void> shareText(BuildContext context, WidgetRef ref) async {
    await Navigator.of(context).push(
      MaterialPageRoute<void>(
        builder: (_) => SendTextEditorPage(
          title: 'Share text',
          onSubmit: (editorContext, text) {
            Navigator.of(editorContext).pop();
            _openTextDraft(context, ref, text);
          },
        ),
      ),
    );
  }

  /// "Share clipboard": grab the clipboard text.  Unless the user opted out in
  /// Settings, open the editor pre-filled so they can confirm before sending.
  Future<void> shareClipboard(BuildContext context, WidgetRef ref) async {
    final text = await readClipboardText();
    if (!context.mounted) {
      return;
    }
    if (text == null) {
      ScaffoldMessenger.of(
        context,
      ).showSnackBar(const SnackBar(content: Text('Clipboard is empty')));
      return;
    }

    final skipConfirm = ref
        .read(settingsControllerProvider)
        .settings
        .skipClipboardConfirm;
    if (skipConfirm) {
      _openTextDraft(context, ref, text);
      return;
    }

    await Navigator.of(context).push(
      MaterialPageRoute<void>(
        builder: (_) => SendTextEditorPage(
          title: 'Share clipboard',
          initialText: text,
          showClipboardHint: true,
          onSubmit: (editorContext, edited) {
            Navigator.of(editorContext).pop();
            _openTextDraft(context, ref, edited);
          },
        ),
      ),
    );
  }

  Future<void> pickSelection(
    BuildContext context,
    WidgetRef ref,
    Future<List<SendPickedFile>> Function(SendSelectionPicker picker) pick,
  ) async {
    final pickerService = ref.read(sendSelectionPickerProvider);
    final files = await _withPickProgress(context, () => pick(pickerService));
    if (files.isEmpty) {
      return;
    }

    if (!context.mounted) {
      return;
    }

    await openSelectedFiles(context, files);
  }

  /// Runs [run] (a file/folder pick) and, on Android, shows a modal progress
  /// dialog once the native copy actually starts streaming bytes. Small picks
  /// that copy instantly never emit progress, so no dialog flashes for them.
  Future<List<SendPickedFile>> _withPickProgress(
    BuildContext context,
    Future<List<SendPickedFile>> Function() run,
  ) async {
    if (!Platform.isAndroid) {
      return run();
    }

    final future = run();
    final notifier = AndroidFilePicker.pickProgress;
    var dialogShown = false;

    void maybeShow() {
      if (dialogShown || notifier.value == null || !context.mounted) {
        return;
      }
      dialogShown = true;
      showDialog<void>(
        context: context,
        barrierDismissible: false,
        builder: (_) => const PickProgressDialog(),
      );
    }

    notifier.addListener(maybeShow);
    maybeShow();
    try {
      return await future;
    } finally {
      notifier.removeListener(maybeShow);
      if (dialogShown && context.mounted) {
        Navigator.of(context, rootNavigator: true).pop();
      }
    }
  }

  Future<void> pickFiles(BuildContext context, WidgetRef ref) {
    return pickSelection(context, ref, (picker) => picker.pickFiles());
  }

  Future<void> pickFolder(BuildContext context, WidgetRef ref) {
    return pickSelection(context, ref, (picker) => picker.pickFolder());
  }
}
