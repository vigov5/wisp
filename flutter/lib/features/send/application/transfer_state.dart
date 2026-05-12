import 'package:flutter/foundation.dart';

import '../../../platform/send_transfer_source.dart';
import '../../../src/rust/api/transfer.dart' as rust_transfer;
import '../../transfers/application/connection_path.dart';

enum SendTransferPhase {
  connecting,
  waitingForDecision,
  accepted,
  sending,
  cancelling,
  completed,
  declined,
  cancelled,
  failed,
}

@immutable
class SendTransferState {
  const SendTransferState({
    required this.phase,
    required this.destinationLabel,
    required this.statusMessage,
    required this.itemCount,
    required this.totalSize,
    required this.bytesSent,
    required this.totalBytes,
    this.plan,
    this.snapshot,
    this.remoteDeviceType,
    this.remoteEndpointId,
    this.remoteTicket,
    this.connectionPath,
    this.error,
  });

  SendTransferState.connecting({
    required String destinationLabel,
    required BigInt itemCount,
    required BigInt totalSize,
  }) : this(
         phase: SendTransferPhase.connecting,
         destinationLabel: destinationLabel,
         statusMessage: 'Request sent',
         itemCount: itemCount,
         totalSize: totalSize,
         bytesSent: BigInt.zero,
         totalBytes: totalSize,
       );

  final SendTransferPhase phase;
  final String destinationLabel;
  final String statusMessage;
  final BigInt itemCount;
  final BigInt totalSize;
  final BigInt bytesSent;
  final BigInt totalBytes;
  final rust_transfer.TransferPlanData? plan;
  final rust_transfer.TransferSnapshotData? snapshot;
  final String? remoteDeviceType;
  final String? remoteEndpointId;
  /// Re-serialized ticket of the resolved peer.  Populated for both
  /// code-based and nearby sends once the destination resolves, so
  /// `_completeTransfer` can persist it to saved devices as `lastTicket`.
  final String? remoteTicket;
  final ConnectionPathInfo? connectionPath;
  final SendTransferErrorData? error;

  bool get isTerminal =>
      phase == SendTransferPhase.completed ||
      phase == SendTransferPhase.declined ||
      phase == SendTransferPhase.cancelled ||
      phase == SendTransferPhase.failed;

  SendTransferState copyWith({
    SendTransferPhase? phase,
    String? destinationLabel,
    String? statusMessage,
    BigInt? itemCount,
    BigInt? totalSize,
    BigInt? bytesSent,
    BigInt? totalBytes,
    rust_transfer.TransferPlanData? plan,
    rust_transfer.TransferSnapshotData? snapshot,
    String? remoteDeviceType,
    String? remoteEndpointId,
    String? remoteTicket,
    ConnectionPathInfo? connectionPath,
    SendTransferErrorData? error,
  }) {
    return SendTransferState(
      phase: phase ?? this.phase,
      destinationLabel: destinationLabel ?? this.destinationLabel,
      statusMessage: statusMessage ?? this.statusMessage,
      itemCount: itemCount ?? this.itemCount,
      totalSize: totalSize ?? this.totalSize,
      bytesSent: bytesSent ?? this.bytesSent,
      totalBytes: totalBytes ?? this.totalBytes,
      plan: plan ?? this.plan,
      snapshot: snapshot ?? this.snapshot,
      remoteDeviceType: remoteDeviceType ?? this.remoteDeviceType,
      remoteEndpointId: remoteEndpointId ?? this.remoteEndpointId,
      remoteTicket: remoteTicket ?? this.remoteTicket,
      connectionPath: connectionPath ?? this.connectionPath,
      error: error ?? this.error,
    );
  }
}
