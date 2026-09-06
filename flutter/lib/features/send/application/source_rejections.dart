import '../../../platform/native_source.dart';
import '../../transfers/application/format_utils.dart';

/// One sentence for the files a pick or share could not prepare.
///
/// Android hands some providers' files over as a live descriptor and others
/// only as bytes to copy into the app cache first, and the copy is refused
/// when it would fill the disk — so the reason matters as much as the name.
/// Reads as "Couldn't add demo.rar — needs 6.2 GB free, 5.5 GB available."
String describeSourceRejections(List<SourceRejection> rejected) {
  if (rejected.isEmpty) return '';

  final subject = rejected.length == 1
      ? rejected.first.name
      : '${rejected.length} files';

  // One reason, or a mix.  A mix is rare enough to describe loosely rather
  // than spell out per file in a SnackBar.
  final reasons = rejected.map((r) => r.reason).toSet();
  if (reasons.length > 1) {
    return "Couldn't add $subject.";
  }

  if (reasons.first == SourceRejectionReason.unreadable) {
    return "Couldn't read $subject.";
  }

  final needed = rejected
      .map((r) => r.requiredBytes ?? BigInt.zero)
      .fold(BigInt.zero, (a, b) => a + b);
  // The smallest headroom seen: with several files it shrinks as each copy
  // lands, and the tightest number is the one that explains the refusal.
  final freeValues = rejected.map((r) => r.availableBytes).nonNulls;
  final free = freeValues.isEmpty
      ? null
      : freeValues.reduce((a, b) => a < b ? a : b);

  final buffer = StringBuffer("Not enough space to add $subject");
  if (needed > BigInt.zero) {
    buffer.write(' — needs ${formatBytes(needed)} free');
    if (free != null) {
      buffer.write(', ${formatBytes(free)} available');
    }
  }
  buffer.write('.');
  return buffer.toString();
}
