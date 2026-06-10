import 'package:shared_preferences/shared_preferences.dart';

const String _checkOnStartupKey = 'update.check_on_startup';
const String _lastCheckAtKey = 'update.last_check_at';
const String _skippedVersionKey = 'update.skipped_version';

/// How long to wait between automatic on-startup checks. Manual checks ignore
/// this throttle.
const Duration _autoCheckInterval = Duration(hours: 24);

/// Persists update-checker preferences. Mirrors the storage pattern used by
/// [SettingsRepository] — a thin wrapper over [SharedPreferences].
class UpdateRepository {
  UpdateRepository({required this.prefs});

  final SharedPreferences prefs;

  bool checkOnStartup() => prefs.getBool(_checkOnStartupKey) ?? true;

  Future<void> setCheckOnStartup(bool value) =>
      prefs.setBool(_checkOnStartupKey, value);

  DateTime? lastCheckAt() {
    final millis = prefs.getInt(_lastCheckAtKey);
    if (millis == null) return null;
    return DateTime.fromMillisecondsSinceEpoch(millis);
  }

  Future<void> markChecked(DateTime now) =>
      prefs.setInt(_lastCheckAtKey, now.millisecondsSinceEpoch);

  String? skippedVersion() => prefs.getString(_skippedVersionKey);

  Future<void> setSkippedVersion(String tag) =>
      prefs.setString(_skippedVersionKey, tag);

  /// Whether an automatic check should run now: the toggle is on and the last
  /// check is older than [_autoCheckInterval] (or never ran).
  bool shouldAutoCheck(DateTime now) {
    if (!checkOnStartup()) return false;
    final last = lastCheckAt();
    if (last == null) return true;
    return now.difference(last) >= _autoCheckInterval;
  }
}
