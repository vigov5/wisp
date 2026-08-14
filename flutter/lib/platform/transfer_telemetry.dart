import 'dart:convert';

import 'package:flutter/foundation.dart';

const bool transferTelemetryEnabled = bool.fromEnvironment(
  'WISP_TRANSFER_TELEMETRY',
);

const String _telemetryTarget = 'wisp_transfer_telemetry';
final BigInt _maxU64 = BigInt.parse('18446744073709551615');

enum MobileTransferTelemetryRole { sender, receiver }

enum MobileTransferTelemetryPhase { safReadCopy, backgroundSave }

enum MobileTransferTelemetryOutcome { complete, failed, skipped }

/// Emits a schema-compatible JSONL phase event when mobile benchmark
/// telemetry was explicitly enabled at build time. No path, URI, peer ID, or
/// raw session ID leaves the process.
bool emitMobileTransferPhase({
  required MobileTransferTelemetryRole role,
  required MobileTransferTelemetryPhase phase,
  required MobileTransferTelemetryOutcome outcome,
  required BigInt? benchmarkRunId,
  required Duration elapsed,
  required BigInt bytesTotal,
  required int fileCount,
}) {
  if (!transferTelemetryEnabled) return false;
  final line = encodeMobileTransferPhase(
    role: role,
    phase: phase,
    outcome: outcome,
    benchmarkRunId: benchmarkRunId,
    elapsed: elapsed,
    bytesTotal: bytesTotal,
    fileCount: fileCount,
  );
  if (line == null) return false;
  debugPrint(line);
  return true;
}

@visibleForTesting
String? encodeMobileTransferPhase({
  required MobileTransferTelemetryRole role,
  required MobileTransferTelemetryPhase phase,
  required MobileTransferTelemetryOutcome outcome,
  required BigInt? benchmarkRunId,
  required Duration elapsed,
  required BigInt bytesTotal,
  required int fileCount,
}) {
  if (benchmarkRunId == null ||
      benchmarkRunId.isNegative ||
      benchmarkRunId > _maxU64 ||
      elapsed.isNegative ||
      bytesTotal.isNegative ||
      bytesTotal > _maxU64 ||
      fileCount < 0) {
    return null;
  }
  // Dart's native `int` is signed, while the Rust run ID and byte counters are
  // u64. Canonical decimal strings preserve all 64 bits; the bounded analyzer
  // accepts this representation alongside ordinary JSON integers.
  return jsonEncode({
    'target': _telemetryTarget,
    'fields': {
      'event': 'blob_phase',
      'role': switch (role) {
        MobileTransferTelemetryRole.sender => 'sender',
        MobileTransferTelemetryRole.receiver => 'receiver',
      },
      'benchmark_run_id_available': true,
      'benchmark_run_id': benchmarkRunId.toString(),
      'phase': switch (phase) {
        MobileTransferTelemetryPhase.safReadCopy => 'saf_read_copy',
        MobileTransferTelemetryPhase.backgroundSave => 'background_save',
      },
      'outcome': switch (outcome) {
        MobileTransferTelemetryOutcome.complete => 'complete',
        MobileTransferTelemetryOutcome.failed => 'failed',
        MobileTransferTelemetryOutcome.skipped => 'skipped',
      },
      'elapsed_ms': elapsed.inMilliseconds,
      'bytes_total': bytesTotal.toString(),
      'file_count': fileCount,
    },
  });
}
