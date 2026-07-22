import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:qr_flutter/qr_flutter.dart';

import '../../../shell/widgets/page_header.dart';
import '../../../src/rust/api/receiver.dart' as rust_receiver;
import '../../../theme/wisp_theme.dart';

/// Full-screen QR code page for offline-LAN pairing.  Renders the
/// receiver's current ticket as a QR code that the sender scans, plus
/// the list of LAN-routable IPs so the user can confirm the device is
/// on the expected network before sharing.
class QrPairingPage extends StatefulWidget {
  const QrPairingPage({super.key});

  @override
  State<QrPairingPage> createState() => _QrPairingPageState();
}

class _QrPairingPageState extends State<QrPairingPage> {
  rust_receiver.QrPairingInfoData? _info;
  String? _error;

  @override
  void initState() {
    super.initState();
    _load();
  }

  Future<void> _load() async {
    try {
      final info = await rust_receiver.currentQrPairingInfo();
      if (!mounted) return;
      setState(() {
        _info = info;
        _error = null;
      });
    } catch (e) {
      if (!mounted) return;
      setState(() => _error = e.toString());
    }
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      backgroundColor: context.wc.bg,
      body: SafeArea(
        // Match the Settings screen: a flat top padding + shared PageHeader
        // instead of a Material AppBar, so the title size and top spacing line
        // up and the back button clears the desktop window controls / macOS
        // traffic lights.
        child: Padding(
          padding: const EdgeInsets.fromLTRB(24, 24, 24, 0),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              PageHeader(
                title: 'Pair via QR',
                onBack: () => Navigator.of(context).pop(),
              ),
              const SizedBox(height: 8),
              Expanded(
                child: _info != null
                    ? _buildContent(context, _info!)
                    : _error != null
                    ? _buildError(context, _error!)
                    : const Center(child: CircularProgressIndicator()),
              ),
            ],
          ),
        ),
      ),
    );
  }

  Widget _buildContent(
    BuildContext context,
    rust_receiver.QrPairingInfoData info,
  ) {
    return SingleChildScrollView(
      // Sides + top come from the page's outer padding + header gap; only add
      // the scroll bottom inset here.
      padding: const EdgeInsets.only(bottom: 24),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Text(
            'Scan this code from the sender device.',
            textAlign: TextAlign.center,
            style: wispSans(
              fontSize: 13.5,
              color: context.wc.muted,
              height: 1.4,
            ),
          ),
          const SizedBox(height: 18),
          Center(
            child: Container(
              padding: const EdgeInsets.all(20),
              decoration: BoxDecoration(
                color: Colors.white,
                borderRadius: BorderRadius.circular(20),
                border: Border.all(color: context.wc.border),
              ),
              child: QrImageView(
                data: info.ticket,
                size: 280,
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
          const SizedBox(height: 22),
          Text(
            'This device on the network',
            style: wispSans(
              fontSize: 13.5,
              fontWeight: FontWeight.w600,
              color: context.wc.ink,
            ),
          ),
          const SizedBox(height: 8),
          if (info.lanIps.isEmpty)
            _buildIpRow(
              context,
              'No LAN address detected — make sure Wi-Fi is connected.',
              copyable: false,
              warn: true,
            )
          else
            ...info.lanIps.map(
              (ip) => Padding(
                padding: const EdgeInsets.only(bottom: 6),
                child: _buildIpRow(context, ip, copyable: true),
              ),
            ),
          const SizedBox(height: 14),
          Text(
            'No internet needed — both devices just need the same Wi-Fi.',
            textAlign: TextAlign.center,
            style: wispSans(
              fontSize: 11.5,
              color: context.wc.muted,
              height: 1.4,
            ),
          ),
        ],
      ),
    );
  }

  Widget _buildIpRow(
    BuildContext context,
    String text, {
    required bool copyable,
    bool warn = false,
  }) {
    final color = warn ? const Color(0xFFC78F2A) : context.wc.ink;
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 14, vertical: 10),
      decoration: BoxDecoration(
        color: context.wc.surface,
        borderRadius: BorderRadius.circular(12),
        border: Border.all(color: context.wc.border),
      ),
      child: Row(
        children: [
          Icon(
            warn ? Icons.warning_amber_rounded : Icons.lan_rounded,
            size: 16,
            color: warn ? const Color(0xFFC78F2A) : context.wc.muted,
          ),
          const SizedBox(width: 10),
          Expanded(
            child: SelectableText(
              text,
              style: wispMono(fontSize: 13, color: color),
            ),
          ),
          if (copyable)
            IconButton(
              tooltip: 'Copy',
              icon: const Icon(Icons.copy_rounded, size: 16),
              color: context.wc.muted,
              visualDensity: VisualDensity.compact,
              onPressed: () async {
                await Clipboard.setData(ClipboardData(text: text));
                if (!context.mounted) return;
                ScaffoldMessenger.of(context).showSnackBar(
                  SnackBar(
                    content: Text('Copied $text'),
                    duration: const Duration(seconds: 2),
                  ),
                );
              },
            ),
        ],
      ),
    );
  }

  Widget _buildError(BuildContext context, String message) {
    return Center(
      child: Column(
        mainAxisAlignment: MainAxisAlignment.center,
        children: [
          Icon(Icons.error_outline_rounded, size: 48, color: context.wc.muted),
          const SizedBox(height: 12),
          Text(
            'Couldn\'t build QR code',
            style: wispSans(
              fontSize: 16,
              fontWeight: FontWeight.w700,
              color: context.wc.ink,
            ),
          ),
          const SizedBox(height: 8),
          Text(
            message,
            textAlign: TextAlign.center,
            style: wispSans(fontSize: 13, color: context.wc.muted),
          ),
          const SizedBox(height: 20),
          FilledButton(
            onPressed: _load,
            style: FilledButton.styleFrom(backgroundColor: kAccentCyanStrong),
            child: const Text('Retry'),
          ),
        ],
      ),
    );
  }
}
