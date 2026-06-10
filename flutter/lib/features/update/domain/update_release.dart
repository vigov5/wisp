import 'dart:io';

import 'package:pub_semver/pub_semver.dart';

/// A downloadable file attached to a GitHub release.
class ReleaseAsset {
  const ReleaseAsset({
    required this.name,
    required this.downloadUrl,
    required this.sizeBytes,
  });

  final String name;
  final String downloadUrl;
  final int sizeBytes;

  factory ReleaseAsset.fromJson(Map<String, dynamic> json) {
    return ReleaseAsset(
      name: json['name'] as String? ?? '',
      downloadUrl: json['browser_download_url'] as String? ?? '',
      sizeBytes: (json['size'] as num?)?.toInt() ?? 0,
    );
  }
}

/// A parsed GitHub release. [version] is derived from the `v`-prefixed
/// `tag_name` (e.g. tag `v1.6.0` → version `1.6.0`).
class UpdateRelease {
  const UpdateRelease({
    required this.version,
    required this.tagName,
    required this.htmlUrl,
    required this.releaseNotes,
    required this.assets,
  });

  final Version version;
  final String tagName;
  final String htmlUrl;
  final String releaseNotes;
  final List<ReleaseAsset> assets;

  factory UpdateRelease.fromJson(Map<String, dynamic> json) {
    final tag = json['tag_name'] as String? ?? '';
    final assetsJson = (json['assets'] as List<dynamic>? ?? const [])
        .whereType<Map<String, dynamic>>()
        .map(ReleaseAsset.fromJson)
        .toList(growable: false);
    return UpdateRelease(
      version: _parseTag(tag),
      tagName: tag,
      htmlUrl: json['html_url'] as String? ?? '',
      releaseNotes: json['body'] as String? ?? '',
      assets: assetsJson,
    );
  }

  /// The installer asset to download for the current desktop platform, or
  /// `null` when no in-app install path exists (macOS/Linux fall back to
  /// opening the Releases page in a browser).
  ReleaseAsset? assetForCurrentPlatform() {
    if (Platform.isWindows) {
      for (final asset in assets) {
        if (asset.name.startsWith('wisp-windows-setup-') &&
            asset.name.endsWith('.exe')) {
          return asset;
        }
      }
    }
    return null;
  }
}

/// Parses a release tag like `v1.6.0` (or `1.6.0`) into a [Version]. Returns
/// `Version.none` (0.0.0) when the tag is malformed so callers treat it as not
/// newer than the running build.
Version _parseTag(String tag) {
  final cleaned = tag.startsWith('v') ? tag.substring(1) : tag;
  try {
    return Version.parse(cleaned);
  } on FormatException {
    return Version.none;
  }
}
