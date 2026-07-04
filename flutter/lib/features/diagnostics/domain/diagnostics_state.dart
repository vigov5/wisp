import 'check_result.dart';

class DiagnosticsState {
  final List<CheckResult> results;
  final bool isRunning;
  final DateTime? lastRunAt;

  const DiagnosticsState({
    this.results = const [],
    this.isRunning = false,
    this.lastRunAt,
  });

  int get passCount =>
      results.where((r) => r.status == CheckStatus.pass).length;
  int get warnCount =>
      results.where((r) => r.status == CheckStatus.warn).length;
  int get failCount =>
      results.where((r) => r.status == CheckStatus.fail).length;
  int get skippedCount =>
      results.where((r) => r.status == CheckStatus.skipped).length;

  CheckStatus get overallStatus {
    if (results.isEmpty || isRunning) return CheckStatus.running;
    if (failCount > 0) return CheckStatus.fail;
    if (warnCount > 0) return CheckStatus.warn;
    return CheckStatus.pass;
  }

  List<CheckResult> resultsFor(CheckGroup group) =>
      results.where((r) => r.group == group).toList(growable: false);

  CheckStatus statusFor(CheckGroup group) {
    final subset = resultsFor(group);
    if (subset.isEmpty) return CheckStatus.running;
    if (subset.any((r) => r.status == CheckStatus.fail)) {
      return CheckStatus.fail;
    }
    if (subset.any((r) => r.status == CheckStatus.warn)) {
      return CheckStatus.warn;
    }
    if (subset.any((r) => r.status == CheckStatus.running)) {
      return CheckStatus.running;
    }
    if (subset.every((r) => r.status == CheckStatus.skipped)) {
      return CheckStatus.skipped;
    }
    return CheckStatus.pass;
  }

  DiagnosticsState copyWith({
    List<CheckResult>? results,
    bool? isRunning,
    DateTime? lastRunAt,
  }) {
    return DiagnosticsState(
      results: results ?? this.results,
      isRunning: isRunning ?? this.isRunning,
      lastRunAt: lastRunAt ?? this.lastRunAt,
    );
  }

  DiagnosticsState upsert(CheckResult result) {
    final next = List<CheckResult>.from(results);
    final idx = next.indexWhere((r) => r.id == result.id);
    if (idx >= 0) {
      next[idx] = result;
    } else {
      next.add(result);
    }
    return copyWith(results: next);
  }
}
