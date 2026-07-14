import 'package:flutter_test/flutter_test.dart';

import 'package:app/features/transfers/application/identity.dart';
import 'package:app/features/transfers/presentation/widgets/transfer_presentation_helpers.dart';

void main() {
  TransferIdentity ident({
    DeviceType type = DeviceType.laptop,
    bool web = false,
  }) => TransferIdentity(
    role: TransferRole.sender,
    endpointId: 'e',
    deviceName: 'X',
    deviceType: type,
    web: web,
  );

  test('web peer maps to the globe (web) device type', () {
    expect(avatarDeviceType(ident(web: true)), 'web');
    // web overrides the underlying laptop/phone type.
    expect(avatarDeviceType(ident(type: DeviceType.phone, web: true)), 'web');
  });

  test('non-web peer keeps its laptop/phone type', () {
    expect(avatarDeviceType(ident(type: DeviceType.laptop)), 'laptop');
    expect(avatarDeviceType(ident(type: DeviceType.phone)), 'phone');
  });
}
