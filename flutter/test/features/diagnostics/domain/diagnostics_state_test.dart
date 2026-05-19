import 'package:app/features/diagnostics/domain/check_result.dart';
import 'package:app/features/diagnostics/domain/diagnostics_state.dart';
import 'package:flutter_test/flutter_test.dart';

CheckResult _result(
  String id,
  CheckGroup group,
  CheckStatus status, {
  String label = 'label',
  String detail = '',
}) {
  return CheckResult(
    id: id,
    group: group,
    status: status,
    label: label,
    detail: detail,
  );
}

void main() {
  group('DiagnosticsState.upsert', () {
    test('appends a new result when id is unseen', () {
      const state = DiagnosticsState();
      final next = state.upsert(
        _result('network.internet', CheckGroup.network, CheckStatus.pass),
      );
      expect(next.results, hasLength(1));
      expect(next.results.first.id, 'network.internet');
    });

    test('replaces an existing result with the same id', () {
      final state = const DiagnosticsState().upsert(
        _result('network.internet', CheckGroup.network, CheckStatus.running),
      );
      final next = state.upsert(
        _result(
          'network.internet',
          CheckGroup.network,
          CheckStatus.pass,
          detail: '32 ms',
        ),
      );
      expect(next.results, hasLength(1));
      expect(next.results.first.status, CheckStatus.pass);
      expect(next.results.first.detail, '32 ms');
    });

    test('preserves order across upserts', () {
      var state = const DiagnosticsState();
      state = state.upsert(
        _result('a', CheckGroup.network, CheckStatus.running),
      );
      state = state.upsert(
        _result('b', CheckGroup.lan, CheckStatus.running),
      );
      state = state.upsert(
        _result('a', CheckGroup.network, CheckStatus.pass),
      );
      expect(state.results.map((r) => r.id), ['a', 'b']);
    });
  });

  group('DiagnosticsState counters', () {
    test('counts pass/warn/fail/skipped independently', () {
      var state = const DiagnosticsState();
      state = state.upsert(_result('a', CheckGroup.network, CheckStatus.pass));
      state = state.upsert(_result('b', CheckGroup.network, CheckStatus.warn));
      state = state.upsert(_result('c', CheckGroup.lan, CheckStatus.fail));
      state = state.upsert(
        _result('d', CheckGroup.local, CheckStatus.skipped),
      );
      expect(state.passCount, 1);
      expect(state.warnCount, 1);
      expect(state.failCount, 1);
      expect(state.skippedCount, 1);
    });
  });

  group('DiagnosticsState.statusFor', () {
    test('returns running when group has no results yet', () {
      const state = DiagnosticsState();
      expect(state.statusFor(CheckGroup.network), CheckStatus.running);
    });

    test('returns fail when any check in the group failed', () {
      var state = const DiagnosticsState();
      state = state.upsert(_result('a', CheckGroup.local, CheckStatus.pass));
      state = state.upsert(_result('b', CheckGroup.local, CheckStatus.fail));
      state = state.upsert(_result('c', CheckGroup.local, CheckStatus.warn));
      expect(state.statusFor(CheckGroup.local), CheckStatus.fail);
    });

    test('returns warn when any check warned but none failed', () {
      var state = const DiagnosticsState();
      state = state.upsert(_result('a', CheckGroup.local, CheckStatus.pass));
      state = state.upsert(_result('b', CheckGroup.local, CheckStatus.warn));
      expect(state.statusFor(CheckGroup.local), CheckStatus.warn);
    });

    test('returns running while any check in the group is still running', () {
      var state = const DiagnosticsState();
      state = state.upsert(_result('a', CheckGroup.local, CheckStatus.pass));
      state = state.upsert(
        _result('b', CheckGroup.local, CheckStatus.running),
      );
      expect(state.statusFor(CheckGroup.local), CheckStatus.running);
    });

    test('returns skipped when every check in the group is skipped', () {
      var state = const DiagnosticsState();
      state = state.upsert(
        _result('a', CheckGroup.local, CheckStatus.skipped),
      );
      state = state.upsert(
        _result('b', CheckGroup.local, CheckStatus.skipped),
      );
      expect(state.statusFor(CheckGroup.local), CheckStatus.skipped);
    });

    test('returns pass only when every check in the group passed', () {
      var state = const DiagnosticsState();
      state = state.upsert(_result('a', CheckGroup.local, CheckStatus.pass));
      state = state.upsert(_result('b', CheckGroup.local, CheckStatus.pass));
      expect(state.statusFor(CheckGroup.local), CheckStatus.pass);
    });
  });

  group('DiagnosticsState.overallStatus', () {
    test('is running while isRunning is true even with results', () {
      var state = const DiagnosticsState(isRunning: true);
      state = state.upsert(_result('a', CheckGroup.network, CheckStatus.pass));
      expect(state.overallStatus, CheckStatus.running);
    });

    test('is running when there are no results yet', () {
      const state = DiagnosticsState();
      expect(state.overallStatus, CheckStatus.running);
    });

    test('returns fail if any check failed', () {
      var state = const DiagnosticsState();
      state = state.upsert(_result('a', CheckGroup.network, CheckStatus.pass));
      state = state.upsert(_result('b', CheckGroup.lan, CheckStatus.fail));
      expect(state.overallStatus, CheckStatus.fail);
    });

    test('returns warn when warnings exist but no failures', () {
      var state = const DiagnosticsState();
      state = state.upsert(_result('a', CheckGroup.network, CheckStatus.pass));
      state = state.upsert(_result('b', CheckGroup.lan, CheckStatus.warn));
      expect(state.overallStatus, CheckStatus.warn);
    });

    test('returns pass only when everything passed', () {
      var state = const DiagnosticsState();
      state = state.upsert(_result('a', CheckGroup.network, CheckStatus.pass));
      state = state.upsert(_result('b', CheckGroup.lan, CheckStatus.pass));
      expect(state.overallStatus, CheckStatus.pass);
    });
  });

  group('DiagnosticsState.resultsFor', () {
    test('returns only the results in the requested group', () {
      var state = const DiagnosticsState();
      state = state.upsert(_result('a', CheckGroup.network, CheckStatus.pass));
      state = state.upsert(_result('b', CheckGroup.lan, CheckStatus.pass));
      state = state.upsert(
        _result('c', CheckGroup.network, CheckStatus.warn),
      );
      final network = state.resultsFor(CheckGroup.network);
      expect(network.map((r) => r.id), ['a', 'c']);
      expect(state.resultsFor(CheckGroup.lan).map((r) => r.id), ['b']);
    });

    test('returns an empty list for groups with no results', () {
      const state = DiagnosticsState();
      expect(state.resultsFor(CheckGroup.permissions), isEmpty);
    });
  });
}
