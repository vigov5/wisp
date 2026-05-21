import 'package:flutter/material.dart';

import '../../../theme/wisp_theme.dart';

/// Stable HSL color derived from an iroh EndpointId / pubkey. Same id always
/// yields the same color across the app, so users can visually disambiguate
/// devices that share a name (or recognize their own identity).
Color colorFromPubkey(String endpointId) {
  if (endpointId.isEmpty) return kMuted;
  final hue = endpointId.codeUnits.fold<int>(0, (a, b) => (a + b) % 360);
  return HSLColor.fromAHSL(1, hue.toDouble(), 0.55, 0.55).toColor();
}

/// Truncated "AAAA…ZZZZ" representation for compact display. [headChars] and
/// [tailChars] control how many characters are kept at each end (default 4/4
/// for tight tiles; pass larger values where there's room).
String shortPubkey(String endpointId, {int headChars = 4, int tailChars = 4}) {
  final upper = endpointId.toUpperCase();
  if (upper.length <= headChars + tailChars + 1) {
    return upper;
  }
  return '${upper.substring(0, headChars)}…${upper.substring(upper.length - tailChars)}';
}

/// Compact identity chip rendering [shortPubkey] inside a tinted pill keyed
/// off [colorFromPubkey].  Same visual contract used by recent-device tiles,
/// nearby tiles, the QR-paired tile, settings, and the saved-devices page —
/// keep them in sync so a given pubkey looks the same everywhere.
class PubkeyBadge extends StatelessWidget {
  const PubkeyBadge({
    super.key,
    required this.endpointId,
    this.size = PubkeyBadgeSize.medium,
    this.tooltip,
  });

  final String endpointId;
  final PubkeyBadgeSize size;
  final String? tooltip;

  @override
  Widget build(BuildContext context) {
    final color = colorFromPubkey(endpointId);
    final textColor = HSLColor.fromColor(color).withLightness(0.32).toColor();
    final spec = _spec(size);

    final badge = Container(
      padding: spec.padding,
      decoration: BoxDecoration(
        color: color.withValues(alpha: 0.15),
        borderRadius: BorderRadius.circular(spec.radius),
        border: Border.all(
          color: color.withValues(alpha: 0.45),
          width: spec.borderWidth,
        ),
      ),
      child: Text(
        shortPubkey(
          endpointId,
          headChars: spec.headChars,
          tailChars: spec.tailChars,
        ),
        style: wispSans(
          fontSize: spec.fontSize,
          fontWeight: FontWeight.w700,
          color: textColor,
          letterSpacing: 0.4,
        ),
        maxLines: 1,
      ),
    );

    if (tooltip == null) return badge;
    return Tooltip(message: tooltip!, child: badge);
  }

  static _BadgeSpec _spec(PubkeyBadgeSize size) {
    switch (size) {
      case PubkeyBadgeSize.tiny:
        return const _BadgeSpec(
          padding: EdgeInsets.symmetric(horizontal: 6, vertical: 1),
          radius: 5,
          borderWidth: 0.6,
          fontSize: 9.5,
          headChars: 4,
          tailChars: 4,
        );
      case PubkeyBadgeSize.small:
        return const _BadgeSpec(
          padding: EdgeInsets.symmetric(horizontal: 6, vertical: 2),
          radius: 6,
          borderWidth: 0.8,
          fontSize: 10.5,
          headChars: 4,
          tailChars: 4,
        );
      case PubkeyBadgeSize.medium:
        return const _BadgeSpec(
          padding: EdgeInsets.symmetric(horizontal: 6, vertical: 2),
          radius: 6,
          borderWidth: 0.8,
          fontSize: 11,
          headChars: 4,
          tailChars: 4,
        );
      case PubkeyBadgeSize.large:
        return const _BadgeSpec(
          padding: EdgeInsets.symmetric(horizontal: 10, vertical: 6),
          radius: 8,
          borderWidth: 0.8,
          fontSize: 12.5,
          headChars: 8,
          tailChars: 8,
        );
    }
  }
}

enum PubkeyBadgeSize { tiny, small, medium, large }

class _BadgeSpec {
  const _BadgeSpec({
    required this.padding,
    required this.radius,
    required this.borderWidth,
    required this.fontSize,
    required this.headChars,
    required this.tailChars,
  });

  final EdgeInsetsGeometry padding;
  final double radius;
  final double borderWidth;
  final double fontSize;
  final int headChars;
  final int tailChars;
}
