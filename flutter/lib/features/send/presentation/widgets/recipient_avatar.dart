import 'package:flutter/material.dart';
import 'package:app/theme/wisp_theme.dart';
import 'package:app/features/transfers/application/connection_path.dart';
import 'package:app/features/transfers/presentation/widgets/connection_path_badge.dart';
import 'package:app/features/transfers/presentation/widgets/sending_connection_strip.dart';

/// Soft red used for the connect / accept-decision countdown ring.  Light
/// enough to read as "time-pressure" without screaming "error".
const Color _kCountdownColor = Color(0xFFE57373);

class RecipientAvatar extends StatefulWidget {
  const RecipientAvatar({
    super.key,
    required this.deviceName,
    required this.deviceType,
    this.progress = 0.0,
    required this.mode,
    this.animate = true,
    this.connectionPath,
    this.connectionCandidates = const [],
    this.countdownDuration,
  });

  final String deviceName;
  final String deviceType;
  final double progress;
  final SendingStripMode mode;
  final bool animate;
  final ConnectionPathInfo? connectionPath;

  /// Candidate paths iroh is attempting, rendered as live active/idle rows
  /// under the device name while connecting. Empty (or ignored) once a path
  /// is established and the [connectionPath] badge takes over.
  final List<ConnectionCandidateInfo> connectionCandidates;

  /// When non-null, draws a thin red ring around the avatar that drains over
  /// [countdownDuration] starting the moment this duration first appears (or
  /// changes value).  Used to show the user how long is left before the
  /// connect / accept-decision phase times out on the wire.  When null, no
  /// countdown ring is rendered.
  final Duration? countdownDuration;

  @override
  State<RecipientAvatar> createState() => _RecipientAvatarState();
}

class _RecipientAvatarState extends State<RecipientAvatar>
    with TickerProviderStateMixin {
  late AnimationController _rippleController;
  late AnimationController _successController;
  late Animation<double> _scaleAnimation;
  bool _hasPlayedSuccess = false;

  AnimationController? _countdownController;

  @override
  void initState() {
    super.initState();
    _rippleController = AnimationController(
      vsync: this,
      duration: const Duration(milliseconds: 2500),
    );

    _successController = AnimationController(
      vsync: this,
      duration: const Duration(milliseconds: 600),
    );

    _scaleAnimation = TweenSequence<double>([
      TweenSequenceItem(
        tween: Tween<double>(
          begin: 1.0,
          end: 1.12,
        ).chain(CurveTween(curve: Curves.easeOut)),
        weight: 40,
      ),
      TweenSequenceItem(
        tween: Tween<double>(
          begin: 1.12,
          end: 1.0,
        ).chain(CurveTween(curve: Curves.easeOutBack)),
        weight: 60,
      ),
    ]).animate(_successController);

    _updateAnimation();
    _syncCountdown();
  }

  @override
  void didUpdateWidget(RecipientAvatar oldWidget) {
    super.didUpdateWidget(oldWidget);
    _updateAnimation();
    if (widget.countdownDuration != oldWidget.countdownDuration) {
      _syncCountdown();
    }

    if (widget.progress >= 1.0 &&
        !_hasPlayedSuccess &&
        widget.mode == SendingStripMode.transferring) {
      _hasPlayedSuccess = true;
      _successController.forward();
    } else if (widget.progress < 1.0) {
      _hasPlayedSuccess = false;
      if (_successController.value > 0 && !widget.animate) {
        _successController.reset();
      }
    }
  }

  void _updateAnimation() {
    final shouldAnimate =
        widget.animate &&
        (widget.mode == SendingStripMode.waitingOnRecipient ||
            widget.mode == SendingStripMode.looping);
    if (shouldAnimate && !_rippleController.isAnimating) {
      _rippleController.repeat();
    } else if (!shouldAnimate && _rippleController.isAnimating) {
      _rippleController.stop();
    }
  }

  /// Restart the countdown drain whenever the parent passes in a new (or
  /// first) duration.  We dispose the old controller because [duration] is
  /// final on AnimationController and we may be switching from a 30 s connect
  /// timer to a 120 s decision timer mid-flow.
  void _syncCountdown() {
    _countdownController?.dispose();
    _countdownController = null;
    final duration = widget.countdownDuration;
    if (duration == null || duration <= Duration.zero) {
      return;
    }
    _countdownController = AnimationController(
      vsync: this,
      duration: duration,
      value: 1.0,
    )..reverse();
  }

  @override
  void dispose() {
    _rippleController.dispose();
    _successController.dispose();
    _countdownController?.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final isPhone = widget.deviceType.toLowerCase() == 'phone';
    final icon = isPhone ? Icons.smartphone_rounded : Icons.laptop_mac_rounded;
    final isRippling = _rippleController.isAnimating;

    return Column(
      mainAxisSize: MainAxisSize.min,
      children: [
        SizedBox(
          width: 112,
          height: 112,
          child: Stack(
            alignment: Alignment.center,
            children: [
              // Ripple animation
              if (isRippling)
                AnimatedBuilder(
                  animation: _rippleController,
                  builder: (context, child) {
                    return Stack(
                      alignment: Alignment.center,
                      children: [
                        for (int i = 0; i < 2; i++)
                          _buildRipple(
                            (_rippleController.value + (i * 0.5)) % 1.0,
                          ),
                      ],
                    );
                  },
                ),

              // Static base ring (always shown for layout stability)
              Container(
                width: 112,
                height: 112,
                decoration: BoxDecoration(
                  shape: BoxShape.circle,
                  color: kAccentCyan.withValues(alpha: 0.12),
                ),
              ),

              // Progress Ring
              if (widget.mode == SendingStripMode.transferring)
                SizedBox(
                  width: 96,
                  height: 96,
                  child: CircularProgressIndicator(
                    value: widget.progress.clamp(0.01, 1.0),
                    strokeWidth: 4,
                    strokeCap: StrokeCap.round,
                    backgroundColor: context.wc.border.withValues(alpha: 0.3),
                    valueColor: const AlwaysStoppedAnimation<Color>(
                      kAccentCyan,
                    ),
                  ),
                ),

              // Countdown drain ring — drawn outside the ripple/progress so
              // it stays visible at the avatar's outer edge regardless of
              // the active mode.  Only painted when the parent has provided
              // a non-zero countdown duration (e.g. connect or decision
              // timeout still pending).
              if (_countdownController != null)
                AnimatedBuilder(
                  animation: _countdownController!,
                  builder: (context, _) {
                    return SizedBox(
                      width: 110,
                      height: 110,
                      child: CircularProgressIndicator(
                        value: _countdownController!.value.clamp(0.0, 1.0),
                        strokeWidth: 3,
                        strokeCap: StrokeCap.round,
                        backgroundColor: _kCountdownColor.withValues(
                          alpha: 0.12,
                        ),
                        valueColor: const AlwaysStoppedAnimation<Color>(
                          _kCountdownColor,
                        ),
                      ),
                    );
                  },
                ),

              // The Pop Container (Background and Icon)
              ScaleTransition(
                scale: _scaleAnimation,
                child: Stack(
                  alignment: Alignment.center,
                  children: [
                    // Background circle
                    Container(
                      width: 90,
                      height: 90,
                      decoration: BoxDecoration(
                        shape: BoxShape.circle,
                        color: context.wc.surface,
                        gradient: LinearGradient(
                          begin: Alignment.topLeft,
                          end: Alignment.bottomRight,
                          colors: [
                            context.wc.surface,
                            context.wc.bg.withValues(alpha: 0.5),
                          ],
                        ),
                        border: Border.all(
                          color: context.wc.border.withValues(alpha: 0.6),
                        ),
                        boxShadow: [
                          BoxShadow(
                            color: Colors.black.withValues(alpha: 0.05),
                            blurRadius: 14,
                            offset: const Offset(0, 4),
                          ),
                        ],
                      ),
                    ),

                    // Icon
                    Icon(
                      icon,
                      size: 40,
                      color: context.wc.ink.withValues(alpha: 0.9),
                    ),
                  ],
                ),
              ),
            ],
          ),
        ),
        const SizedBox(height: 16),
        Text(
          widget.deviceName,
          textAlign: TextAlign.center,
          style: wispSans(
            fontSize: 20,
            fontWeight: FontWeight.w700,
            color: context.wc.ink,
            letterSpacing: -0.4,
          ),
        ),
        if (_showConnectionBadge) ...[
          const SizedBox(height: 10),
          ConnectionPathBadge(path: widget.connectionPath, dense: false),
        ] else if (_showCandidateRows) ...[
          const SizedBox(height: 12),
          _CandidatePathList(candidates: widget.connectionCandidates),
        ],
      ],
    );
  }

  /// The resolved-path badge wins once iroh has settled on a direct/relay
  /// path; until then we show the live candidate rows instead.
  bool get _showConnectionBadge =>
      widget.connectionPath != null &&
      widget.connectionPath!.kind != ConnectionPathKind.unknown;

  /// Only show the "trying these paths" rows while connecting (the looping
  /// mode) and only once iroh has surfaced at least one candidate.
  bool get _showCandidateRows =>
      widget.mode == SendingStripMode.looping &&
      widget.connectionCandidates.isNotEmpty;

  Widget _buildRipple(double t) {
    // Starts at avatar edge (90) and expands to edge of footprint (112)
    final size = 90 + (22 * t);
    final opacity = (1.0 - t) * 0.25;

    return Container(
      width: size,
      height: size,
      decoration: BoxDecoration(
        shape: BoxShape.circle,
        border: Border.all(
          color: kAccentCyan.withValues(alpha: opacity),
          width: 1.5,
        ),
      ),
    );
  }
}

/// Live list of the transport paths iroh is attempting during Connecting.
///
/// iroh probes every candidate in parallel and only exposes active/idle (no
/// per-path failure or latency), so each row is just a dot + address: a filled
/// cyan dot for the active path, a hollow muted dot for the idle candidates.
/// The active candidate is hoisted to the top so a winning path is obvious.
class _CandidatePathList extends StatelessWidget {
  const _CandidatePathList({required this.candidates});

  final List<ConnectionCandidateInfo> candidates;

  @override
  Widget build(BuildContext context) {
    // Active first, then a stable order so rows don't jitter as iroh re-snapshots.
    final ordered = [...candidates]
      ..sort((a, b) {
        if (a.isActive != b.isActive) {
          return a.isActive ? -1 : 1;
        }
        return a.addr.compareTo(b.addr);
      });
    final activeCount = ordered.where((c) => c.isActive).length;

    return Column(
      mainAxisSize: MainAxisSize.min,
      children: [
        Text(
          activeCount > 0
              ? 'Connected via 1 of ${ordered.length} paths'
              : 'Trying ${ordered.length} '
                    '${ordered.length == 1 ? 'path' : 'paths'}…',
          style: wispSans(
            fontSize: 12,
            fontWeight: FontWeight.w600,
            color: context.wc.muted,
          ),
        ),
        const SizedBox(height: 8),
        for (final candidate in ordered) ...[
          _CandidatePathRow(candidate: candidate),
          const SizedBox(height: 4),
        ],
      ],
    );
  }
}

class _CandidatePathRow extends StatelessWidget {
  const _CandidatePathRow({required this.candidate});

  final ConnectionCandidateInfo candidate;

  @override
  Widget build(BuildContext context) {
    final active = candidate.isActive;
    final kindLabel = candidate.isRelay ? 'relay' : 'direct';
    final color = active ? context.wc.ink : context.wc.muted;

    return Row(
      mainAxisSize: MainAxisSize.min,
      children: [
        // Active → filled cyan dot; idle → hollow muted ring.
        Container(
          width: 8,
          height: 8,
          decoration: BoxDecoration(
            shape: BoxShape.circle,
            color: active ? kAccentCyanStrong : Colors.transparent,
            border: active
                ? null
                : Border.all(
                    color: context.wc.muted.withValues(alpha: 0.6),
                    width: 1.2,
                  ),
          ),
        ),
        const SizedBox(width: 8),
        ConstrainedBox(
          constraints: const BoxConstraints(maxWidth: 220),
          child: Text(
            candidate.displayHost,
            overflow: TextOverflow.ellipsis,
            style: wispMono(
              fontSize: 12,
              fontWeight: active ? FontWeight.w700 : FontWeight.w500,
              color: color,
            ),
          ),
        ),
        const SizedBox(width: 6),
        Text(
          '($kindLabel)',
          style: wispSans(
            fontSize: 11,
            fontWeight: FontWeight.w500,
            color: context.wc.muted.withValues(alpha: 0.8),
          ),
        ),
      ],
    );
  }
}
