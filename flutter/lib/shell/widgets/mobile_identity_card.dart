import 'dart:async';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import '../../../features/receive/application/state.dart';
import '../../../theme/wisp_theme.dart';

class MobileIdentityCard extends StatefulWidget {
  const MobileIdentityCard({
    super.key,
    required this.state,
    this.onRefreshCode,
  });

  final ReceiverIdleViewState state;

  /// Tapped when the user explicitly asks for a fresh pairing code (refresh
  /// icon or the "may have been used" hint).
  final VoidCallback? onRefreshCode;

  @override
  State<MobileIdentityCard> createState() => _MobileIdentityCardState();
}

class _MobileIdentityCardState extends State<MobileIdentityCard> {
  bool _copied = false;
  Timer? _copiedResetTimer;
  Timer? _ttlTickTimer;
  bool _refreshing = false;

  @override
  void initState() {
    super.initState();
    _syncTtlTimer();
  }

  @override
  void didUpdateWidget(MobileIdentityCard oldWidget) {
    super.didUpdateWidget(oldWidget);
    _syncTtlTimer();
  }

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

  void _copy() {
    Clipboard.setData(ClipboardData(text: widget.state.clipboardCode));
    _copiedResetTimer?.cancel();
    HapticFeedback.mediumImpact();
    setState(() => _copied = true);
    _copiedResetTimer = Timer(const Duration(seconds: 2), () {
      if (mounted) setState(() => _copied = false);
    });
  }

  Future<void> _handleRefresh() async {
    final action = widget.onRefreshCode;
    if (action == null || _refreshing) return;
    HapticFeedback.selectionClick();
    setState(() => _refreshing = true);
    try {
      action();
      await Future<void>.delayed(const Duration(milliseconds: 600));
    } finally {
      if (mounted) setState(() => _refreshing = false);
    }
  }

  /// Fraction of the original 5-minute TTL remaining as `[0.0, 1.0]`.  See
  /// idle_card.dart for the matching server-side constant.
  double? _ttlRemaining() {
    final expiresAt = widget.state.expiresAt;
    if (expiresAt == null) return null;
    final now = DateTime.now().toUtc();
    if (now.isAfter(expiresAt)) return 0.0;
    final remaining = expiresAt.difference(now).inMilliseconds;
    const totalMs = 300 * 1000;
    return (remaining / totalMs).clamp(0.0, 1.0);
  }

  @override
  void dispose() {
    _copiedResetTimer?.cancel();
    _ttlTickTimer?.cancel();
    super.dispose();
  }

  String _formatCode(String code) {
    if (code.length != 6) return code;
    return '${code.substring(0, 3)} ${code.substring(3)}';
  }

  @override
  Widget build(BuildContext context) {
    final ttl = _ttlRemaining();
    final showStaleHint =
        widget.state.isStale && widget.state.lifecycle == ReceiverLifecycle.ready;
    final canRefresh = widget.onRefreshCode != null &&
        widget.state.lifecycle == ReceiverLifecycle.ready &&
        !_refreshing;

    return Card(
      elevation: 0,
      color: kSurface,
      shape: RoundedRectangleBorder(
        borderRadius: BorderRadius.circular(28),
        side: const BorderSide(color: kBorder, width: 1),
      ),
      child: Padding(
        padding: const EdgeInsets.all(24.0),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(
                  widget.state.deviceName,
                  style: wispSans(
                    fontSize: 20,
                    fontWeight: FontWeight.w700,
                    color: kInk,
                  ),
                ),
                const SizedBox(height: 4),
                Row(
                  children: [
                    Container(
                      width: 8,
                      height: 8,
                      decoration: BoxDecoration(
                        color: widget.state.badge.color,
                        shape: BoxShape.circle,
                      ),
                    ),
                    const SizedBox(width: 8),
                    Text(
                      widget.state.badge.label,
                      style: wispSans(
                        fontSize: 14,
                        color: widget.state.badge.color,
                        fontWeight: FontWeight.w500,
                      ),
                    ),
                  ],
                ),
              ],
            ),
            const SizedBox(height: 32),
            Text(
              'RECEIVE CODE',
              style: wispSans(
                fontSize: 12,
                fontWeight: FontWeight.w800,
                color: kMuted,
                letterSpacing: 1.2,
              ),
            ),
            const SizedBox(height: 8),
            Row(
              crossAxisAlignment: CrossAxisAlignment.center,
              children: [
                Expanded(
                  child: GestureDetector(
                    onTap: _copy,
                    behavior: HitTestBehavior.opaque,
                    child: Row(
                      children: [
                        Text(
                          _formatCode(widget.state.code),
                          style: wispMono(
                            fontSize: 36,
                            fontWeight: FontWeight.w700,
                            color: kInk,
                            letterSpacing: 4,
                          ),
                        ),
                        const SizedBox(width: 12),
                        AnimatedSwitcher(
                          duration: const Duration(milliseconds: 200),
                          child: _copied
                              ? const Icon(
                                  Icons.check_circle_outline_rounded,
                                  color: Color(0xFF49B36C),
                                  key: ValueKey('done'),
                                )
                              : Icon(
                                  Icons.copy_rounded,
                                  color: kMuted.withValues(alpha: 0.5),
                                  key: const ValueKey('copy'),
                                ),
                        ),
                      ],
                    ),
                  ),
                ),
                // Refresh button — amber-tinted when the rendezvous flagged
                // the displayed code as stale, otherwise neutral.
                Material(
                  color: Colors.transparent,
                  child: InkWell(
                    key: const ValueKey<String>('mobile-refresh-code-button'),
                    onTap: canRefresh ? _handleRefresh : null,
                    borderRadius: BorderRadius.circular(14),
                    child: Container(
                      width: 44,
                      height: 44,
                      decoration: BoxDecoration(
                        color: widget.state.isStale
                            ? const Color(0xFFFFF6E5)
                            : const Color(0xFFFCFCFC),
                        borderRadius: BorderRadius.circular(14),
                        border: Border.all(
                          color: widget.state.isStale
                              ? const Color(0xFFE0B96A)
                              : const Color(0xFFD7D7D7),
                        ),
                      ),
                      child: Center(
                        child: _refreshing
                            ? SizedBox(
                                width: 18,
                                height: 18,
                                child: CircularProgressIndicator(
                                  strokeWidth: 2,
                                  color: kMuted.withValues(alpha: 0.9),
                                ),
                              )
                            : Icon(
                                Icons.refresh_rounded,
                                size: 22,
                                color: widget.state.isStale
                                    ? const Color(0xFFC0912C)
                                    : kMuted.withValues(
                                        alpha: canRefresh ? 0.9 : 0.35,
                                      ),
                              ),
                      ),
                    ),
                  ),
                ),
              ],
            ),
            if (ttl != null &&
                widget.state.lifecycle == ReceiverLifecycle.ready) ...[
              const SizedBox(height: 14),
              ClipRRect(
                borderRadius: BorderRadius.circular(3),
                child: LinearProgressIndicator(
                  value: ttl,
                  minHeight: 4,
                  backgroundColor: const Color(0xFFEDEDED),
                  color: widget.state.isStale
                      ? const Color(0xFFC0912C)
                      : widget.state.badge.color.withValues(alpha: 0.55),
                ),
              ),
            ],
            if (showStaleHint) ...[
              const SizedBox(height: 12),
              InkWell(
                key: const ValueKey<String>('mobile-stale-hint'),
                onTap: widget.onRefreshCode == null ? null : _handleRefresh,
                borderRadius: BorderRadius.circular(10),
                child: Container(
                  padding: const EdgeInsets.symmetric(
                    horizontal: 12,
                    vertical: 10,
                  ),
                  decoration: BoxDecoration(
                    color: const Color(0xFFFFF6E5),
                    borderRadius: BorderRadius.circular(10),
                    border: Border.all(color: const Color(0xFFE0B96A)),
                  ),
                  child: Row(
                    children: [
                      const Icon(
                        Icons.info_outline_rounded,
                        size: 16,
                        color: Color(0xFFC0912C),
                      ),
                      const SizedBox(width: 10),
                      Expanded(
                        child: Text(
                          'Code may have been used. Tap to refresh.',
                          style: wispSans(
                            fontSize: 13,
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
          ],
        ),
      ),
    );
  }
}
