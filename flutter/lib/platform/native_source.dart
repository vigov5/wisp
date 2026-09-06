import 'package:flutter/foundation.dart';

/// One file the native side has made ready to send.
///
/// On Android a picked or shared file usually reaches us as a live descriptor
/// rather than a cache copy — [path] is then `/proc/self/fd/<n>`, which the
/// core can open but which carries no usable file name, so [name] has to
/// travel with it.  Everywhere else (iOS, desktop, and the Android fallback
/// copy) [path] is an ordinary file that names itself.
@immutable
class NativeSource {
  const NativeSource({
    required this.path,
    required this.name,
    this.sizeBytes,
    this.fromDescriptor = false,
  });

  /// A plain filesystem path, naming itself.
  factory NativeSource.fromPath(String path) {
    return NativeSource(path: path, name: _basename(path));
  }

  /// Parses one entry as the platform channels deliver it: a bare path string
  /// (iOS share sheet, desktop) or the richer map the Android picker and share
  /// handler send.  Returns null for anything unusable.
  static NativeSource? parse(Object? raw) {
    if (raw is String) {
      final path = raw.trim();
      return path.isEmpty ? null : NativeSource.fromPath(path);
    }
    if (raw is! Map) return null;
    final path = (raw['path'] as String?)?.trim();
    if (path == null || path.isEmpty) return null;
    final name = (raw['name'] as String?)?.trim();
    final size = raw['size'];
    return NativeSource(
      path: path,
      name: name == null || name.isEmpty ? _basename(path) : name,
      sizeBytes: size is int ? BigInt.from(size) : null,
      fromDescriptor: raw['copied'] == false,
    );
  }

  static List<NativeSource> parseAll(List<dynamic>? raw) {
    if (raw == null) return const [];
    return raw
        .map(NativeSource.parse)
        .nonNulls
        .toList(growable: false);
  }

  final String path;
  final String name;
  final BigInt? sizeBytes;

  /// True when [path] is a descriptor path rather than a durable file: the
  /// bytes were never copied anywhere, and [name] — the file's path within the
  /// picked folder, or its bare name for a picked file — is the only place
  /// that survives.
  final bool fromDescriptor;

  static String _basename(String path) {
    final normalized = path.replaceAll(r'\', '/');
    final segment = normalized.split('/').where((s) => s.isNotEmpty).lastOrNull;
    return segment == null || segment.isEmpty ? path : segment;
  }
}

/// One source handed to the core for a send.
@immutable
class SendSource {
  const SendSource({required this.path, this.fdTransferPath});

  /// Built from a platform-provided [NativeSource].
  factory SendSource.fromNative(NativeSource source) {
    return SendSource(
      path: source.path,
      fdTransferPath: source.fromDescriptor ? source.name : null,
    );
  }

  /// The path the core opens.  On Android this is usually
  /// `/proc/self/fd/<n>`: the picked file read straight from its SAF
  /// descriptor, needing no free space at all.  A cache copy only where the
  /// platform leaves no alternative — a provider that answers with a pipe
  /// rather than a file, which cannot be read at an offset.
  final String path;

  /// Where this lands on the receiver, for a descriptor [path] — which ends in
  /// the fd number and so cannot name itself.  A bare file name for a picked
  /// file; a relative path (`photos/trip/cat.jpg`) for one file of a picked
  /// folder, which travels as one descriptor per file.  Null for an ordinary
  /// file, which names itself.
  final String? fdTransferPath;

  @override
  bool operator ==(Object other) =>
      other is SendSource &&
      other.path == path &&
      other.fdTransferPath == fdTransferPath;

  @override
  int get hashCode => Object.hash(path, fdTransferPath);
}

/// Why the platform could not hand a picked or shared file over to a send.
enum SourceRejectionReason {
  /// Preparing the file would have needed more free space than the device can
  /// spare.  Android only lets an app read some providers' files by copying
  /// them into its own cache first, and a copy that fills the disk takes the
  /// whole device down, so one that cannot fit is refused before it starts.
  noSpace,

  /// The provider would not give up the bytes at all.
  unreadable,
}

/// One file the platform refused to prepare, and why — so an empty (or short)
/// draft can say what happened instead of silently dropping the file.
@immutable
class SourceRejection {
  const SourceRejection({
    required this.name,
    required this.reason,
    this.requiredBytes,
    this.availableBytes,
  });

  static SourceRejection? parse(Object? raw) {
    if (raw is! Map) return null;
    final name = (raw['name'] as String?)?.trim();
    if (name == null || name.isEmpty) return null;
    final required = raw['requiredBytes'];
    final available = raw['availableBytes'];
    return SourceRejection(
      name: name,
      reason: raw['reason'] == 'no_space'
          ? SourceRejectionReason.noSpace
          : SourceRejectionReason.unreadable,
      requiredBytes: required is int ? BigInt.from(required) : null,
      availableBytes: available is int ? BigInt.from(available) : null,
    );
  }

  static List<SourceRejection> parseAll(Object? raw) {
    if (raw is! List) return const [];
    return raw.map(SourceRejection.parse).nonNulls.toList(growable: false);
  }

  final String name;
  final SourceRejectionReason reason;

  /// Bytes the copy would have needed, when the provider reported a size.
  final BigInt? requiredBytes;

  /// Bytes that were free for it, over and above the headroom the device
  /// keeps for itself.
  final BigInt? availableBytes;
}
