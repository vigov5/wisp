import 'package:shared_preferences/shared_preferences.dart';

const String _checkOnStartupKey = 'update.check_on_startup';
const String _skippedVersionKey = 'update.skipped_version';

/// Persists update-checker preferences. Mirrors the storage pattern used by
/// [SettingsRepository] — a thin wrapper over [SharedPreferences].
class UpdateRepository {
  UpdateRepository({required this.prefs});

  final SharedPreferences prefs;

  /// Whether the automatic on-launch check runs. Defaults on; the only gate on
  /// the startup check (there is no time throttle — anonymous GitHub allows 60
  /// requests/hour, far more than one cold start ever needs).
  bool checkOnStartup() => prefs.getBool(_checkOnStartupKey) ?? true;

  Future<void> setCheckOnStartup(bool value) =>
      prefs.setBool(_checkOnStartupKey, value);

  String? skippedVersion() => prefs.getString(_skippedVersionKey);

  Future<void> setSkippedVersion(String tag) =>
      prefs.setString(_skippedVersionKey, tag);
}
