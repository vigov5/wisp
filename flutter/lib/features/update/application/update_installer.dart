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

  /// Launches the downloaded Inno Setup installer silently and quits the app.
  ///
  /// The installer overwrites Wisp.exe and its bundled DLLs (the Flutter engine
  /// and the Rust native library) in `{app}`. Those files stay locked for as
  /// long as *our own process* is alive, so the install must not begin until we
  /// have fully exited. Previously we started the installer, waited 300 ms, then
  /// called [WindowManager.destroy] — letting Inno's `/CLOSEAPPLICATIONS`
  /// Restart Manager race our own teardown. By the time Inno reached the copy
  /// step our DLLs were often still mapped, so it hit a locked file and rolled
  /// the whole update back.
  ///
  /// Instead we hand the install off to a tiny detached PowerShell stub that
  /// blocks on our PID and only spawns the installer once we are gone, then we
  /// hard-exit. With the directory unlocked Inno copies cleanly and its `[Run]`
  /// entry relaunches Wisp (`skipifsilent` was removed from inno_setup.iss so
  /// the postinstall relaunch fires under `/SILENT`). `/CLOSEAPPLICATIONS`
  /// remains as a safety net for any second Wisp instance.
  Future<void> runWindowsInstaller(File installer) async {
    // PowerShell single-quoted literals only need the quote itself doubled.
    final escapedPath = installer.path.replaceAll("'", "''");
    final command =
        'Wait-Process -Id $pid -ErrorAction SilentlyContinue; '
        "Start-Process -FilePath '$escapedPath' "
        "-ArgumentList '/SILENT','/SUPPRESSMSGBOXES','/CLOSEAPPLICATIONS'";

    await Process.start('powershell', [
      '-NoProfile',
      '-NonInteractive',
      '-WindowStyle',
      'Hidden',
      '-Command',
      command,
    ], mode: ProcessStartMode.detached);

    // Close the window for a clean shutdown, then hard-exit so the process — and
    // every file handle it holds in {app} — is guaranteed gone before the stub
    // unblocks and runs the installer. Without the exit() backstop, lingering
    // native threads could keep us alive and the install would never start.
    await windowManager.destroy();
    exit(0);
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
