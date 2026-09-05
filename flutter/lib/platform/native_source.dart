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
  /// `/proc/self/fd/<n>`: the picked file is read straight from its SAF
  /// descriptor rather than copied into the app cache first, so a multi-GB
  /// send needs no free space at all.
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
