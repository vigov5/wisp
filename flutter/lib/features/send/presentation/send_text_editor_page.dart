import 'dart:convert';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../../../theme/wisp_theme.dart';

/// Inline-text cap mirrored from `wisp_core::protocol::message::INLINE_TEXT_MAX_BYTES`.
/// Text at or under this rides inline; larger text is sent as a `.txt` file.
const int _inlineTextMaxBytes = 16 * 1024;

/// Screen for composing (or reviewing) a text snippet before sending it.
/// Reused by "Share text" (blank) and "Share clipboard" (pre-filled, with a
/// confirm hint). On continue it hands the trimmed text back via [onSubmit].
class SendTextEditorPage extends StatefulWidget {
  const SendTextEditorPage({
    super.key,
    this.initialText = '',
    this.showClipboardHint = false,
    this.title = 'Share text',
    this.continueLabel = 'Next',
    required this.onSubmit,
  });

  final String initialText;
  final bool showClipboardHint;
  final String title;
  final String continueLabel;
  final void Function(BuildContext context, String text) onSubmit;

  @override
  State<SendTextEditorPage> createState() => _SendTextEditorPageState();
}

class _SendTextEditorPageState extends State<SendTextEditorPage> {
  late final TextEditingController _controller;

  @override
  void initState() {
    super.initState();
    _controller = TextEditingController(text: widget.initialText);
    _controller.addListener(_onChanged);
  }

  @override
  void dispose() {
    _controller.removeListener(_onChanged);
    _controller.dispose();
    super.dispose();
  }

  void _onChanged() => setState(() {});

  void _submit() {
    final text = _controller.text;
    if (text.trim().isEmpty) {
      return;
    }
    widget.onSubmit(context, text);
  }

  @override
  Widget build(BuildContext context) {
    final byteLen = utf8.encode(_controller.text).length;
    final canContinue = _controller.text.trim().isNotEmpty;
    final willFallback = byteLen > _inlineTextMaxBytes;

    return Scaffold(
      backgroundColor: context.wc.bg,
      appBar: AppBar(
        backgroundColor: context.wc.bg,
        elevation: 0,
        title: Text(
          widget.title,
          style: wispSans(
            fontSize: 17,
            fontWeight: FontWeight.w700,
            color: context.wc.ink,
          ),
        ),
      ),
      body: SafeArea(
        child: Padding(
          padding: const EdgeInsets.fromLTRB(20, 8, 20, 16),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              if (widget.showClipboardHint) ...[
                _HintBanner(
                  icon: Icons.content_paste_rounded,
                  text:
                      'Pasted from your clipboard. '
                      'Please confirm before sending.',
                ),
                const SizedBox(height: 12),
              ],
              Expanded(
                child: Container(
                  padding: const EdgeInsets.all(16),
                  decoration: BoxDecoration(
                    color: context.wc.surface,
                    borderRadius: BorderRadius.circular(16),
                    border: Border.all(color: context.wc.border),
                  ),
                  child: TextField(
                    controller: _controller,
                    autofocus: !widget.showClipboardHint,
                    expands: true,
                    maxLines: null,
                    minLines: null,
                    textAlignVertical: TextAlignVertical.top,
                    keyboardType: TextInputType.multiline,
                    style: wispMono(fontSize: 14, color: context.wc.ink),
                    decoration: InputDecoration(
                      isCollapsed: true,
                      border: InputBorder.none,
                      hintText: 'Type or paste text to send…',
                      hintStyle: wispSans(
                        fontSize: 14,
                        color: context.wc.muted,
                      ),
                    ),
                  ),
                ),
              ),
              const SizedBox(height: 8),
              Text(
                willFallback
                    ? '$byteLen bytes · will be sent as a .txt file'
                    : '$byteLen bytes',
                textAlign: TextAlign.right,
                style: wispSans(
                  fontSize: 12,
                  color: willFallback ? context.wc.accentFg : context.wc.muted,
                ),
              ),
              const SizedBox(height: 12),
              FilledButton(
                onPressed: canContinue ? _submit : null,
                style: FilledButton.styleFrom(
                  minimumSize: const Size.fromHeight(48),
                  shape: RoundedRectangleBorder(
                    borderRadius: BorderRadius.circular(12),
                  ),
                  backgroundColor: kAccentCyanStrong,
                  foregroundColor: Colors.white,
                ),
                child: Text(
                  widget.continueLabel,
                  style: wispSans(fontWeight: FontWeight.w700, fontSize: 15),
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }
}

class _HintBanner extends StatelessWidget {
  const _HintBanner({required this.icon, required this.text});

  final IconData icon;
  final String text;

  @override
  Widget build(BuildContext context) {
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 14, vertical: 12),
      decoration: BoxDecoration(
        color: kAccentCyanHover,
        borderRadius: BorderRadius.circular(12),
      ),
      child: Row(
        children: [
          Icon(icon, size: 18, color: context.wc.accentFg),
          const SizedBox(width: 10),
          Expanded(
            child: Text(
              text,
              style: wispSans(
                fontSize: 13,
                fontWeight: FontWeight.w600,
                color: context.wc.ink.withValues(alpha: 0.8),
                height: 1.35,
              ),
            ),
          ),
        ],
      ),
    );
  }
}

/// Read the system clipboard's plain text. Returns null when empty.
Future<String?> readClipboardText() async {
  final data = await Clipboard.getData(Clipboard.kTextPlain);
  final text = data?.text;
  if (text == null || text.trim().isEmpty) {
    return null;
  }
  return text;
}
