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

  /// The check or download failed. [UpdateState.errorMessage] is set.
  error,
}

class UpdateState {
  const UpdateState({
    this.phase = UpdatePhase.idle,
    this.release,
    this.downloadProgress,
    this.errorMessage,
  });

  final UpdatePhase phase;
  final UpdateRelease? release;

  /// 0.0–1.0 while [phase] is [UpdatePhase.downloading], or `null` when the
  /// total size is unknown (indeterminate progress).
  final double? downloadProgress;
  final String? errorMessage;

  UpdateState copyWith({
    UpdatePhase? phase,
    UpdateRelease? release,
    double? downloadProgress,
    String? errorMessage,
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
    );
  }
}
