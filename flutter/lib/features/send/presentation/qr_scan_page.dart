import 'dart:async';
import 'dart:math' as math;

import 'package:flutter/material.dart';
import 'package:flutter_zxing/flutter_zxing.dart';
import 'package:permission_handler/permission_handler.dart';

import '../../../theme/wisp_theme.dart';

/// Camera-based QR scanner.  Returns the scanned ticket string when the
/// user picks one — pop the page with `Navigator.pop(context, ticket)`.
class QrScanPage extends StatefulWidget {
  const QrScanPage({super.key});

  @override
  State<QrScanPage> createState() => _QrScanPageState();
}

/// Fraction of the shorter screen edge that zxing actually decodes. The
/// reticle below is sized from the same number, so the box the user aims
/// with is exactly the region being scanned — a larger reticle would promise
/// an area the decoder never looks at.
const double _kCropPercent = 0.7;

enum _CameraAccess { pending, granted, denied }

class _QrScanPageState extends State<QrScanPage> {
  _CameraAccess _access = _CameraAccess.pending;
  bool _handled = false;

  @override
  void initState() {
    super.initState();
    unawaited(_requestCamera());
  }

  /// Settles the camera permission *before* mounting the reader. Letting the
  /// camera plugin trigger the prompt on its own leaves the preview a dead
  /// black rectangle after the user grants it — the controller has already
  /// failed to open by then and nothing retries, so the first scan of a fresh
  /// install looks broken until you back out and come in again.
  Future<void> _requestCamera() async {
    var granted = true;
    try {
      var status = await Permission.camera.status;
      if (!status.isGranted) {
        status = await Permission.camera.request();
      }
      granted = status.isGranted;
    } catch (_) {
      // Plugin unavailable (widget tests, unsupported platform) — fall through
      // and let the reader try, which is the pre-existing behaviour.
      granted = true;
    }
    if (!mounted) return;
    setState(
      () => _access = granted ? _CameraAccess.granted : _CameraAccess.denied,
    );
  }

  void _onScan(Code code) {
    if (_handled) return;
    final value = code.text?.trim();
    if (value == null || value.isEmpty) return;
    _handled = true;
    Navigator.of(context).pop(value);
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      backgroundColor: Colors.black,
      appBar: AppBar(
        backgroundColor: Colors.black,
        foregroundColor: Colors.white,
        elevation: 0,
        title: Text(
          'Scan QR',
          style: wispSans(
            fontSize: 18,
            fontWeight: FontWeight.w700,
            color: Colors.white,
          ),
        ),
      ),
      body: switch (_access) {
        _CameraAccess.pending => const Center(
          child: CircularProgressIndicator(color: Colors.white70),
        ),
        _CameraAccess.denied => _buildDenied(),
        _CameraAccess.granted => _buildScanner(),
      },
    );
  }

  Widget _buildDenied() {
    return Padding(
      padding: const EdgeInsets.symmetric(horizontal: 32),
      child: Column(
        mainAxisAlignment: MainAxisAlignment.center,
        children: [
          Icon(
            Icons.no_photography_outlined,
            size: 44,
            color: Colors.white.withValues(alpha: 0.7),
          ),
          const SizedBox(height: 16),
          Text(
            'Wisp needs camera access to scan a QR code.',
            textAlign: TextAlign.center,
            style: wispSans(fontSize: 14, color: Colors.white, height: 1.45),
          ),
          const SizedBox(height: 16),
          TextButton(
            onPressed: openAppSettings,
            child: Text(
              'Open settings',
              style: wispSans(
                fontSize: 13.5,
                fontWeight: FontWeight.w600,
                color: kAccentCyan,
              ),
            ),
          ),
        ],
      ),
    );
  }

  Widget _buildScanner() {
    return Stack(
      fit: StackFit.expand,
      children: [
        // zxing-cpp over the camera stream. The reticle below is ours, and
        // the gallery / multi-scan affordances the widget offers by default
        // are off: pairing only ever scans one live QR code.
        ReaderWidget(
          onScan: _onScan,
          codeFormat: Format.qrCode,
          lensDirection: CameraLensDirection.back,
          cropPercent: _kCropPercent,
          showScannerOverlay: false,
          showGallery: false,
          showToggleCamera: false,
          actionButtonsBackgroundColor: Colors.white24,
        ),
        // Reticle overlay, matching the decoded crop square.
        LayoutBuilder(
          builder: (context, constraints) {
            final side =
                math.min(constraints.maxWidth, constraints.maxHeight) *
                _kCropPercent;
            return Center(
              child: Container(
                width: side,
                height: side,
                decoration: BoxDecoration(
                  borderRadius: BorderRadius.circular(20),
                  border: Border.all(color: Colors.white70, width: 2),
                ),
              ),
            );
          },
        ),
        // Sits clear of the reader's own torch button, which it pins to the
        // bottom-left of a full-width row regardless of alignment.
        Positioned(
          left: 24,
          right: 24,
          bottom: 110,
          child: Text(
            'Point at the QR code shown on the receiver device.',
            textAlign: TextAlign.center,
            style: wispSans(
              fontSize: 13.5,
              color: Colors.white.withValues(alpha: 0.85),
              height: 1.45,
            ),
          ),
        ),
      ],
    );
  }
}
