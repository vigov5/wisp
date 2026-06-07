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

const Color kPrimary = kAccentCyanStrong;
const Color kPrimaryDark = kAccentCyanStrong;
const Color kPrimaryLight = Color(0xFFA5F3FC);
const Color kSurface2 = Color(0xFFFAFBFB);

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

ThemeData buildWispTheme() {
  final colorScheme =
      ColorScheme.fromSeed(
        seedColor: kAccentCyan,
        brightness: Brightness.light,
      ).copyWith(
        primary: kAccentCyanStrong,
        secondary: kAccentWarm,
        surface: kSurface,
        onSurface: kInk,
        outline: kBorder,
      );

  final textTheme = TextTheme(
    headlineLarge: wispSans(
      fontSize: 30,
      fontWeight: FontWeight.w700,
      color: kInk,
      letterSpacing: -0.8,
      height: 1.15,
    ),
    headlineMedium: wispSans(
      fontSize: 22,
      fontWeight: FontWeight.w700,
      color: kInk,
      letterSpacing: -0.5,
      height: 1.2,
    ),
    titleLarge: wispSans(
      fontSize: 17,
      fontWeight: FontWeight.w600,
      color: kInk,
      letterSpacing: -0.2,
    ),
    titleMedium: wispSans(
      fontSize: 14,
      fontWeight: FontWeight.w600,
      color: kInk,
    ),
    bodyLarge: wispSans(
      fontSize: 14,
      fontWeight: FontWeight.w400,
      color: kInk,
      height: 1.5,
    ),
    bodyMedium: wispSans(
      fontSize: 13,
      fontWeight: FontWeight.w400,
      color: kMuted,
      height: 1.5,
    ),
    labelLarge: wispSans(
      fontSize: 13,
      fontWeight: FontWeight.w500,
      color: kInk,
    ),
    labelMedium: wispSans(
      fontSize: 11.5,
      fontWeight: FontWeight.w500,
      color: kMuted,
      letterSpacing: 0.1,
    ),
  );

  return ThemeData(
    useMaterial3: true,
    fontFamily: kFontFamily,
    colorScheme: colorScheme,
    scaffoldBackgroundColor: kBg,
    textTheme: textTheme,
    cardTheme: CardThemeData(
      color: kSurface,
      elevation: 0,
      shape: RoundedRectangleBorder(
        borderRadius: BorderRadius.circular(12),
        side: const BorderSide(color: kBorder),
      ),
      margin: EdgeInsets.zero,
    ),
    dividerColor: kBorder,
    dividerTheme: const DividerThemeData(color: kBorder, space: 0),
    inputDecorationTheme: InputDecorationTheme(
      filled: true,
      fillColor: kSurface,
      hoverColor: kSurface,
      contentPadding: const EdgeInsets.symmetric(horizontal: 14, vertical: 14),
      hintStyle: wispSans(color: kSubtle, fontSize: 14),
      border: OutlineInputBorder(
        borderRadius: BorderRadius.circular(8),
        borderSide: const BorderSide(color: kBorder),
      ),
      enabledBorder: OutlineInputBorder(
        borderRadius: BorderRadius.circular(8),
        borderSide: const BorderSide(color: kBorder),
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
        backgroundColor: kInk,
        foregroundColor: Colors.white,
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
        foregroundColor: kInk,
        side: const BorderSide(color: kBorder, width: 1.5),
        minimumSize: const Size(80, 38),
        padding: const EdgeInsets.symmetric(horizontal: 20, vertical: 10),
        shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(8)),
        textStyle: wispSans(fontSize: 13.5, fontWeight: FontWeight.w600),
      ),
    ),
    textButtonTheme: TextButtonThemeData(
      style: TextButton.styleFrom(
        foregroundColor: kMuted,
        minimumSize: const Size(0, 36),
        padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 8),
        textStyle: wispSans(fontSize: 13, fontWeight: FontWeight.w500),
        shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(8)),
      ),
    ),
    iconButtonTheme: IconButtonThemeData(
      style: IconButton.styleFrom(
        foregroundColor: kMuted,
        minimumSize: const Size(34, 34),
        shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(8)),
      ),
    ),
  );
}
