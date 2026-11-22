import 'package:app/features/saved_devices/application/saved_device.dart';
import 'package:app/features/saved_devices/application/saved_devices_controller.dart';
import 'package:flutter_test/flutter_test.dart';

SavedDevice _device(String id, int minutesAgo, {bool favorite = false}) =>
    SavedDevice(
      endpointId: id,
      label: id,
      deviceType: 'laptop',
      lastSeenAt: DateTime.utc(
        2026,
        11,
        22,
        12,
      ).subtract(Duration(minutes: minutesAgo)),
      transferCount: 1,
      totalBytes: BigInt.zero,
      favorite: favorite,
    );

void main() {
  test('favorites come first, each group newest first', () {
    final ordered = favoritesFirst([
      _device('recent', 1),
      _device('old-fav', 90, favorite: true),
      _device('older', 30),
      _device('new-fav', 10, favorite: true),
    ]);
    expect(ordered.map((d) => d.endpointId), [
      'new-fav',
      'old-fav',
      'recent',
      'older',
    ]);
  });

  test('no favorites keeps newest-first order', () {
    final ordered = favoritesFirst([_device('b', 5), _device('a', 1)]);
    expect(ordered.map((d) => d.endpointId), ['a', 'b']);
  });

  test('empty in, empty out', () {
    expect(favoritesFirst(const []), isEmpty);
  });
}
