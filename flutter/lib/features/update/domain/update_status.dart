import 'update_release.dart';

enum UpdatePhase {
  /// No check has run yet, or the result was dismissed.
  idle,

  /// A check is in flight.
  checking,

  /// The running build is the latest available.
  upToDate,

  /// A newer release exists. [UpdateState.release] is non-null.
  available,

  /// The installer is being downloaded. [UpdateState.downloadProgress] is set.
  downloading,

  /// The installer finished downloading and is about to launch.
  readyToInstall,

  /// The installer downloaded but couldn't be launched automatically (e.g. the
  /// user cancelled the elevation prompt). It's sitting on disk and the user
  /// must run it themselves. [UpdateState.installerPath] points at the file and
  /// its folder has been revealed in the file manager.
  manualInstall,

  /// The check or download failed. [UpdateState.errorMessage] is set.
  error,
}

class UpdateState {
  const UpdateState({
    this.phase = UpdatePhase.idle,
    this.release,
    this.downloadProgress,
    this.errorMessage,
    this.installerPath,
  });

  final UpdatePhase phase;
  final UpdateRelease? release;

  /// 0.0–1.0 while [phase] is [UpdatePhase.downloading], or `null` when the
  /// total size is unknown (indeterminate progress).
  final double? downloadProgress;
  final String? errorMessage;

  /// Absolute path of the downloaded installer, set once [phase] reaches
  /// [UpdatePhase.manualInstall] so the UI can name the file and reopen its
  /// folder.
  final String? installerPath;

  UpdateState copyWith({
    UpdatePhase? phase,
    UpdateRelease? release,
    double? downloadProgress,
    String? errorMessage,
    String? installerPath,
    bool clearRelease = false,
    bool clearError = false,
    bool clearProgress = false,
  }) {
    return UpdateState(
      phase: phase ?? this.phase,
      release: clearRelease ? null : (release ?? this.release),
      downloadProgress: clearProgress
          ? null
          : (downloadProgress ?? this.downloadProgress),
      errorMessage: clearError ? null : (errorMessage ?? this.errorMessage),
      installerPath: installerPath ?? this.installerPath,
    );
  }
}
