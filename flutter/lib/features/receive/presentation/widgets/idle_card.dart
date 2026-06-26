import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../../../../theme/wisp_theme.dart';
import '../../../transfers/application/pubkey_visual.dart';
import '../../../usb_cable/presentation/usb_status_entry.dart';
import '../../application/state.dart';

class ReceiveIdleCard extends StatefulWidget {
  const ReceiveIdleCard({
    super.key,
    required this.state,
    this.onOpenSettings,
    this.onOpenQr,
    this.onRefreshCode,
  });

  final ReceiverIdleViewState state;
  final VoidCallback? onOpenSettings;
  final VoidCallback? onOpenQr;

  /// Tapped when the user explicitly asks for a fresh pairing code (refresh
  /// icon or the "may have been used" hint).  Wired to the receiver
  /// service's `ensureRegistered` from the shell.
  final VoidCallback? onRefreshCode;

  @override
  State<ReceiveIdleCard> createState() => _ReceiveIdleCardState();
}

class _ReceiveIdleCardState extends State<ReceiveIdleCard> {
  bool _codeHovering = false;
  bool _copied = false;
  Timer? _copiedResetTimer;

  /// Drives a low-frequency tick so the TTL countdown bar redraws ~once a
  /// second without rebuilding the whole shell.  Only ticks while the
  /// pairing code is visible and has a parseable expiry.
  Timer? _ttlTickTimer;
  bool _refreshing = false;

  void _syncTtlTimer() {
    final hasCountdown = widget.state.expiresAt != null;
    if (hasCountdown && _ttlTickTimer == null) {
      _ttlTickTimer = Timer.periodic(const Duration(seconds: 1), (_) {
        if (mounted) setState(() {});
      });
    } else if (!hasCountdown && _ttlTickTimer != null) {
      _ttlTickTimer?.cancel();
      _ttlTickTimer = null;
    }
  }

  @override
  void initState() {
    super.initState();
    _syncTtlTimer();
  }

  @override
  void didUpdateWidget(ReceiveIdleCard oldWidget) {
    super.didUpdateWidget(oldWidget);
    _syncTtlTimer();
  }

  String _formatCode(String raw) {
    if (raw.length != 6) return raw;
    return '${raw.substring(0, 3)} ${raw.substring(3)}';
  }

  Future<void> _copyCode(String code) async {
    await Clipboard.setData(ClipboardData(text: code));
    _copiedResetTimer?.cancel();
    if (mounted) {
      setState(() => _copied = true);
    }
    _copiedResetTimer = Timer(const Duration(milliseconds: 1100), () {
      if (mounted) {
        setState(() => _copied = false);
      }
    });
  }

  @override
  void dispose() {
    _copiedResetTimer?.cancel();
    _ttlTickTimer?.cancel();
    super.dispose();
  }

  Future<void> _handleRefresh() async {
    final action = widget.onRefreshCode;
    if (action == null || _refreshing) return;
    setState(() => _refreshing = true);
    try {
      action();
      // Give the service a beat to round-trip the new code through the
      // watch channel so the spinner doesn't blink off before the new code
      // becomes visible.  Even if the call fails we exit the spinner state
      // after a short window — the stale banner stays so user can retry.
      await Future<void>.delayed(const Duration(milliseconds: 600));
    } finally {
      if (mounted) setState(() => _refreshing = false);
    }
  }

  /// Fraction of the original 5-minute TTL window remaining as `[0.0, 1.0]`.
  /// Returns `null` when there's no countdown to render (no `expiresAt`).
  /// Saturates at 0 when expired (we leave the bar at empty until the next
  /// `RegistrationUpdated` event swaps in the rotated code).
  double? _ttlRemaining() {
    final expiresAt = widget.state.expiresAt;
    if (expiresAt == null) return null;
    final now = DateTime.now().toUtc();
    if (now.isAfter(expiresAt)) return 0.0;
    final remaining = expiresAt.difference(now).inMilliseconds;
    // 300s TTL is set server-side in crates/server/src/lib.rs:29
    // (DISCOVERY_TTL_SECONDS).  If the server ever ramps that up, the bar
    // simply renders fuller for longer — no breakage.
    const totalMs = 300 * 1000;
    return (remaining / totalMs).clamp(0.0, 1.0);
  }

  @override
  Widget build(BuildContext context) {
    final badgeColor = widget.state.badge.color;
    final ttl = _ttlRemaining();
    final showStaleHint =
        widget.state.isStale &&
        widget.state.lifecycle == ReceiverLifecycle.ready;

    return Padding(
      padding: const EdgeInsets.symmetric(horizontal: 4, vertical: 2),
      child: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Row(
            crossAxisAlignment: CrossAxisAlignment.end,
            children: [
              Expanded(
                child: Column(
                  mainAxisSize: MainAxisSize.min,
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Row(
                      children: [
                        Flexible(
                          child: Text(
                            widget.state.deviceName,
                            maxLines: 1,
                            overflow: TextOverflow.ellipsis,
                            style: wispSans(
                              fontSize: 13.5,
                              fontWeight: FontWeight.w600,
                              color: kInk,
                              letterSpacing: -0.25,
                            ),
                          ),
                        ),
                      ],
                    ),
                    if (widget.state.endpointId.isNotEmpty) ...[
                      const SizedBox(height: 4),
                      CopyablePubkeyBadge(
                        endpointId: widget.state.endpointId,
                        size: PubkeyBadgeSize.small,
                        iconSize: 15,
                      ),
                    ],
                    const SizedBox(height: 2),
                    Row(
                      children: [
                        Container(
                          width: 7,
                          height: 7,
                          decoration: BoxDecoration(
                            color: badgeColor,
                            shape: BoxShape.circle,
                            boxShadow: [
                              BoxShadow(
                                color: badgeColor.withValues(alpha: 0.22),
                                blurRadius: 6,
                              ),
                            ],
                          ),
                        ),
                        const SizedBox(width: 7),
                        Flexible(
                          child: Text(
                            widget.state.badge.label,
                            maxLines: 1,
                            overflow: TextOverflow.ellipsis,
                            style: wispSans(
                              fontSize: 11.5,
                              fontWeight: FontWeight.w500,
                              color: badgeColor,
                            ),
                          ),
                        ),
                      ],
                    ),
                  ],
                ),
              ),
              const SizedBox(width: 12),
              Row(
                mainAxisSize: MainAxisSize.min,
                crossAxisAlignment: CrossAxisAlignment.end,
                children: [
                  Column(
                    mainAxisSize: MainAxisSize.min,
                    crossAxisAlignment: CrossAxisAlignment.end,
                    children: [
                      AnimatedSwitcher(
                        duration: const Duration(milliseconds: 160),
                        switchInCurve: Curves.easeOutCubic,
                        switchOutCurve: Curves.easeInCubic,
                        transitionBuilder: (child, animation) =>
                            FadeTransition(opacity: animation, child: child),
                        child: Text(
                          _copied ? 'Copied' : 'Receive code',
                          key: ValueKey<String>(
                            _copied ? 'copied-label' : 'receive-label',
                          ),
                          style: wispSans(
                            fontSize: 9.5,
                            fontWeight: FontWeight.w500,
                            color: _copied
                                ? const Color(0xFF5E9B70)
                                : kMuted.withValues(alpha: 0.62),
                            letterSpacing: 0.18,
                          ),
                        ),
                      ),
                      const SizedBox(height: 6),
                      Tooltip(
                        message: 'Copy receive code',
                        child: Semantics(
                          button: true,
                          label: 'Copy receive code',
                          hint: 'Copies the receive code to clipboard',
                          child: Material(
                            color: Colors.transparent,
                            child: InkWell(
                              key: const ValueKey<String>('idle-receive-code'),
                              onTap: () =>
                                  _copyCode(widget.state.clipboardCode),
                              canRequestFocus: true,
                              onHover: (value) {
                                if (_codeHovering == value) {
                                  return;
                                }
                                setState(() => _codeHovering = value);
                              },
                              onFocusChange: (value) {
                                if (_codeHovering == value) {
                                  return;
                                }
                                setState(() => _codeHovering = value);
                              },
                              borderRadius: BorderRadius.circular(12),
                              child: AnimatedContainer(
                                duration: const Duration(milliseconds: 160),
                                curve: Curves.easeOutCubic,
                                height: 38,
                                padding: const EdgeInsets.symmetric(
                                  horizontal: 14,
                                ),
                                decoration: BoxDecoration(
                                  color: _codeHovering
                                      ? Colors.white
                                      : const Color(0xFFFDFDFD),
                                  borderRadius: BorderRadius.circular(12),
                                  border: Border.all(
                                    color: _codeHovering
                                        ? const Color(0xFFCFCFCF)
                                        : const Color(0xFFD7D7D7),
                                  ),
                                  boxShadow: [
                                    BoxShadow(
                                      color: Colors.black.withValues(
                                        alpha: _codeHovering ? 0.028 : 0.018,
                                      ),
                                      blurRadius: _codeHovering ? 10 : 6,
                                      offset: const Offset(0, 2),
                                    ),
                                  ],
                                ),
                                child: Center(
                                  child: Text(
                                    _formatCode(widget.state.code),
                                    style: wispMono(
                                      fontSize: 15,
                                      fontWeight: FontWeight.w700,
                                      color: const Color(0xFF111111),
                                      letterSpacing: 2.2,
                                    ),
                                  ),
                                ),
                              ),
                            ),
                          ),
                        ),
                      ),
                    ],
                  ),
                  const SizedBox(width: 8),
                  // Refresh icon — only meaningful when there's an active code
                  // to rotate.  Disabled (faded) while the service is starting
                  // / unavailable so the affordance matches reality.
                  IconButton(
                    key: const ValueKey<String>('idle-refresh-code-button'),
                    onPressed:
                        (widget.onRefreshCode != null &&
                            widget.state.lifecycle == ReceiverLifecycle.ready &&
                            !_refreshing)
                        ? _handleRefresh
                        : null,
                    tooltip: widget.state.isStale
                        ? 'Code may have been used — tap to refresh'
                        : 'Refresh code',
                    style: IconButton.styleFrom(
                      fixedSize: const Size(38, 38),
                      minimumSize: const Size(38, 38),
                      padding: EdgeInsets.zero,
                      tapTargetSize: MaterialTapTargetSize.shrinkWrap,
                      backgroundColor: widget.state.isStale
                          ? const Color(0xFFFFF6E5)
                          : const Color(0xFFFCFCFC),
                      shape: RoundedRectangleBorder(
                        borderRadius: BorderRadius.circular(12),
                        side: BorderSide(
                          color: widget.state.isStale
                              ? const Color(0xFFE0B96A)
                              : const Color(0xFFD7D7D7),
                        ),
                      ),
                    ),
                    icon: _refreshing
                        ? SizedBox(
                            width: 16,
                            height: 16,
                            child: CircularProgressIndicator(
                              strokeWidth: 2,
                              color: kMuted.withValues(alpha: 0.9),
                            ),
                          )
                        : Icon(
                            Icons.refresh_rounded,
                            size: 18,
                            color: widget.state.isStale
                                ? const Color(0xFFC0912C)
                                : kMuted.withValues(alpha: 0.9),
                          ),
                  ),
                  const SizedBox(width: 8),
                  IconButton(
                    key: const ValueKey<String>('idle-qr-button'),
                    onPressed: widget.onOpenQr ?? () {},
                    tooltip: 'Pair via QR',
                    style: IconButton.styleFrom(
                      fixedSize: const Size(38, 38),
                      minimumSize: const Size(38, 38),
                      padding: EdgeInsets.zero,
                      tapTargetSize: MaterialTapTargetSize.shrinkWrap,
                      backgroundColor: const Color(0xFFFCFCFC),
                      shape: RoundedRectangleBorder(
                        borderRadius: BorderRadius.circular(12),
                        side: const BorderSide(color: Color(0xFFD7D7D7)),
                      ),
                    ),
                    icon: Icon(
                      Icons.qr_code_rounded,
                      size: 18,
                      color: kMuted.withValues(alpha: 0.9),
                    ),
                  ),
                  const SizedBox(width: 8),
                  IconButton(
                    key: const ValueKey<String>('idle-settings-button'),
                    onPressed: widget.onOpenSettings ?? () {},
                    tooltip: 'Settings',
                    style: IconButton.styleFrom(
                      fixedSize: const Size(38, 38),
                      minimumSize: const Size(38, 38),
                      padding: EdgeInsets.zero,
                      tapTargetSize: MaterialTapTargetSize.shrinkWrap,
                      backgroundColor: const Color(0xFFFCFCFC),
                      shape: RoundedRectangleBorder(
                        borderRadius: BorderRadius.circular(12),
                        side: const BorderSide(color: Color(0xFFD7D7D7)),
                      ),
                    ),
                    icon: Icon(
                      Icons.tune_rounded,
                      size: 18,
                      color: kMuted.withValues(alpha: 0.9),
                    ),
                  ),
                ],
              ),
            ],
          ),
          // Stale hint banner — only shown when the rendezvous server has
          // told us the code is no longer claimable.  Tapping refreshes;
          // we leave the existing code visible underneath so the user has
          // context, but the banner makes the failure mode obvious.
          if (showStaleHint) ...[
            const SizedBox(height: 10),
            InkWell(
              key: const ValueKey<String>('idle-stale-hint'),
              onTap: widget.onRefreshCode == null ? null : _handleRefresh,
              borderRadius: BorderRadius.circular(8),
              child: Container(
                padding: const EdgeInsets.symmetric(
                  horizontal: 10,
                  vertical: 8,
                ),
                decoration: BoxDecoration(
                  color: const Color(0xFFFFF6E5),
                  borderRadius: BorderRadius.circular(8),
                  border: Border.all(color: const Color(0xFFE0B96A)),
                ),
                child: Row(
                  children: [
                    Icon(
                      Icons.info_outline_rounded,
                      size: 14,
                      color: const Color(0xFFC0912C),
                    ),
                    const SizedBox(width: 8),
                    Expanded(
                      child: Text(
                        'Code may have been used. Tap to refresh.',
                        style: wispSans(
                          fontSize: 11.5,
                          fontWeight: FontWeight.w500,
                          color: const Color(0xFF6B4D14),
                        ),
                      ),
                    ),
                  ],
                ),
              ),
            ),
          ],
          // TTL countdown bar — drains over the 5-minute server TTL so the
          // user knows roughly when the displayed code will auto-rotate.
          // Hidden when there's no parseable expiry or the receiver isn't
          // in the Ready lifecycle phase.
          if (ttl != null &&
              widget.state.lifecycle == ReceiverLifecycle.ready) ...[
            const SizedBox(height: 8),
            ClipRRect(
              borderRadius: BorderRadius.circular(2),
              child: LinearProgressIndicator(
                value: ttl,
                minHeight: 3,
                backgroundColor: const Color(0xFFEDEDED),
                color: widget.state.isStale
                    ? const Color(0xFFC0912C)
                    : widget.state.badge.color.withValues(alpha: 0.55),
              ),
            ),
          ],
          const SizedBox(height: 10),
          const UsbStatusEntry(),
        ],
      ),
    );
  }
}
