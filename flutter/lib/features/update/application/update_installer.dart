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

  /// Launches the downloaded Inno Setup installer silently and quits the app so
  /// it can overwrite the running executable. `/CLOSEAPPLICATIONS` lets the
  /// installer shut any lingering Wisp window via the Restart Manager; the
  /// installer's `[Run]` entry relaunches Wisp once the install completes (the
  /// `skipifsilent` flag was removed from inno_setup.iss for this).
  Future<void> runWindowsInstaller(File installer) async {
    await Process.start(installer.path, const [
      '/SILENT',
      '/SUPPRESSMSGBOXES',
      '/CLOSEAPPLICATIONS',
    ], mode: ProcessStartMode.detached);
    // Give the detached process a moment to spawn before we exit.
    await Future<void>.delayed(const Duration(milliseconds: 300));
    await windowManager.destroy();
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
