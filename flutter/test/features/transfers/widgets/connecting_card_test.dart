import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shared_preferences/shared_preferences.dart';

import 'package:app/features/saved_devices/application/saved_devices_controller.dart';
import 'package:app/features/transfers/application/identity.dart';
import 'package:app/features/transfers/application/manifest.dart';
import 'package:app/features/transfers/application/state.dart';
import 'package:app/features/transfers/presentation/widgets/connecting_card.dart';

import '../../../support/test_overrides.dart';

void main() {
  setUp(() => SharedPreferences.setMockInitialValues({}));

  TransferIncomingOffer connectingOffer({
    String senderName = 'Maya',
    String statusMessage = 'Maya is connecting…',
  }) {
    return TransferIncomingOffer(
      sender: TransferIdentity(
        role: TransferRole.sender,
        endpointId: 'endpoint-1',
        deviceName: senderName,
        deviceType: DeviceType.laptop,
      ),
      // Pre-offer: identity known, manifest empty.
      manifest: const TransferManifest(items: []),
      destinationLabel: senderName,
      saveRootLabel: 'Downloads',
      statusMessage: statusMessage,
      bytesReceived: BigInt.zero,
      senderEndpointId: 'endpoint-1',
    );
  }

  testWidgets('ConnectingCard shows the sender and a connecting status', (
    tester,
  ) async {
    final savedDevicesRepo = await mockSavedDevicesRepo();
    await tester.pumpWidget(
      ProviderScope(
        overrides: [
          savedDevicesRepositoryProvider.overrideWithValue(savedDevicesRepo),
        ],
        child: MaterialApp(
          home: Scaffold(
            body: ConnectingCard(offer: connectingOffer(), animate: false),
          ),
        ),
      ),
    );
    await tester.pump();

    // The status label is rendered upper-cased by TransferFlowLayout.
    expect(find.text('CONNECTING'), findsOneWidget);
    expect(find.text('Maya'), findsWidgets);
    expect(find.text('Maya is connecting…'), findsOneWidget);
    // No accept/decline affordance pre-offer.
    expect(find.text('Accept'), findsNothing);
    expect(find.text('Decline'), findsNothing);
  });
}
