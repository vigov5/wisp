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
    this.countdownDuration,
  });

  final String deviceName;
  final String deviceType;
  final double progress;
  final SendingStripMode mode;
  final bool animate;
  final ConnectionPathInfo? connectionPath;

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
    _countdownController =
        AnimationController(vsync: this, duration: duration, value: 1.0)
          ..reverse();
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
                    backgroundColor: kBorder.withValues(alpha: 0.3),
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
                        color: kSurface,
                        gradient: LinearGradient(
                          begin: Alignment.topLeft,
                          end: Alignment.bottomRight,
                          colors: [kSurface, kBg.withValues(alpha: 0.5)],
                        ),
                        border: Border.all(
                          color: kBorder.withValues(alpha: 0.6),
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
                    Icon(icon, size: 40, color: kInk.withValues(alpha: 0.9)),
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
            color: kInk,
            letterSpacing: -0.4,
          ),
        ),
        if (widget.connectionPath != null &&
            widget.connectionPath!.kind != ConnectionPathKind.unknown) ...[
          const SizedBox(height: 10),
          ConnectionPathBadge(path: widget.connectionPath, dense: false),
        ],
      ],
    );
  }

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
