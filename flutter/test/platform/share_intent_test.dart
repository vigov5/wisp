import 'package:app/platform/native_source.dart';
import 'package:app/platform/share_intent.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  test('Android hands over sources and whatever it could not prepare', () {
    final shared = SharedFiles.parse(<String, Object>{
      'sources': <Map<String, Object>>[
        {
          'path': '/proc/self/fd/42',
          'name': 'holiday.mp4',
          'size': 6000000000,
          'copied': false,
        },
      ],
      'rejected': <Map<String, Object>>[
        {
          'name': 'demo.rar',
          'reason': 'no_space',
          'requiredBytes': 6218218542,
          'availableBytes': 5100000000,
        },
      ],
    });

    expect(shared.sources.single.name, 'holiday.mp4');
    expect(shared.sources.single.fromDescriptor, isTrue);

    final rejected = shared.rejected.single;
    expect(rejected.name, 'demo.rar');
    expect(rejected.reason, SourceRejectionReason.noSpace);
    expect(rejected.requiredBytes, BigInt.from(6218218542));
    expect(rejected.availableBytes, BigInt.from(5100000000));
    expect(shared.isEmpty, isFalse);
  });

  test('a share that prepared nothing still carries the reason', () {
    // The whole point: an empty draft has to be able to say why it is empty.
    final shared = SharedFiles.parse(<String, Object>{
      'sources': <Object>[],
      'rejected': <Map<String, Object>>[
        {'name': 'demo.rar', 'reason': 'no_space'},
      ],
    });

    expect(shared.sources, isEmpty);
    expect(shared.rejected, hasLength(1));
    expect(shared.isEmpty, isFalse);
  });

  test('iOS still sends a bare list of paths', () {
    final shared = SharedFiles.parse(<Object>['/tmp/holiday.mp4']);

    expect(shared.sources.single.path, '/tmp/holiday.mp4');
    expect(shared.sources.single.name, 'holiday.mp4');
    expect(shared.rejected, isEmpty);
  });

  test('an unrecognised reason is treated as unreadable, not as space', () {
    final shared = SharedFiles.parse(<String, Object>{
      'rejected': <Map<String, Object>>[
        {'name': 'odd.bin', 'reason': 'something_new'},
      ],
    });

    expect(shared.rejected.single.reason, SourceRejectionReason.unreadable);
  });

  test('nothing at all parses to empty rather than throwing', () {
    expect(SharedFiles.parse(null).isEmpty, isTrue);
    expect(SharedFiles.parse(const <Object>[]).isEmpty, isTrue);
  });
}
