import 'dart:io';

import 'package:flutter/foundation.dart';
import 'package:http/http.dart' as http;
import 'package:path_provider/path_provider.dart';
import 'package:url_launcher/url_launcher.dart';
import 'package:window_manager/window_manager.dart';

import '../domain/update_release.dart';

const String releasesPageUrl = 'https://github.com/vigov5/wisp/releases';

/// Matches the Android `applicationId` in android/app/build.gradle.kts.
const String _androidPackageId = 'dev.vigov5.wisp';
const String playStoreWebUrl =
    'https://play.google.com/store/apps/details?id=$_androidPackageId';

/// Downloads release installers and launches them. Windows is the only
/// platform with a true in-app install path; macOS/Linux open the Releases
/// page in a browser instead.
class UpdateInstaller {
  UpdateInstaller({http.Client? client}) : _client = client ?? http.Client();

  final http.Client _client;

  /// Streams [asset] to a file in the temp directory, reporting fractional
  /// progress (0.0–1.0) when the server advertises a content length.
  Future<File> download(
    ReleaseAsset asset, {
    void Function(double? progress)? onProgress,
  }) async {
    final tmpDir = await getTemporaryDirectory();
    final file = File('${tmpDir.path}${Platform.pathSeparator}${asset.name}');

    final request = http.Request('GET', Uri.parse(asset.downloadUrl));
    request.headers['User-Agent'] = 'Wisp';
    final response = await _client.send(request);
    if (response.statusCode != 200) {
      throw http.ClientException(
        'Download failed: HTTP ${response.statusCode}',
        Uri.parse(asset.downloadUrl),
      );
    }

    final total = response.contentLength ?? asset.sizeBytes;
    var received = 0;
    final sink = file.openWrite();
    try {
      await for (final chunk in response.stream) {
        sink.add(chunk);
        received += chunk.length;
        if (total > 0) {
          onProgress?.call(received / total);
        } else {
          onProgress?.call(null);
        }
      }
    } finally {
      await sink.close();
    }
    return file;
  }

  /// Launches the downloaded Inno Setup installer with its wizard visible and,
  /// on a successful launch, quits the app. Returns `false` when the launch
  /// itself failed (see below) — the caller then falls back to revealing the
  /// installer so the user can run it by hand. On success this never returns:
  /// it hard-exits the process.
  ///
  /// The installer overwrites Wisp.exe and its bundled DLLs (the Flutter engine
  /// and the Rust native library) in `{app}`, which live under Program Files,
  /// so the setup requests administrator elevation. We previously ran it under
  /// `/SILENT` from a detached, hidden PowerShell stub — but a silent elevation
  /// prompt raised by an orphaned process (we had already exited) was easy to
  /// miss or decline, SmartScreen/AV silently blocked the hidden spawn, and any
  /// failure left the app simply gone with the old build still in place.
  ///
  /// Instead we launch the wizard *visibly* via `Start-Process` (ShellExecute,
  /// which auto-elevates the admin installer and shows a normal UAC prompt),
  /// and we wait just long enough to learn whether the launch succeeded — the
  /// PowerShell call returns as soon as the process is spawned (or throws if the
  /// user cancels elevation / the file is missing / a policy blocks it). Only on
  /// a confirmed launch do we destroy the window and hard-exit, releasing our
  /// file handles in `{app}` before the user clicks through to the copy step
  /// (seconds later — no race). Inno's `[Run]` entry then relaunches Wisp, and
  /// `/CLOSEAPPLICATIONS` stays as a Restart Manager safety net for any second
  /// instance.
  Future<bool> runWindowsInstaller(File installer) async {
    // Single-quoted PowerShell literals only need the quote itself doubled.
    final escapedPath = installer.path.replaceAll("'", "''");
    final command =
        "try { Start-Process -FilePath '$escapedPath' "
        "-ArgumentList '/CLOSEAPPLICATIONS' -ErrorAction Stop } "
        'catch { exit 1 }';

    ProcessResult result;
    try {
      // Blocks only until Start-Process has spawned the installer (UAC resolved)
      // — not for the whole install. A non-zero exit means the launch failed.
      result = await Process.run('powershell', [
        '-NoProfile',
        '-NonInteractive',
        '-Command',
        command,
      ]);
    } catch (error) {
      debugPrint('[update] could not launch installer: $error');
      return false;
    }
    if (result.exitCode != 0) {
      debugPrint('[update] installer launch exited ${result.exitCode}');
      return false;
    }

    // Launch confirmed. Close the window for a clean shutdown, then hard-exit so
    // the process — and every file handle it holds in {app} — is gone before the
    // user reaches the installer's copy step. Without the exit() backstop,
    // lingering native threads could keep us alive and Inno would hit a locked
    // file and roll the update back.
    await windowManager.destroy();
    exit(0);
  }

  /// Opens the file manager with [installer] preselected so the user can run it
  /// by hand after an automatic launch failed. Windows only; a no-op elsewhere.
  Future<void> revealInstaller(File installer) async {
    if (!Platform.isWindows) return;
    try {
      // `/select,<path>` must be a single token — Explorer won't accept the
      // path as a separate argument — so we can't split it into two entries.
      await Process.start('explorer', ['/select,${installer.path}']);
    } catch (error) {
      debugPrint('[update] could not reveal installer: $error');
    }
  }

  Future<void> openReleasesPage() async {
    try {
      await launchUrl(
        Uri.parse(releasesPageUrl),
        mode: LaunchMode.externalApplication,
      );
    } catch (error) {
      debugPrint('[update] could not open releases page: $error');
    }
  }

  /// Opens the Play Store listing for Wisp. Prefers the `market://` deep link so
  /// the Play Store app handles it directly; falls back to the https listing
  /// when the Play Store app isn't installed (e.g. emulators without Play
  /// services, or sideloaded builds). The `market` scheme is declared in
  /// AndroidManifest's `<queries>` so canLaunchUrl resolves on Android 11+.
  Future<void> openPlayStore() async {
    final marketUri = Uri.parse('market://details?id=$_androidPackageId');
    try {
      if (await canLaunchUrl(marketUri)) {
        await launchUrl(marketUri, mode: LaunchMode.externalApplication);
        return;
      }
      await launchUrl(
        Uri.parse(playStoreWebUrl),
        mode: LaunchMode.externalApplication,
      );
    } catch (error) {
      debugPrint('[update] could not open Play Store: $error');
    }
  }
}
