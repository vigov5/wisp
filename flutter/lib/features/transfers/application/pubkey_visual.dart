import 'package:flutter/material.dart';

import '../../../theme/drift_theme.dart';

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
