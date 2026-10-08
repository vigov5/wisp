import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:flutter_test/flutter_test.dart';

import 'package:app/features/receive/feature.dart';
import 'package:app/features/settings/feature.dart';
import 'package:app/platform/rust/receiver/fake_source.dart';
import '../../support/settings_test_overrides.dart';

void main() {
  test('maps a registering receiver to the registering badge', () {
    final container = ProviderContainer(
      overrides: [
        initialAppSettingsProvider.overrideWithValue(testAppSettings),
        receiverServiceSourceProvider.overrideWithValue(
          FakeReceiverServiceSource(
            initialState: const ReceiverServiceState.registering(),
          ),
        ),
      ],
    );
    addTearDown(container.dispose);

    final state = container.read(receiverIdleViewStateProvider);

    expect(state.badge.label, 'Registering');
    expect(state.badge.phase, ReceiverBadgePhase.registering);
    expect(state.code, '......');
  });

  test('a confirmed code earns the green Ready badge', () {
    final container = ProviderContainer(
      overrides: [
        initialAppSettingsProvider.overrideWithValue(testAppSettings),
        receiverServiceSourceProvider.overrideWithValue(
          FakeReceiverServiceSource(
            initialState: ReceiverServiceState.ready(code: 'ABC123'),
          ),
        ),
      ],
    );
    addTearDown(container.dispose);

    final state = container.read(receiverIdleViewStateProvider);

    expect(state.badge.label, 'Ready');
    expect(state.badge.phase, ReceiverBadgePhase.ready);
  });

  test('a code the server has not confirmed does not read as Ready', () {
    // The receiver is still listening — LAN, QR and saved devices all work —
    // but nothing can vouch for the six digits on screen, so the badge must
    // not claim they work.
    final container = ProviderContainer(
      overrides: [
        initialAppSettingsProvider.overrideWithValue(testAppSettings),
        receiverServiceSourceProvider.overrideWithValue(
          FakeReceiverServiceSource(
            initialState: ReceiverServiceState.stale(code: 'ABC123'),
          ),
        ),
      ],
    );
    addTearDown(container.dispose);

    final state = container.read(receiverIdleViewStateProvider);

    expect(state.badge.phase, ReceiverBadgePhase.localOnly);
    expect(state.badge.label, 'Local only');
    expect(
      state.code,
      'ABC 123',
      reason:
          'the code stays on screen for context, it just stops being a promise',
    );
  });

  test('maps unavailable receiver to unavailable lifecycle', () {
    final container = ProviderContainer(
      overrides: [
        initialAppSettingsProvider.overrideWithValue(testAppSettings),
        receiverServiceSourceProvider.overrideWithValue(
          FakeReceiverServiceSource(
            initialState: const ReceiverServiceState.unavailable(),
          ),
        ),
      ],
    );
    addTearDown(container.dispose);

    final state = container.read(receiverIdleViewStateProvider);

    expect(state.badge.label, 'Unavailable');
    expect(state.lifecycle, ReceiverLifecycle.stopped);
    expect(state.code, '......');
  });
}
