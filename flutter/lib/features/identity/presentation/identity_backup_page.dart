import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:qr_flutter/qr_flutter.dart';

import '../../../platform/device_auth_gate.dart';
import '../../../theme/wisp_theme.dart';
import '../../settings/presentation/widgets/settings_toggle_field.dart';
import '../application/identity_backup_file.dart';
import '../identity_providers.dart';

/// Warning accent reused from the QR-pairing screen's "no LAN" hint.
const Color _kWarn = Color(0xFFC78F2A);

/// Whether device auth has been cleared for this visit to the screen.
enum _GateState { checking, granted, denied }

/// Settings → "Back up identity".
///
/// Reveals the device's secret key as a QR code + copyable text code (and an
/// optional `.wispkey` file) so the user can restore the *same* identity on a
/// reinstall or a new device — peers who already saved this device keep
/// connecting without re-pairing. Optionally password-protects the payload.
class IdentityBackupPage extends ConsumerStatefulWidget {
  const IdentityBackupPage({super.key});

  @override
  ConsumerState<IdentityBackupPage> createState() => _IdentityBackupPageState();
}

class _IdentityBackupPageState extends ConsumerState<IdentityBackupPage> {
  final _passwordController = TextEditingController();
  final _confirmController = TextEditingController();

  Uint8List? _keyBytes;
  String? _loadError;

  /// Device-auth gate state. The secret key is not read from storage until
  /// this reaches [_GateState.granted].
  _GateState _gate = _GateState.checking;

  /// True when we let the user through without a real challenge because the
  /// mobile device has no screen lock — surface a nudge to set one up.
  bool _noLockReminder = false;

  /// When true, the QR/code/file are AES-GCM encrypted with the password.
  bool _protect = false;

  /// The computed backup payload currently shown. Null while a password is
  /// required but not yet applied, or before the key has loaded.
  String? _payload;
  bool _busy = false;

  @override
  void initState() {
    super.initState();
    _runGate();
  }

  /// Challenges the user before anything reads the key. On success (or when no
  /// challenge can be enforced) it proceeds to load the key.
  Future<void> _runGate() async {
    final result = await DeviceAuthGate().authenticate(
      'Unlock to reveal this device\'s identity backup',
    );
    if (!mounted) return;
    if (result == DeviceAuthResult.failed) {
      setState(() => _gate = _GateState.denied);
      return;
    }
    setState(() {
      _gate = _GateState.granted;
      _noLockReminder =
          DeviceAuthGate.isGatedPlatform &&
          result == DeviceAuthResult.unsupported;
    });
    await _load();
  }

  @override
  void dispose() {
    _passwordController.dispose();
    _confirmController.dispose();
    super.dispose();
  }

  Future<void> _load() async {
    try {
      final bytes = await ref.read(identityStorageProvider).read();
      if (!mounted) return;
      if (bytes == null) {
        setState(
          () => _loadError = 'No identity is stored on this device yet.',
        );
        return;
      }
      setState(() => _keyBytes = bytes);
      // Plaintext payload can be shown immediately.
      await _recompute();
    } catch (e) {
      if (mounted) setState(() => _loadError = e.toString());
    }
  }

  String? _passwordValidationError() {
    final pw = _passwordController.text;
    final confirm = _confirmController.text;
    if (pw.length < 6) return 'Use at least 6 characters.';
    if (pw != confirm) return 'Passwords don\'t match.';
    return null;
  }

  /// Recomputes [_payload] for the current key + protection settings.
  /// For the encrypted case the password must already be valid.
  Future<void> _recompute() async {
    final key = _keyBytes;
    if (key == null) return;
    setState(() => _busy = true);
    try {
      final codec = ref.read(identityBackupCodecProvider);
      final payload = _protect
          ? await codec.encode(key, password: _passwordController.text)
          : await codec.encode(key);
      if (mounted) setState(() => _payload = payload);
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  void _onToggleProtect(bool value) {
    setState(() {
      _protect = value;
      // Any previously shown payload is now stale (plaintext <-> encrypted).
      _payload = null;
    });
    if (!value) {
      // Back to plaintext: show it right away.
      unawaited(_recompute());
    }
  }

  Future<void> _applyPassword() async {
    final error = _passwordValidationError();
    if (error != null) {
      _snack(error);
      return;
    }
    FocusScope.of(context).unfocus();
    await _recompute();
  }

  Future<void> _copy() async {
    final payload = _payload;
    if (payload == null) return;
    await Clipboard.setData(ClipboardData(text: payload));
    _snack('Backup code copied');
  }

  Future<void> _saveFile() async {
    final payload = _payload;
    if (payload == null) return;
    setState(() => _busy = true);
    try {
      final dest = await const IdentityBackupFile().save(payload);
      if (dest == null) return; // cancelled
      _snack('Saved to $dest');
    } catch (e) {
      _snack('Couldn\'t save file: $e');
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  void _snack(String message) {
    if (!mounted) return;
    ScaffoldMessenger.of(context).showSnackBar(
      SnackBar(content: Text(message), duration: const Duration(seconds: 2)),
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
          'Back up identity',
          style: wispSans(
            fontSize: 18,
            fontWeight: FontWeight.w700,
            color: context.wc.ink,
          ),
        ),
      ),
      body: SafeArea(child: _buildBody(context)),
    );
  }

  Widget _buildBody(BuildContext context) {
    switch (_gate) {
      case _GateState.checking:
        return const Center(child: CircularProgressIndicator());
      case _GateState.denied:
        return _buildLocked(context);
      case _GateState.granted:
        if (_loadError != null) return _buildError(context, _loadError!);
        if (_keyBytes == null) {
          return const Center(child: CircularProgressIndicator());
        }
        return _buildContent(context);
    }
  }

  Widget _buildLocked(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.all(24),
      child: Column(
        mainAxisAlignment: MainAxisAlignment.center,
        children: [
          Icon(Icons.lock_outline_rounded, size: 48, color: context.wc.muted),
          const SizedBox(height: 12),
          Text(
            'Authentication required',
            style: wispSans(
              fontSize: 16,
              fontWeight: FontWeight.w700,
              color: context.wc.ink,
            ),
          ),
          const SizedBox(height: 8),
          Text(
            'Unlock with your fingerprint, face, or device PIN to reveal your '
            'identity backup.',
            textAlign: TextAlign.center,
            style: wispSans(
              fontSize: 13,
              color: context.wc.muted,
              height: 1.45,
            ),
          ),
          const SizedBox(height: 20),
          FilledButton(
            onPressed: () {
              setState(() => _gate = _GateState.checking);
              _runGate();
            },
            style: FilledButton.styleFrom(backgroundColor: kAccentCyanStrong),
            child: const Text('Unlock'),
          ),
        ],
      ),
    );
  }

  Widget _buildContent(BuildContext context) {
    final passwordError = _protect ? _passwordValidationError() : null;
    return SingleChildScrollView(
      padding: const EdgeInsets.fromLTRB(24, 8, 24, 28),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Text(
            'Save this to keep the same identity after reinstalling or moving '
            'to a new device. Restore it there and the people who saved you '
            'can still reach you — no need to re-pair or share a new code.',
            style: wispSans(
              fontSize: 13,
              color: context.wc.muted,
              height: 1.45,
            ),
          ),
          const SizedBox(height: 16),
          _buildWarningCard(),
          if (_noLockReminder) ...[
            const SizedBox(height: 10),
            Text(
              'Tip: this device has no screen lock, so we couldn\'t verify it '
              'was you. Set up a lock screen for stronger protection.',
              style: wispSans(
                fontSize: 11.5,
                color: context.wc.muted,
                height: 1.45,
              ),
            ),
          ],
          const SizedBox(height: 20),
          _buildProtectToggle(),
          if (_protect) ...[
            const SizedBox(height: 14),
            _buildPasswordFields(passwordError),
          ],
          const SizedBox(height: 22),
          if (_payload != null)
            _buildPayloadSection(context)
          else if (_protect)
            Text(
              'Set a password above, then tap "Show backup".',
              style: wispSans(
                fontSize: 12.5,
                color: context.wc.muted,
                height: 1.45,
              ),
            ),
        ],
      ),
    );
  }

  Widget _buildWarningCard() {
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 14, vertical: 12),
      decoration: BoxDecoration(
        color: _kWarn.withValues(alpha: 0.10),
        borderRadius: BorderRadius.circular(12),
        border: Border.all(color: _kWarn.withValues(alpha: 0.35)),
      ),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          const Icon(Icons.warning_amber_rounded, size: 18, color: _kWarn),
          const SizedBox(width: 10),
          Expanded(
            child: Text(
              'This code is your private key. Anyone who gets it can '
              'impersonate this device. Keep it secret — protect it with a '
              'password if you\'ll store it anywhere shared.',
              style: wispSans(
                fontSize: 12,
                color: const Color(0xFF8A6A1F),
                height: 1.45,
              ),
            ),
          ),
        ],
      ),
    );
  }

  Widget _buildProtectToggle() {
    return SettingsToggleField(
      title: 'Protect with a password',
      subtitle:
          'Encrypts the code and file. You\'ll enter this password when '
          'restoring.',
      value: _protect,
      onChanged: (value) {
        if (_busy) return;
        _onToggleProtect(value);
      },
    );
  }

  Widget _buildPasswordFields(String? error) {
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        TextField(
          controller: _passwordController,
          obscureText: true,
          decoration: const InputDecoration(labelText: 'Password'),
          onChanged: (_) => setState(() => _payload = null),
        ),
        const SizedBox(height: 10),
        TextField(
          controller: _confirmController,
          obscureText: true,
          decoration: const InputDecoration(labelText: 'Confirm password'),
          onChanged: (_) => setState(() => _payload = null),
        ),
        if (error != null) ...[
          const SizedBox(height: 6),
          Text(
            error,
            style: wispSans(fontSize: 11.5, color: const Color(0xFFC0392B)),
          ),
        ],
        const SizedBox(height: 12),
        FilledButton(
          onPressed: _busy ? null : _applyPassword,
          style: FilledButton.styleFrom(
            backgroundColor: kAccentCyanStrong,
            foregroundColor: Colors.white,
            minimumSize: const Size(0, 46),
            shape: RoundedRectangleBorder(
              borderRadius: BorderRadius.circular(12),
            ),
          ),
          child: const Text('Show backup'),
        ),
      ],
    );
  }

  Widget _buildPayloadSection(BuildContext context) {
    final payload = _payload!;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Center(
          child: Container(
            padding: const EdgeInsets.all(18),
            decoration: BoxDecoration(
              color: Colors.white,
              borderRadius: BorderRadius.circular(20),
              border: Border.all(color: context.wc.border),
            ),
            child: QrImageView(
              data: payload,
              size: 240,
              backgroundColor: Colors.white,
              eyeStyle: const QrEyeStyle(
                eyeShape: QrEyeShape.square,
                color: Color(0xFF111111),
              ),
              dataModuleStyle: const QrDataModuleStyle(
                dataModuleShape: QrDataModuleShape.square,
                color: Color(0xFF111111),
              ),
            ),
          ),
        ),
        const SizedBox(height: 14),
        Text(
          'Scan this on the new device, or copy the code below.',
          textAlign: TextAlign.center,
          style: wispSans(fontSize: 12.5, color: context.wc.muted, height: 1.4),
        ),
        const SizedBox(height: 16),
        Container(
          padding: const EdgeInsets.fromLTRB(14, 10, 6, 10),
          decoration: BoxDecoration(
            color: context.wc.surface,
            borderRadius: BorderRadius.circular(12),
            border: Border.all(color: context.wc.border),
          ),
          child: Row(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Expanded(
                child: SelectableText(
                  payload,
                  style: wispMono(fontSize: 12, color: context.wc.ink),
                ),
              ),
              IconButton(
                tooltip: 'Copy code',
                icon: const Icon(Icons.copy_rounded, size: 18),
                color: context.wc.muted,
                visualDensity: VisualDensity.compact,
                onPressed: _copy,
              ),
            ],
          ),
        ),
        if (IdentityBackupFile.isSupportedForSave) ...[
          const SizedBox(height: 14),
          // Soft-tint secondary style (see drift button-style conventions):
          // accent foreground, accent fill at 0.08, accent border at 0.15.
          OutlinedButton.icon(
            onPressed: _busy ? null : _saveFile,
            icon: const Icon(Icons.save_alt_rounded, size: 18),
            label: const Text('Save to file'),
            style: OutlinedButton.styleFrom(
              foregroundColor: kAccentCyanStrong,
              backgroundColor: kAccentCyanStrong.withValues(alpha: 0.08),
              minimumSize: const Size(0, 46),
              side: BorderSide(
                color: kAccentCyanStrong.withValues(alpha: 0.15),
              ),
              shape: RoundedRectangleBorder(
                borderRadius: BorderRadius.circular(12),
              ),
            ),
          ),
        ],
      ],
    );
  }

  Widget _buildError(BuildContext context, String message) {
    return Padding(
      padding: const EdgeInsets.all(24),
      child: Column(
        mainAxisAlignment: MainAxisAlignment.center,
        children: [
          Icon(Icons.error_outline_rounded, size: 48, color: context.wc.muted),
          const SizedBox(height: 12),
          Text(
            message,
            textAlign: TextAlign.center,
            style: wispSans(fontSize: 13, color: context.wc.muted),
          ),
        ],
      ),
    );
  }
}
