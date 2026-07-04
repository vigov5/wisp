import 'dart:typed_data';

import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../../theme/wisp_theme.dart';
import '../../send/presentation/qr_scan_page.dart';
import '../application/identity_backup_codec.dart';
import '../application/identity_backup_file.dart';
import '../identity_providers.dart';

/// Settings → "Restore identity".
///
/// Accepts a backup payload by paste, QR scan, or `.wispkey` file, decrypts it
/// if needed, and overwrites this device's stored secret key. Because the
/// native engine reads the key once at startup, a successful restore prompts
/// the user to relaunch Wisp.
class IdentityImportPage extends ConsumerStatefulWidget {
  const IdentityImportPage({super.key});

  @override
  ConsumerState<IdentityImportPage> createState() => _IdentityImportPageState();
}

class _IdentityImportPageState extends ConsumerState<IdentityImportPage> {
  final _codeController = TextEditingController();
  final _passwordController = TextEditingController();

  bool _busy = false;
  bool _restored = false;

  @override
  void dispose() {
    _codeController.dispose();
    _passwordController.dispose();
    super.dispose();
  }

  bool get _needsPassword =>
      _codeController.text.trim().isNotEmpty &&
      IdentityBackupCodec.isEncrypted(_codeController.text);

  Future<void> _scanQr() async {
    final value = await Navigator.of(context).push<String>(
      MaterialPageRoute<String>(builder: (_) => const QrScanPage()),
    );
    if (!mounted || value == null || value.isEmpty) return;
    if (!IdentityBackupCodec.looksLikeBackup(value)) {
      _snack('That QR code isn\'t a Wisp identity backup.');
      return;
    }
    setState(() => _codeController.text = value.trim());
  }

  Future<void> _pickFile() async {
    setState(() => _busy = true);
    try {
      final contents = await const IdentityBackupFile().open();
      if (contents == null) return; // cancelled
      final trimmed = contents.trim();
      if (!IdentityBackupCodec.looksLikeBackup(trimmed)) {
        _snack('That file isn\'t a Wisp identity backup.');
        return;
      }
      setState(() => _codeController.text = trimmed);
    } catch (e) {
      _snack('Couldn\'t read file: $e');
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  Future<void> _restore() async {
    final code = _codeController.text.trim();
    if (code.isEmpty) {
      _snack('Paste a backup code, scan a QR, or pick a file first.');
      return;
    }
    if (!IdentityBackupCodec.looksLikeBackup(code)) {
      _snack('That doesn\'t look like a Wisp identity backup.');
      return;
    }

    final confirmed = await _confirmReplace();
    if (confirmed != true || !mounted) return;

    setState(() => _busy = true);
    try {
      final codec = ref.read(identityBackupCodecProvider);
      final Uint8List bytes = await codec.decode(
        code,
        password: _needsPassword ? _passwordController.text : null,
      );
      await ref.read(identityStorageProvider).replace(bytes);
      if (!mounted) return;
      setState(() => _restored = true);
      await _showRestartDialog();
    } on IdentityBackupBadPasswordException {
      _snack('Wrong password — couldn\'t decrypt this backup.');
    } on IdentityBackupException catch (e) {
      _snack(e.message);
    } catch (e) {
      _snack('Couldn\'t restore identity: $e');
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  Future<bool?> _confirmReplace() {
    return showDialog<bool>(
      context: context,
      builder: (context) => AlertDialog(
        title: const Text('Replace this device\'s identity?'),
        content: const Text(
          'The current identity will be overwritten. If you haven\'t backed '
          'it up, peers who saved THIS device will need to pair again.\n\n'
          'Use a restored identity on only one device at a time. If the old '
          'device still runs Wisp with the same key, the two will conflict — '
          'remove Wisp from it (or restore a different identity there) first.',
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(context).pop(false),
            child: const Text('Cancel'),
          ),
          FilledButton(
            onPressed: () => Navigator.of(context).pop(true),
            style: FilledButton.styleFrom(backgroundColor: kAccentCyanStrong),
            child: const Text('Replace'),
          ),
        ],
      ),
    );
  }

  Future<void> _showRestartDialog() {
    return showDialog<void>(
      context: context,
      barrierDismissible: false,
      builder: (dialogContext) => AlertDialog(
        title: const Text('Identity restored'),
        content: const Text(
          'Restart Wisp for the restored identity to take effect. Until then '
          'this device still uses its previous key.',
        ),
        actions: [
          FilledButton(
            onPressed: () {
              Navigator.of(dialogContext).pop();
              // Leave the import screen; settings reflects the new key on
              // next launch (the engine reads it at bootstrap).
              Navigator.of(context).pop();
            },
            style: FilledButton.styleFrom(backgroundColor: kAccentCyanStrong),
            child: const Text('Got it'),
          ),
        ],
      ),
    );
  }

  void _snack(String message) {
    if (!mounted) return;
    ScaffoldMessenger.of(context).showSnackBar(
      SnackBar(content: Text(message), duration: const Duration(seconds: 3)),
    );
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      backgroundColor: context.wc.bg,
      appBar: AppBar(
        backgroundColor: context.wc.bg,
        elevation: 0,
        leading: IconButton(
          icon: const Icon(Icons.arrow_back_rounded),
          onPressed: () => Navigator.of(context).pop(),
        ),
        title: Text(
          'Restore identity',
          style: wispSans(
            fontSize: 18,
            fontWeight: FontWeight.w700,
            color: context.wc.ink,
          ),
        ),
      ),
      body: SafeArea(
        child: SingleChildScrollView(
          padding: const EdgeInsets.fromLTRB(24, 8, 24, 28),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              Text(
                'Paste a backup code, scan its QR, or pick a saved .wispkey '
                'file to restore a previous identity on this device.',
                style: wispSans(
                  fontSize: 13,
                  color: context.wc.muted,
                  height: 1.45,
                ),
              ),
              const SizedBox(height: 20),
              TextField(
                controller: _codeController,
                minLines: 2,
                maxLines: 4,
                style: wispMono(fontSize: 12, color: context.wc.ink),
                decoration: const InputDecoration(
                  labelText: 'Backup code',
                  hintText: 'wisp-key:v1:…',
                  alignLabelWithHint: true,
                ),
                onChanged: (_) => setState(() {}),
              ),
              const SizedBox(height: 12),
              Row(
                children: [
                  Expanded(
                    child: OutlinedButton.icon(
                      onPressed: _busy ? null : _scanQr,
                      icon: const Icon(Icons.qr_code_scanner_rounded, size: 18),
                      label: const Text('Scan QR'),
                      style: _secondaryButtonStyle(),
                    ),
                  ),
                  const SizedBox(width: 12),
                  Expanded(
                    child: OutlinedButton.icon(
                      onPressed: _busy ? null : _pickFile,
                      icon: const Icon(Icons.folder_open_rounded, size: 18),
                      label: const Text('Pick file'),
                      style: _secondaryButtonStyle(),
                    ),
                  ),
                ],
              ),
              if (_needsPassword) ...[
                const SizedBox(height: 16),
                TextField(
                  controller: _passwordController,
                  obscureText: true,
                  decoration: const InputDecoration(
                    labelText: 'Password',
                    helperText: 'This backup is password-protected.',
                  ),
                ),
              ],
              const SizedBox(height: 24),
              FilledButton(
                onPressed: (_busy || _restored) ? null : _restore,
                style: FilledButton.styleFrom(
                  backgroundColor: kAccentCyanStrong,
                  foregroundColor: Colors.white,
                  minimumSize: const Size(0, 48),
                  shape: RoundedRectangleBorder(
                    borderRadius: BorderRadius.circular(12),
                  ),
                ),
                child: Text(_busy ? 'Restoring…' : 'Restore identity'),
              ),
            ],
          ),
        ),
      ),
    );
  }

  // Soft-tint secondary style (see drift button-style conventions): accent
  // foreground, accent fill at 0.08, accent border at 0.15.
  ButtonStyle _secondaryButtonStyle() => OutlinedButton.styleFrom(
    foregroundColor: kAccentCyanStrong,
    backgroundColor: kAccentCyanStrong.withValues(alpha: 0.08),
    minimumSize: const Size(0, 46),
    side: BorderSide(color: kAccentCyanStrong.withValues(alpha: 0.15)),
    shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(12)),
  );
}
