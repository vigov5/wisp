import 'dart:io';

import 'package:flutter/foundation.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:package_info_plus/package_info_plus.dart';
import 'package:pub_semver/pub_semver.dart';

import '../domain/update_status.dart';
import 'github_release_api.dart';
import 'update_installer.dart';
import 'update_providers.dart';
import 'update_repository.dart';

/// Drives the update lifecycle: check → notify → (Windows) download + install.
///
/// Auto-checks on startup swallow errors silently; manual checks surface them
/// via [UpdatePhase.error] so the UI can show feedback.
class UpdateController extends Notifier<UpdateState> {
  Version? _currentVersion;

  // Read lazily (not in build()) so merely watching/listening this provider —
  // e.g. WispApp's ref.listen — never forces a read of the bootstrap-only
  // updateRepositoryProvider. Only an actual check/install touches it.
  GithubReleaseApi get _api => ref.read(githubReleaseApiProvider);
  UpdateInstaller get _installer => ref.read(updateInstallerProvider);
  UpdateRepository get _repository => ref.read(updateRepositoryProvider);

  @override
  UpdateState build() => const UpdateState();

  Future<Version> _resolveCurrentVersion() async {
    final cached = _currentVersion;
    if (cached != null) return cached;
    final info = await PackageInfo.fromPlatform();
    final parsed = _tryParse(info.version);
    _currentVersion = parsed;
    return parsed;
  }

  /// Checks GitHub for a newer release. [manual] checks bypass the startup
  /// throttle and surface errors; automatic checks respect the toggle/throttle
  /// and fail quietly.
  Future<void> checkForUpdates({bool manual = false}) async {
    final now = DateTime.now();
    if (!manual && !_repository.shouldAutoCheck(now)) return;
    if (state.phase == UpdatePhase.checking ||
        state.phase == UpdatePhase.downloading) {
      return;
    }

    state = state.copyWith(
      phase: UpdatePhase.checking,
      clearError: true,
      clearRelease: true,
    );
    try {
      final release = await _api.fetchLatest();
      await _repository.markChecked(now);
      final current = await _resolveCurrentVersion();

      final isNewer = release.version > current;
      final skipped =
          !manual && release.tagName == _repository.skippedVersion();
      if (isNewer && !skipped) {
        state = state.copyWith(phase: UpdatePhase.available, release: release);
      } else {
        state = state.copyWith(phase: UpdatePhase.upToDate, clearRelease: true);
      }
    } catch (error) {
      debugPrint('[update] check failed: $error');
      if (manual) {
        state = state.copyWith(
          phase: UpdatePhase.error,
          errorMessage: 'Could not check for updates. Please try again later.',
        );
      } else {
        // Stay idle on a silent failure so nothing surfaces to the user.
        state = state.copyWith(phase: UpdatePhase.idle);
      }
    }
  }

  /// Windows: downloads the installer asset and launches it (quitting the app).
  /// On other platforms, or when no installer asset is present, hands off to the
  /// external update destination (Play Store on Android, Releases page else).
  Future<void> downloadAndInstall() async {
    final release = state.release;
    if (release == null) return;

    final asset = release.assetForCurrentPlatform();
    if (!Platform.isWindows || asset == null) {
      await _openExternalUpdate();
      return;
    }

    state = state.copyWith(
      phase: UpdatePhase.downloading,
      downloadProgress: 0,
      clearError: true,
    );
    try {
      final file = await _installer.download(
        asset,
        onProgress: (progress) {
          state = state.copyWith(downloadProgress: progress);
        },
      );
      state = state.copyWith(phase: UpdatePhase.readyToInstall);
      await _installer.runWindowsInstaller(file);
    } catch (error) {
      debugPrint('[update] download/install failed: $error');
      state = state.copyWith(
        phase: UpdatePhase.error,
        errorMessage: 'Download failed. You can update manually instead.',
      );
    }
  }

  /// Opens the platform's update destination for the "update manually" /
  /// "Download" paths: the Play Store listing on Android, the GitHub Releases
  /// page everywhere else.
  Future<void> openUpdatePage() => _openExternalUpdate();

  Future<void> _openExternalUpdate() {
    return Platform.isAndroid
        ? _installer.openPlayStore()
        : _installer.openReleasesPage();
  }

  /// Suppresses notifications for the current available release until a newer
  /// one ships.
  Future<void> skipCurrentVersion() async {
    final release = state.release;
    if (release != null) {
      await _repository.setSkippedVersion(release.tagName);
    }
    dismiss();
  }

  void dismiss() {
    state = state.copyWith(phase: UpdatePhase.idle, clearRelease: true);
  }

  bool checkOnStartup() => _repository.checkOnStartup();

  Future<void> setCheckOnStartup(bool value) =>
      _repository.setCheckOnStartup(value);

  static Version _tryParse(String value) {
    try {
      return Version.parse(value);
    } on FormatException {
      return Version.none;
    }
  }
}
