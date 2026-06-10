import 'dart:io';

import 'package:app/features/update/domain/update_release.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:pub_semver/pub_semver.dart';

// A trimmed-down shape of the GitHub `releases/latest` payload.
Map<String, dynamic> _sampleJson() => {
  'tag_name': 'v1.6.0',
  'html_url': 'https://github.com/vigov5/wisp/releases/tag/v1.6.0',
  'body': 'New stuff and bug fixes.',
  'assets': [
    {
      'name': 'wisp-windows-setup-v1.6.0.exe',
      'browser_download_url':
          'https://example.com/wisp-windows-setup-v1.6.0.exe',
      'size': 12345,
    },
    {
      'name': 'wisp-macos-v1.6.0.dmg',
      'browser_download_url': 'https://example.com/wisp-macos-v1.6.0.dmg',
      'size': 6789,
    },
    {
      'name': 'wisp-linux-v1.6.0.deb',
      'browser_download_url': 'https://example.com/wisp-linux-v1.6.0.deb',
      'size': 4321,
    },
  ],
};

void main() {
  group('UpdateRelease.fromJson', () {
    test('parses tag, version, notes and assets', () {
      final release = UpdateRelease.fromJson(_sampleJson());

      expect(release.tagName, 'v1.6.0');
      expect(release.version, Version.parse('1.6.0'));
      expect(release.releaseNotes, 'New stuff and bug fixes.');
      expect(release.assets, hasLength(3));
      expect(release.assets.first.name, 'wisp-windows-setup-v1.6.0.exe');
      expect(release.assets.first.sizeBytes, 12345);
    });

    test('a malformed tag falls back to 0.0.0 (never newer)', () {
      final release = UpdateRelease.fromJson({
        ...(_sampleJson()),
        'tag_name': 'nightly',
      });
      expect(release.version, Version.none);
      expect(release.version < Version.parse('1.0.0'), isTrue);
    });

    test('tolerates a missing assets array', () {
      final json = _sampleJson()..remove('assets');
      final release = UpdateRelease.fromJson(json);
      expect(release.assets, isEmpty);
    });
  });

  group('version comparison', () {
    test('semver ordering drives the "newer" decision', () {
      expect(Version.parse('1.6.0') > Version.parse('1.5.0'), isTrue);
      expect(Version.parse('1.5.0') > Version.parse('1.5.0'), isFalse);
      expect(Version.parse('1.5.1') > Version.parse('1.5.0'), isTrue);
      expect(Version.parse('1.10.0') > Version.parse('1.9.0'), isTrue);
    });
  });

  group('assetForCurrentPlatform', () {
    test('picks the Windows setup .exe on Windows, null elsewhere', () {
      final release = UpdateRelease.fromJson(_sampleJson());
      final asset = release.assetForCurrentPlatform();
      if (Platform.isWindows) {
        expect(asset, isNotNull);
        expect(asset!.name, 'wisp-windows-setup-v1.6.0.exe');
      } else {
        expect(asset, isNull);
      }
    });
  });
}
