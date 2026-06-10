import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../domain/update_status.dart';
import 'github_release_api.dart';
import 'update_controller.dart';
import 'update_installer.dart';
import 'update_repository.dart';

/// Overridden at bootstrap with the [SharedPreferences]-backed instance, the
/// same way [settingsRepositoryProvider] is wired in main.dart.
final updateRepositoryProvider = Provider<UpdateRepository>((ref) {
  throw UnimplementedError(
    'updateRepositoryProvider must be overridden at bootstrap',
  );
});

final githubReleaseApiProvider = Provider<GithubReleaseApi>(
  (ref) => GithubReleaseApi(),
);

final updateInstallerProvider = Provider<UpdateInstaller>(
  (ref) => UpdateInstaller(),
);

final updateControllerProvider =
    NotifierProvider<UpdateController, UpdateState>(UpdateController.new);
