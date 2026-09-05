import 'package:flutter_test/flutter_test.dart';

import 'package:app/features/send/application/model.dart';
import 'package:app/features/send/application/send_selection_picker.dart';
import 'package:app/platform/native_source.dart';

void main() {
  test('a descriptor source keeps its name as the fd display name', () {
    final picked = sendPickedFileFromNativeSource(
      NativeSource(
        path: '/proc/self/fd/42',
        name: 'holiday.mp4',
        sizeBytes: BigInt.from(6000000000),
        fromDescriptor: true,
      ),
    );

    expect(picked.path, '/proc/self/fd/42');
    expect(picked.name, 'holiday.mp4');
    // The path cannot name the file, so this is what the core is told.
    expect(picked.fdDisplayName, 'holiday.mp4');
    expect(picked.sizeBytes, BigInt.from(6000000000));
    expect(picked.kind, SendPickedFileKind.file);
  });

  test('an ordinary path source carries no fd display name', () {
    final picked = sendPickedFileFromNativeSource(
      NativeSource.fromPath('/cache/report.pdf'),
    );

    expect(picked.path, '/cache/report.pdf');
    expect(picked.name, 'report.pdf');
    expect(picked.fdDisplayName, isNull);
  });

  test('draft items hand the core exactly what the picker resolved', () {
    final item = SendDraftItem.fromPickedFile(
      sendPickedFileFromNativeSource(
        const NativeSource(
          path: '/proc/self/fd/7',
          name: 'clip.mov',
          fromDescriptor: true,
        ),
      ),
    );

    expect(
      item.source,
      const SendSource(path: '/proc/self/fd/7', fdDisplayName: 'clip.mov'),
    );
  });
}
