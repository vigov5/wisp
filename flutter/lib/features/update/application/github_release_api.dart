import 'dart:convert';

import 'package:http/http.dart' as http;

import '../domain/update_release.dart';

const String _latestReleaseUrl =
    'https://api.github.com/repos/vigov5/wisp/releases/latest';

/// Thin wrapper over the GitHub Releases REST API.
///
/// GitHub requires a `User-Agent` on every request and rate-limits anonymous
/// callers to 60 requests/hour per IP — plenty for an on-startup check.
class GithubReleaseApi {
  GithubReleaseApi({http.Client? client}) : _client = client ?? http.Client();

  final http.Client _client;

  Future<UpdateRelease> fetchLatest() async {
    final response = await _client.get(
      Uri.parse(_latestReleaseUrl),
      headers: const {
        'Accept': 'application/vnd.github+json',
        'User-Agent': 'Wisp',
        'X-GitHub-Api-Version': '2022-11-28',
      },
    );
    if (response.statusCode != 200) {
      throw http.ClientException(
        'GitHub releases returned HTTP ${response.statusCode}',
        Uri.parse(_latestReleaseUrl),
      );
    }
    final json = jsonDecode(response.body) as Map<String, dynamic>;
    return UpdateRelease.fromJson(json);
  }
}
