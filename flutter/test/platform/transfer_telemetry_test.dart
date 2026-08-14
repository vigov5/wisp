import 'dart:convert';

import 'package:app/platform/transfer_telemetry.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  test('runtime emission stays disabled without the benchmark build flag', () {
    expect(transferTelemetryEnabled, isFalse);
    expect(
      emitMobileTransferPhase(
        role: MobileTransferTelemetryRole.sender,
        phase: MobileTransferTelemetryPhase.safReadCopy,
        outcome: MobileTransferTelemetryOutcome.complete,
        sessionId: '0123456789abcdef',
        elapsed: Duration.zero,
        bytesTotal: BigInt.zero,
        fileCount: 0,
      ),
      isFalse,
    );
  });

  test('encodes anonymous mobile phase with full u64 counters', () {
    final line = encodeMobileTransferPhase(
      role: MobileTransferTelemetryRole.receiver,
      phase: MobileTransferTelemetryPhase.backgroundSave,
      outcome: MobileTransferTelemetryOutcome.complete,
      sessionId: 'ffffffffffffffff',
      elapsed: const Duration(milliseconds: 1250),
      bytesTotal: BigInt.parse('18446744073709551615'),
      fileCount: 3,
    );

    expect(line, isNotNull);
    final payload = jsonDecode(line!) as Map<String, dynamic>;
    final fields = payload['fields'] as Map<String, dynamic>;
    expect(payload['target'], 'wisp_transfer_telemetry');
    expect(fields['event'], 'blob_phase');
    expect(fields['role'], 'receiver');
    expect(fields['benchmark_run_id'], '18446744073709551615');
    expect(fields['phase'], 'background_save');
    expect(fields['elapsed_ms'], 1250);
    expect(fields['bytes_total'], '18446744073709551615');
    expect(fields['file_count'], 3);
  });

  test('rejects session IDs outside the anonymous benchmark format', () {
    String? encode(String sessionId) => encodeMobileTransferPhase(
      role: MobileTransferTelemetryRole.sender,
      phase: MobileTransferTelemetryPhase.safReadCopy,
      outcome: MobileTransferTelemetryOutcome.complete,
      sessionId: sessionId,
      elapsed: Duration.zero,
      bytesTotal: BigInt.zero,
      fileCount: 0,
    );

    expect(encode('ABCDEF0123456789'), isNull);
    expect(encode('../private-path'), isNull);
    expect(encode('0123456789abcdef0'), isNull);
  });

  test('rejects counters outside the u64 telemetry schema', () {
    final line = encodeMobileTransferPhase(
      role: MobileTransferTelemetryRole.sender,
      phase: MobileTransferTelemetryPhase.safReadCopy,
      outcome: MobileTransferTelemetryOutcome.complete,
      sessionId: '0123456789abcdef',
      elapsed: Duration.zero,
      bytesTotal: BigInt.parse('18446744073709551616'),
      fileCount: 1,
    );

    expect(line, isNull);
  });
}
