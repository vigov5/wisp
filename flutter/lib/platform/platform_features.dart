import 'package:flutter/foundation.dart';

bool get isDesktopPlatform {
  if (kIsWeb) {
    return false;
  }

  switch (defaultTargetPlatform) {
    case TargetPlatform.macOS:
    case TargetPlatform.windows:
    case TargetPlatform.linux:
      return true;
    case TargetPlatform.android:
    case TargetPlatform.iOS:
    case TargetPlatform.fuchsia:
      return false;
  }
}

/// True when the binary was compiled for F-Droid:
/// `flutter build apk --release --dart-define=WISP_FDROID=true`.
///
/// F-Droid ships and updates the APKs it builds itself, and its reviewers
/// expect an app not to route users around that channel, so the build sold
/// through F-Droid compiles the in-app updater out entirely — no call to
/// api.github.com on startup, and no Play Store deep link. Every other build
/// (Play, GitHub releases, desktop) is unaffected.
const bool kIsFdroidBuild = bool.fromEnvironment('WISP_FDROID');

/// Whether the in-app updater (GitHub release check, download/install, Play
/// Store deep link) should run at all. See [kIsFdroidBuild].
bool get hasInAppUpdates => !kIsFdroidBuild;
