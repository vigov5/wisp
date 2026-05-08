import 'package:app/features/transfers/application/connection_path.dart';
import 'package:app/src/rust/api/receiver.dart' as rust_receiver;
import 'package:app/src/rust/api/sender.dart' as rust_sender;
import 'package:flutter_test/flutter_test.dart';

void main() {
  group('ConnectionPathInfo.fromReceiver', () {
    test('returns null when path is null', () {
      expect(ConnectionPathInfo.fromReceiver(null), isNull);
    });

    test('parses p2p kind with direct addr', () {
      final info = ConnectionPathInfo.fromReceiver(
        const rust_receiver.ReceiverConnectionPath(
          kind: 'p2p',
          relayUrl: null,
          directAddr: '192.168.1.5:5000',
        ),
      );
      expect(info, isNotNull);
      expect(info!.kind, ConnectionPathKind.direct);
      expect(info.isDirect, isTrue);
      expect(info.relayUrl, isNull);
      expect(info.relayHost, isNull);
      expect(info.directAddr, '192.168.1.5:5000');
      expect(info.directIpHost, '192.168.1.5');
    });

    test('directIpHost handles IPv6 with port', () {
      const info = ConnectionPathInfo(
        kind: ConnectionPathKind.direct,
        directAddr: '[fe80::1]:5000',
      );
      expect(info.directIpHost, 'fe80::1');
    });

    test('directIpHost is null when directAddr null', () {
      const info = ConnectionPathInfo(kind: ConnectionPathKind.direct);
      expect(info.directIpHost, isNull);
    });

    test('parses relay kind and exposes host portion', () {
      final info = ConnectionPathInfo.fromReceiver(
        const rust_receiver.ReceiverConnectionPath(
          kind: 'relay',
          relayUrl: 'https://relay.example.com:443/',
        ),
      );
      expect(info!.kind, ConnectionPathKind.relay);
      expect(info.isRelay, isTrue);
      expect(info.relayHost, 'relay.example.com');
    });

    test('falls back to unknown kind for unrecognized strings', () {
      final info = ConnectionPathInfo.fromReceiver(
        const rust_receiver.ReceiverConnectionPath(
          kind: 'mystery',
          relayUrl: null,
        ),
      );
      expect(info!.kind, ConnectionPathKind.unknown);
    });

    test('returns null relayHost when URL is malformed', () {
      final info = ConnectionPathInfo.fromReceiver(
        const rust_receiver.ReceiverConnectionPath(
          kind: 'relay',
          relayUrl: '://not-a-url',
        ),
      );
      expect(info!.kind, ConnectionPathKind.relay);
      expect(info.relayHost, anyOf(isNull, isEmpty));
    });
  });

  group('ConnectionPathInfo.fromSender', () {
    test('returns null when path is null', () {
      expect(ConnectionPathInfo.fromSender(null), isNull);
    });

    test('maps p2p sender kind to direct', () {
      final info = ConnectionPathInfo.fromSender(
        const rust_sender.SendConnectionPath(kind: 'p2p', relayUrl: null),
      );
      expect(info!.kind, ConnectionPathKind.direct);
    });

    test('maps relay sender kind with relay URL', () {
      final info = ConnectionPathInfo.fromSender(
        const rust_sender.SendConnectionPath(
          kind: 'relay',
          relayUrl: 'https://relay.eu.example/',
        ),
      );
      expect(info!.kind, ConnectionPathKind.relay);
      expect(info.relayUrl, 'https://relay.eu.example/');
      expect(info.relayHost, 'relay.eu.example');
    });
  });

  group('equality', () {
    test('same kind and relayUrl compare equal', () {
      const a = ConnectionPathInfo(
        kind: ConnectionPathKind.relay,
        relayUrl: 'https://r.example/',
      );
      const b = ConnectionPathInfo(
        kind: ConnectionPathKind.relay,
        relayUrl: 'https://r.example/',
      );
      expect(a, equals(b));
      expect(a.hashCode, equals(b.hashCode));
    });

    test('different kind not equal', () {
      const a = ConnectionPathInfo(kind: ConnectionPathKind.direct);
      const b = ConnectionPathInfo(kind: ConnectionPathKind.relay);
      expect(a, isNot(equals(b)));
    });
  });
}
