import 'package:flutter/material.dart';

// Bundled UI typeface (assets/fonts/NotoSans-Variable.ttf). Noto Sans renders
// Vietnamese diacritics and broad Unicode correctly and identically on every
// platform, replacing the per-OS system default that mispositioned Vietnamese
// marks on Android.
const String kFontFamily = 'Noto Sans';

// Bundled monospace face (assets/fonts/NotoSansMono-Variable.ttf) for codes,
// device IDs, and the send-text editor. Replaces 'Courier New', which is
// absent on Android and fell back to a per-OS mono that mangled Vietnamese.
const String kMonoFontFamily = 'Noto Sans Mono';

const Color kBg = Color(0xFFF3F4F4);
const Color kSurface = Color(0xFFFFFFFF);
const Color kFill = Color(0xFFEEF0F0);
const Color kBorder = Color(0xFFDDE2E3);
const Color kInk = Color(0xFF141414);
const Color kMuted = Color(0xFF8A8A8A);
const Color kSubtle = Color(0xFFBBBBBB);
const Color kCodeBg = Color(0xFF191919);
// Wisp accent palette — derived from the launcher icon's cyan
// (#06B6D4, Tailwind cyan-500). `Strong` is cyan-600 for primary buttons
// where the icon shade would feel a touch loud; `Light` is cyan-200 for
// soft surfaces. Hover/Pressed are alpha-overlaid versions of the base.
const Color kAccentCyan = Color(0xFF06B6D4);
const Color kAccentCyanStrong = Color(0xFF0891B2);
const Color kAccentCyanHover = Color(0x1F06B6D4);
const Color kAccentCyanPressed = Color(0x3306B6D4);
const Color kAccentWarm = Color(0xFFF2E7BA);
const Color kAccentWarmSurface = Color(0x14F2E7BA);
const Color kAccentDirect = Color(0xFF4DA372);
const Color kAccentRelay = Color(0xFFC78F2A);

// Semantic status colors. `kDanger` is the destructive-action red named in the
// button conventions below (Decline / Cancel / Delete); it was previously
// hand-written as `Color(0xFFB34A4A)` at ~13 call sites — use this const
// instead. `kError` is the invalid-input red used by the input theme's error
// borders.
const Color kDanger = Color(0xFFB34A4A);
const Color kError = Color(0xFFCC3333);

const Color kPrimary = kAccentCyanStrong;
const Color kPrimaryDark = kAccentCyanStrong;
const Color kPrimaryLight = Color(0xFFA5F3FC);
const Color kSurface2 = Color(0xFFFAFBFB);

// ─── Spacing & radius scale ──────────────────────────────────────────────────
// A formal scale for gaps/padding and corner radii. Historically these were
// bare literals everywhere; the app's de-facto spine is a 4-based scale
// (4/8/12/16/24) plus a couple of fine adjustments. Prefer these named steps
// over raw numbers in new/edited code — the web receiver mirrors the same values
// (web/style.css --space-* / --radius-*). Off-grid one-offs (10/14/18/…) should
// fold into the nearest step during cleanup. See theme/design-tokens.md.
abstract final class WispSpace {
  static const double hair = 2; // hairline nudges
  static const double xs = 4;
  static const double tiny = 6;
  static const double sm = 8;
  static const double md = 12; // the most common gap
  static const double lg = 16;
  static const double xl = 20;
  static const double xxl = 24;
  static const double xxxl = 32;
}

abstract final class WispRadius {
  static const double sm = 6; // chips / small dialogs
  static const double control =
      8; // buttons / inputs (matches component themes)
  static const double card = 12; // cards / panels (matches cardTheme)
  static const double surface = 16; // large surfaces / sheets
  static const double sheet = 24; // bottom sheets / drop zones
  static const double pill = 999;
}

// ─── Theme-varying neutrals ──────────────────────────────────────────────────
// The neutral palette above (kBg, kSurface, kInk, kMuted, …) is the *light*
// theme's source of truth. Because those are compile-time `const`, they can't
// flip at runtime — so widgets read the active neutrals through this
// [ThemeExtension] instead (via `context.wc.<field>`), and the dark theme
// supplies a parallel set. Accents (kAccentCyan*, kAccentWarm, kAccentDirect,
// kAccentRelay) stay constant across both themes.
@immutable
class WispColors extends ThemeExtension<WispColors> {
  const WispColors({
    required this.bg,
    required this.surface,
    required this.surface2,
    required this.fill,
    required this.border,
    required this.ink,
    required this.muted,
    required this.subtle,
    required this.codeBg,
    required this.accentFg,
  });

  /// App/scaffold background.
  final Color bg;

  /// Primary raised surface (cards, tiles, fields).
  final Color surface;

  /// Slightly-off surface for nested/secondary panels.
  final Color surface2;

  /// Muted fill for inert chips/wells.
  final Color fill;

  /// Hairline borders and dividers.
  final Color border;

  /// Primary text / high-emphasis foreground.
  final Color ink;

  /// Secondary text / low-emphasis icons.
  final Color muted;

  /// Tertiary text / hints / disabled.
  final Color subtle;

  /// Dark surface behind code / monospace blocks.
  final Color codeBg;

  /// Accent used as a foreground (text/icon) color on a surface. Brighter on
  /// dark, where cyan-600 ([kAccentCyanStrong]) would be too low-contrast.
  /// (Where an accent is a *filled button background* with white text, keep
  /// [kAccentCyanStrong] directly — it reads fine on both themes.)
  final Color accentFg;

  @override
  WispColors copyWith({
    Color? bg,
    Color? surface,
    Color? surface2,
    Color? fill,
    Color? border,
    Color? ink,
    Color? muted,
    Color? subtle,
    Color? codeBg,
    Color? accentFg,
  }) {
    return WispColors(
      bg: bg ?? this.bg,
      surface: surface ?? this.surface,
      surface2: surface2 ?? this.surface2,
      fill: fill ?? this.fill,
      border: border ?? this.border,
      ink: ink ?? this.ink,
      muted: muted ?? this.muted,
      subtle: subtle ?? this.subtle,
      codeBg: codeBg ?? this.codeBg,
      accentFg: accentFg ?? this.accentFg,
    );
  }

  @override
  WispColors lerp(covariant ThemeExtension<WispColors>? other, double t) {
    if (other is! WispColors) return this;
    return WispColors(
      bg: Color.lerp(bg, other.bg, t)!,
      surface: Color.lerp(surface, other.surface, t)!,
      surface2: Color.lerp(surface2, other.surface2, t)!,
      fill: Color.lerp(fill, other.fill, t)!,
      border: Color.lerp(border, other.border, t)!,
      ink: Color.lerp(ink, other.ink, t)!,
      muted: Color.lerp(muted, other.muted, t)!,
      subtle: Color.lerp(subtle, other.subtle, t)!,
      codeBg: Color.lerp(codeBg, other.codeBg, t)!,
      accentFg: Color.lerp(accentFg, other.accentFg, t)!,
    );
  }
}

const WispColors _lightColors = WispColors(
  bg: kBg,
  surface: kSurface,
  surface2: kSurface2,
  fill: kFill,
  border: kBorder,
  ink: kInk,
  muted: kMuted,
  subtle: kSubtle,
  codeBg: kCodeBg,
  accentFg: kAccentCyanStrong,
);

const WispColors _darkColors = WispColors(
  bg: Color(0xFF0F1213),
  surface: Color(0xFF171A1B),
  surface2: Color(0xFF1D2122),
  fill: Color(0xFF23282A),
  border: Color(0xFF2E3436),
  ink: Color(0xFFECEEEE),
  muted: Color(0xFF9AA1A2),
  subtle: Color(0xFF64696B),
  codeBg: Color(0xFF0A0C0D),
  accentFg: kAccentCyan,
);

/// Reads the active [WispColors] for the current theme. Use `context.wc.ink`,
/// `context.wc.border`, etc. instead of the raw `k*` neutral constants so the
/// color flips with light/dark.
///
/// Falls back to the light palette when no [WispColors] is registered on the
/// ambient theme. In the running app every `MaterialApp` is built via
/// [buildWispTheme], which always registers the extension, so the fallback only
/// applies to isolated contexts (e.g. widget tests that pump a bare
/// `MaterialApp`) — where the historical light palette is the right default.
extension WispColorsX on BuildContext {
  WispColors get wc => Theme.of(this).extension<WispColors>() ?? _lightColors;
}

// ─── Button conventions ─────────────────────────────────────────────────────
// Action buttons in this app fall into four styles. Reach for an existing one
// before inventing a new colour/alpha combination.
//
// Two cyans, two jobs: a *filled* primary CTA uses the darker
// [kAccentCyanStrong]; an *inline text* action uses the brighter
// [kAccentCyan]. Don't mix them up — that distinction is the whole system.
//
// 1. Primary — the dominant action (Done, Accept, Send). `FilledButton` with a
//    solid fill (`kPrimary`/`kAccentCyanStrong`, white text). One per row.
//
// 2. Soft-tint — secondary or destructive actions that should read as a real,
//    tappable surface without competing with the primary. The recipe is:
//        foregroundColor: <accent>
//        backgroundColor: <accent>.withValues(alpha: 0.08)
//        side:            <accent>.withValues(alpha: 0.15)
//    Destructive uses the red accent (0xFFB34A4A) — see "Cancel transfer" /
//    "Decline". Affirmative-but-secondary uses [kAccentCyan]/[kAccentCyanStrong]
//    — see "Show in Files" on the transfer result card, or "Choose" beside the
//    download-root field. Do NOT hand-pick other alphas; keep 0.08 / 0.15 so
//    the tint language stays consistent.
//
// 3. Neutral outline — low-emphasis escape hatches (e.g. "Done" beside a
//    "Retry"). `OutlinedButton` with `kInk` text and a `kBorder` side, no fill.
//
// 4. Inline text action — a low-chrome action that sits next to a section
//    title or inside an action cluster (e.g. "Scan QR" / "Stop" / "Rescan" on
//    the send screen, "Add files" / "Add folders", "Clear" on the Storage row,
//    "Re-check" on the Update row). `TextButton.icon` with:
//        foregroundColor: kAccentCyan          // the *bright* cyan, not Strong
//        label: wispSans(fontSize: 13, fontWeight: FontWeight.w500)
//        icon size 18
//    Make it compact when it hugs a title (padding: EdgeInsets.zero,
//    minimumSize: Size.zero, tapTargetSize: shrinkWrap). When the button can be
//    disabled (e.g. "Clear"), let the colour come from the button's
//    foreground/`disabledForegroundColor` rather than hard-coding it on the
//    label, so the disabled state still greys out.
// ─────────────────────────────────────────────────────────────────────────────

TextStyle wispSans({
  required double fontSize,
  FontWeight fontWeight = FontWeight.w400,
  Color? color,
  double? height,
  double? letterSpacing,
}) {
  return TextStyle(
    fontFamily: kFontFamily,
    fontSize: fontSize,
    fontWeight: fontWeight,
    color: color,
    height: height,
    letterSpacing: letterSpacing,
  );
}

TextStyle wispMono({
  required double fontSize,
  FontWeight fontWeight = FontWeight.w600,
  Color? color,
  double? letterSpacing,
}) {
  return TextStyle(
    fontFamily: kMonoFontFamily,
    fontFamilyFallback: const ['Courier New', 'monospace'],
    fontSize: fontSize,
    fontWeight: fontWeight,
    color: color,
    letterSpacing: letterSpacing,
  );
}

ThemeData buildWispTheme([Brightness brightness = Brightness.light]) {
  final isDark = brightness == Brightness.dark;
  final c = isDark ? _darkColors : _lightColors;

  final colorScheme =
      ColorScheme.fromSeed(
        seedColor: kAccentCyan,
        brightness: brightness,
      ).copyWith(
        primary: kAccentCyanStrong,
        secondary: kAccentWarm,
        surface: c.surface,
        onSurface: c.ink,
        outline: c.border,
      );

  final textTheme = TextTheme(
    headlineLarge: wispSans(
      fontSize: 30,
      fontWeight: FontWeight.w700,
      color: c.ink,
      letterSpacing: -0.8,
      height: 1.15,
    ),
    headlineMedium: wispSans(
      fontSize: 22,
      fontWeight: FontWeight.w700,
      color: c.ink,
      letterSpacing: -0.5,
      height: 1.2,
    ),
    titleLarge: wispSans(
      fontSize: 17,
      fontWeight: FontWeight.w600,
      color: c.ink,
      letterSpacing: -0.2,
    ),
    titleMedium: wispSans(
      fontSize: 14,
      fontWeight: FontWeight.w600,
      color: c.ink,
    ),
    bodyLarge: wispSans(
      fontSize: 14,
      fontWeight: FontWeight.w400,
      color: c.ink,
      height: 1.5,
    ),
    bodyMedium: wispSans(
      fontSize: 13,
      fontWeight: FontWeight.w400,
      color: c.muted,
      height: 1.5,
    ),
    labelLarge: wispSans(
      fontSize: 13,
      fontWeight: FontWeight.w500,
      color: c.ink,
    ),
    labelMedium: wispSans(
      fontSize: 11.5,
      fontWeight: FontWeight.w500,
      color: c.muted,
      letterSpacing: 0.1,
    ),
  );

  return ThemeData(
    brightness: brightness,
    useMaterial3: true,
    fontFamily: kFontFamily,
    colorScheme: colorScheme,
    scaffoldBackgroundColor: c.bg,
    textTheme: textTheme,
    extensions: [c],
    cardTheme: CardThemeData(
      color: c.surface,
      elevation: 0,
      shape: RoundedRectangleBorder(
        borderRadius: BorderRadius.circular(12),
        side: BorderSide(color: c.border),
      ),
      margin: EdgeInsets.zero,
    ),
    dividerColor: c.border,
    dividerTheme: DividerThemeData(color: c.border, space: 0),
    inputDecorationTheme: InputDecorationTheme(
      filled: true,
      fillColor: c.surface,
      hoverColor: c.surface,
      contentPadding: const EdgeInsets.symmetric(horizontal: 14, vertical: 14),
      hintStyle: wispSans(color: c.subtle, fontSize: 14),
      border: OutlineInputBorder(
        borderRadius: BorderRadius.circular(8),
        borderSide: BorderSide(color: c.border),
      ),
      enabledBorder: OutlineInputBorder(
        borderRadius: BorderRadius.circular(8),
        borderSide: BorderSide(color: c.border),
      ),
      focusedBorder: OutlineInputBorder(
        borderRadius: BorderRadius.circular(8),
        borderSide: const BorderSide(color: kAccentCyanStrong, width: 1.4),
      ),
      errorBorder: OutlineInputBorder(
        borderRadius: BorderRadius.circular(8),
        borderSide: const BorderSide(color: Color(0xFFCC3333)),
      ),
      focusedErrorBorder: OutlineInputBorder(
        borderRadius: BorderRadius.circular(8),
        borderSide: const BorderSide(color: Color(0xFFCC3333), width: 1.5),
      ),
    ),
    filledButtonTheme: FilledButtonThemeData(
      style: FilledButton.styleFrom(
        // The default filled CTA uses the ink neutral as its fill. On dark that
        // becomes a near-white button, so the label must flip to a dark ink.
        backgroundColor: c.ink,
        foregroundColor: isDark ? c.bg : Colors.white,
        minimumSize: const Size(80, 38),
        padding: const EdgeInsets.symmetric(horizontal: 20, vertical: 10),
        shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(8)),
        textStyle: wispSans(fontSize: 13.5, fontWeight: FontWeight.w600),
        elevation: 0,
        shadowColor: Colors.transparent,
      ),
    ),
    outlinedButtonTheme: OutlinedButtonThemeData(
      style: OutlinedButton.styleFrom(
        foregroundColor: c.ink,
        side: BorderSide(color: c.border, width: 1.5),
        minimumSize: const Size(80, 38),
        padding: const EdgeInsets.symmetric(horizontal: 20, vertical: 10),
        shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(8)),
        textStyle: wispSans(fontSize: 13.5, fontWeight: FontWeight.w600),
      ),
    ),
    textButtonTheme: TextButtonThemeData(
      style: TextButton.styleFrom(
        foregroundColor: c.muted,
        minimumSize: const Size(0, 36),
        padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 8),
        textStyle: wispSans(fontSize: 13, fontWeight: FontWeight.w500),
        shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(8)),
      ),
    ),
    iconButtonTheme: IconButtonThemeData(
      style: IconButton.styleFrom(
        foregroundColor: c.muted,
        minimumSize: const Size(34, 34),
        shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(8)),
      ),
    ),
  );
}
