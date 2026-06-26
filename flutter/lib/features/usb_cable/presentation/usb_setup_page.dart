import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../../platform/android/usb_tether_channel.dart';
import '../../../src/rust/api/lan.dart' as rust_lan;
import '../../../theme/wisp_theme.dart';
import '../application/usb_cable_controller.dart';
import '../application/usb_link_status.dart';

/// Which USB transport the user is setting up.
enum _UsbMode { phoneToPhone, tether }

/// Status of a single setup step, mirroring the connection self-test's
/// pass/running/pending look.
enum _StepState { pending, active, done, failed }

extension _StepStateX on _StepState {
  Color get color => switch (this) {
    _StepState.done => kAccentDirect,
    _StepState.active => kAccentCyan,
    _StepState.failed => const Color(0xFFCC3333),
    _StepState.pending => kSubtle,
  };
}

/// Dedicated, full-screen USB setup hub reached from the home top bar (USB icon
/// beside the QR button). Lets the user pick between the two USB transports and
/// walks each through ordered steps with live per-step status — so both phones
/// can be set up and verified *before* heading to Send. The direct phone↔phone
/// link is driven by [usbCableControllerProvider]; the phone↔computer tether is
/// a guided checklist over [usbTetherLinkProvider].
class UsbSetupPage extends ConsumerStatefulWidget {
  const UsbSetupPage({super.key});

  @override
  ConsumerState<UsbSetupPage> createState() => _UsbSetupPageState();
}

class _UsbSetupPageState extends ConsumerState<UsbSetupPage> {
  _UsbMode? _selected;

  @override
  Widget build(BuildContext context) {
    final cable = ref.watch(usbCableControllerProvider);
    final tether = ref.watch(usbTetherLinkProvider).value;
    final tetherUp = isTetherLink(tether);

    // Default the selection to the direct link where it's supported (the
    // headline feature); otherwise the tether checklist.
    final selected = _selected ?? (cable.supported ? _UsbMode.phoneToPhone : _UsbMode.tether);

    return Scaffold(
      backgroundColor: kBg,
      appBar: AppBar(
        backgroundColor: kBg,
        elevation: 0,
        title: Text(
          'USB transfer',
          style: wispSans(fontSize: 17, fontWeight: FontWeight.w700, color: kInk),
        ),
      ),
      body: SafeArea(
        child: ListView(
          padding: const EdgeInsets.fromLTRB(16, 8, 16, 28),
          children: [
            Text(
              'Move files over a USB cable — a direct, encrypted link with no '
              'shared Wi-Fi. Plug both phones in, pick a role on each '
              '(Accessory on the phone that controls USB, Host on the other), '
              'then Start VPN on both. Then head to Send or share your receive '
              'code as usual.',
              style: wispSans(fontSize: 13, color: kMuted, height: 1.45),
            ),
            const SizedBox(height: 18),
            if (cable.supported)
              _ModeCard(
                icon: Icons.smartphone_rounded,
                title: 'Phone to phone',
                subtitle: 'One cable between two phones — pick a role on each.',
                connected: cable.tunnelUp,
                selected: selected == _UsbMode.phoneToPhone,
                onTap: () => setState(() => _selected = _UsbMode.phoneToPhone),
              ),
            if (cable.supported) const SizedBox(height: 12),
            _ModeCard(
              icon: Icons.laptop_mac_rounded,
              title: 'Phone to computer',
              subtitle: 'USB tethering bridges the phone and a computer.',
              connected: tetherUp,
              selected: selected == _UsbMode.tether,
              onTap: () => setState(() => _selected = _UsbMode.tether),
            ),
            const SizedBox(height: 20),
            if (selected == _UsbMode.phoneToPhone && cable.supported)
              _PhoneToPhonePanel(state: cable)
            else
              _TetherPanel(link: tether),
          ],
        ),
      ),
    );
  }
}

/// Selectable mode tile. Highlights when picked; shows a small dot when that
/// transport is currently connected.
class _ModeCard extends StatelessWidget {
  const _ModeCard({
    required this.icon,
    required this.title,
    required this.subtitle,
    required this.connected,
    required this.selected,
    required this.onTap,
  });

  final IconData icon;
  final String title;
  final String subtitle;
  final bool connected;
  final bool selected;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    return InkWell(
      onTap: onTap,
      borderRadius: BorderRadius.circular(16),
      child: AnimatedContainer(
        duration: const Duration(milliseconds: 160),
        padding: const EdgeInsets.symmetric(horizontal: 14, vertical: 14),
        decoration: BoxDecoration(
          color: selected ? const Color(0xFFF4F8FA) : kSurface,
          borderRadius: BorderRadius.circular(16),
          border: Border.all(
            color: selected ? const Color(0xFF8DBED4) : kBorder,
            width: selected ? 1.4 : 1,
          ),
        ),
        child: Row(
          children: [
            Container(
              width: 38,
              height: 38,
              alignment: Alignment.center,
              decoration: BoxDecoration(
                color: selected ? kAccentCyanHover : kFill,
                borderRadius: BorderRadius.circular(10),
              ),
              child: Icon(icon, size: 20, color: selected ? kAccentCyanStrong : kMuted),
            ),
            const SizedBox(width: 12),
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Row(
                    children: [
                      Text(
                        title,
                        style: wispSans(
                          fontSize: 14.5,
                          fontWeight: FontWeight.w700,
                          color: kInk,
                        ),
                      ),
                      if (connected) ...[
                        const SizedBox(width: 8),
                        _ConnectedDot(),
                      ],
                    ],
                  ),
                  const SizedBox(height: 3),
                  Text(
                    subtitle,
                    style: wispSans(fontSize: 12.5, color: kMuted, height: 1.35),
                  ),
                ],
              ),
            ),
            const SizedBox(width: 8),
            Icon(
              selected
                  ? Icons.radio_button_checked_rounded
                  : Icons.radio_button_unchecked_rounded,
              size: 20,
              color: selected ? kAccentCyan : kSubtle,
            ),
          ],
        ),
      ),
    );
  }
}

class _ConnectedDot extends StatelessWidget {
  @override
  Widget build(BuildContext context) {
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 7, vertical: 2),
      decoration: BoxDecoration(
        color: kAccentDirect.withValues(alpha: 0.12),
        borderRadius: BorderRadius.circular(6),
      ),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          Container(
            width: 6,
            height: 6,
            decoration: const BoxDecoration(
              color: kAccentDirect,
              shape: BoxShape.circle,
            ),
          ),
          const SizedBox(width: 5),
          Text(
            'Connected',
            style: wispSans(
              fontSize: 10.5,
              fontWeight: FontWeight.w700,
              color: kAccentDirect,
            ),
          ),
        ],
      ),
    );
  }
}

/// Steps for the direct phone↔phone (AOA) link, derived from the controller's
/// lifecycle phase.
class _PhoneToPhonePanel extends ConsumerWidget {
  const _PhoneToPhonePanel({required this.state});

  final UsbCableState state;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final phase = state.phase;
    final err = state.error;
    final role = state.role; // "host" | "accessory"
    final connecting = phase == UsbCablePhase.connecting;
    final linkFailed = phase == UsbCablePhase.error;
    // A VPN failure keeps the link up and records the error on the (linkUp)
    // tunnel step rather than dropping to the error phase.
    final vpnFailed = state.linkUp && !state.tunnelUp && err != null;

    // Step 1 — cable present (informational; roles are chosen manually).
    final s1 = phase == UsbCablePhase.idle ? _StepState.pending : _StepState.done;

    // Step 2 — AOA link established.
    final _StepState s2;
    if (connecting) {
      s2 = _StepState.active;
    } else if (state.linkUp) {
      s2 = _StepState.done;
    } else if (linkFailed) {
      s2 = _StepState.failed;
    } else {
      s2 = _StepState.pending;
    }

    // Step 3 — IP tunnel (VPN) up.
    final _StepState s3;
    if (phase == UsbCablePhase.tunnelUp) {
      s3 = _StepState.done;
    } else if (state.tunnelStarting) {
      s3 = _StepState.active;
    } else if (vpnFailed) {
      s3 = _StepState.failed;
    } else {
      s3 = _StepState.pending;
    }

    final roleWord = role == 'host'
        ? 'host'
        : role == 'accessory'
        ? 'accessory'
        : null;
    final detectedWord = state.detectedRole == 'host' ? 'Host' : 'Accessory';

    return _StepPanel(
      title: 'Phone-to-phone setup',
      steps: [
        _StepData(
          state: s1,
          title: 'Cable connected',
          detail: s1 == _StepState.done
              ? 'USB cable detected.'
              : 'Plug one USB-C cable into both phones.',
        ),
        _StepData(
          state: s2,
          title: 'Direct link established',
          detail: switch (s2) {
            _StepState.done => roleWord == null
                ? 'Linked over USB.'
                : 'Linked — this phone is the $roleWord.',
            _StepState.active => role == 'accessory'
                ? 'Waiting to be switched — keep this open and start Host on '
                      'the other phone.'
                : 'Driving the link — approve the USB permission prompt.',
            _StepState.failed => err ?? 'Could not establish the USB link.',
            _StepState.pending =>
              'This phone is the $detectedWord. Order: ① the Accessory phone '
                  'connects, ② the Host phone connects, ③ both Start VPN.',
          },
        ),
        _StepData(
          state: s3,
          title: 'Secure connection ready',
          detail: switch (s3) {
            _StepState.done =>
              'Ready${state.localIp != null ? ' · ${state.localIp}' : ''}. '
                  'Send to or receive from the other phone over the cable.',
            _StepState.active =>
              'Bringing up the encrypted tunnel — approve the VPN prompt (once).',
            _StepState.failed => err ?? 'Could not bring up the tunnel.',
            _StepState.pending => '③ Tap Start VPN on both phones.',
          },
        ),
      ],
      footer: _PhoneToPhoneAction(state: state),
    );
  }
}

class _PhoneToPhoneAction extends ConsumerWidget {
  const _PhoneToPhoneAction({required this.state});

  final UsbCableState state;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final notifier = ref.read(usbCableControllerProvider.notifier);
    final phase = state.phase;

    // Connected — the obvious next move is to go send.
    if (phase == UsbCablePhase.tunnelUp) {
      return Row(
        children: [
          Expanded(
            child: _filled(
              label: 'Done',
              icon: Icons.check_rounded,
              onPressed: () => Navigator.of(context).maybePop(),
            ),
          ),
          const SizedBox(width: 10),
          OutlinedButton(
            onPressed: () => unawaited(notifier.disable()),
            child: const Text('Disconnect'),
          ),
        ],
      );
    }

    // Establishing the AOA link, or bringing up the VPN — show progress + Stop.
    if (phase == UsbCablePhase.connecting || state.tunnelStarting) {
      final what = state.tunnelStarting ? 'Starting VPN…' : 'Connecting…';
      return Row(
        children: [
          Expanded(child: _spinnerButton(what)),
          const SizedBox(width: 10),
          OutlinedButton(
            onPressed: () => unawaited(notifier.stop()),
            child: const Text('Stop'),
          ),
        ],
      );
    }

    // Link up, VPN not started → the explicit Start VPN step (both phones).
    if (state.linkUp) {
      final retry = state.error != null;
      return Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          _filled(
            label: retry ? '③ Retry VPN' : '③ Start VPN',
            icon: Icons.vpn_lock_rounded,
            onPressed: () => unawaited(notifier.startVpn()),
          ),
          const SizedBox(height: 8),
          Center(
            child: TextButton(
              onPressed: () => unawaited(notifier.disable()),
              child: Text(
                'Disconnect',
                style: wispSans(fontSize: 12.5, color: kMuted),
              ),
            ),
          ),
        ],
      );
    }

    // Idle / detected / link-failed → connect in this phone's detected role.
    // The role is auto-assigned from the USB bus; a secondary action covers the
    // rare case where detection is off.
    final isHost = state.detectedRole == 'host';
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        _filled(
          label: isHost ? '② Connect as host' : '① Connect as accessory',
          icon: isHost ? Icons.usb_rounded : Icons.cable_rounded,
          onPressed: () => unawaited(
            isHost ? notifier.connectAsHost() : notifier.connectAsAccessory(),
          ),
        ),
        const SizedBox(height: 8),
        Center(
          child: TextButton(
            onPressed: () => unawaited(
              isHost ? notifier.connectAsAccessory() : notifier.connectAsHost(),
            ),
            child: Text(
              isHost ? "I'm the accessory instead" : "I'm the host instead",
              style: wispSans(fontSize: 12.5, color: kMuted),
            ),
          ),
        ),
      ],
    );
  }

  Widget _filled({
    required String label,
    required IconData icon,
    required VoidCallback onPressed,
  }) => FilledButton.icon(
    onPressed: onPressed,
    icon: Icon(icon, size: 18),
    label: Text(label),
    style: FilledButton.styleFrom(
      backgroundColor: kAccentCyanStrong,
      foregroundColor: Colors.white,
      padding: const EdgeInsets.symmetric(vertical: 13),
      shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(12)),
    ),
  );

  Widget _spinnerButton(String label) => FilledButton.icon(
    onPressed: null,
    icon: const SizedBox(
      width: 16,
      height: 16,
      child: CircularProgressIndicator(strokeWidth: 2, color: Colors.white),
    ),
    label: Text(label),
    style: FilledButton.styleFrom(
      backgroundColor: kAccentCyanStrong,
      disabledBackgroundColor: kAccentCyanStrong.withValues(alpha: 0.55),
      disabledForegroundColor: Colors.white,
      padding: const EdgeInsets.symmetric(vertical: 13),
      shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(12)),
    ),
  );
}

/// Steps for the phone↔computer USB-tether checklist, derived from the detected
/// link. Manual flow, so completed steps show a check and the rest stay as
/// numbered placeholders (no misleading spinners).
class _TetherPanel extends StatelessWidget {
  const _TetherPanel({required this.link});

  final rust_lan.UsbLinkData? link;

  @override
  Widget build(BuildContext context) {
    final on = isTetherLink(link);
    final done = on ? _StepState.done : _StepState.pending;

    return _StepPanel(
      title: 'Phone-to-computer setup',
      steps: [
        _StepData(
          state: done,
          title: 'Connect the cable',
          detail: on
              ? 'Cable detected.'
              : 'Plug the phone into the computer with a USB cable.',
        ),
        _StepData(
          state: done,
          title: 'Turn on USB tethering',
          detail: on
              ? 'USB tethering is on.'
              : 'On the phone: Settings → Hotspot & tethering → USB tethering.',
          trailing: on || !UsbTether.isSupported
              ? null
              : _TetherSettingsButton(),
        ),
        _StepData(
          state: done,
          title: 'Cable link active',
          detail: on
              ? 'Connected · ${link!.localIp}. The other device shows under '
                    '"Nearby devices" on Send.'
              : 'Once tethering is on, the link comes up automatically.',
        ),
      ],
    );
  }
}

class _TetherSettingsButton extends StatelessWidget {
  @override
  Widget build(BuildContext context) {
    return OutlinedButton.icon(
      onPressed: () async {
        final opened = await UsbTether.openTetherSettings();
        if (!context.mounted || opened) return;
        ScaffoldMessenger.of(context).showSnackBar(
          const SnackBar(
            content: Text(
              'Open Settings → Hotspot & tethering, then turn on USB tethering.',
            ),
            duration: Duration(seconds: 4),
          ),
        );
      },
      icon: const Icon(Icons.settings_rounded, size: 16),
      label: const Text('Open tethering settings'),
    );
  }
}

class _StepData {
  const _StepData({
    required this.state,
    required this.title,
    required this.detail,
    this.trailing,
  });

  final _StepState state;
  final String title;
  final String detail;
  final Widget? trailing;
}

/// Card wrapping an ordered, connected list of steps (top→down) with an
/// optional footer action.
class _StepPanel extends StatelessWidget {
  const _StepPanel({required this.title, required this.steps, this.footer});

  final String title;
  final List<_StepData> steps;
  final Widget? footer;

  @override
  Widget build(BuildContext context) {
    return Container(
      padding: const EdgeInsets.fromLTRB(16, 16, 16, 16),
      decoration: BoxDecoration(
        color: kSurface,
        borderRadius: BorderRadius.circular(16),
        border: Border.all(color: kBorder.withValues(alpha: 0.7)),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(
            title,
            style: wispSans(fontSize: 12, fontWeight: FontWeight.w700, color: kMuted),
          ),
          const SizedBox(height: 14),
          for (var i = 0; i < steps.length; i++)
            _StepRow(
              index: i + 1,
              data: steps[i],
              isLast: i == steps.length - 1,
            ),
          if (footer != null) ...[
            const SizedBox(height: 6),
            footer!,
          ],
        ],
      ),
    );
  }
}

class _StepRow extends StatelessWidget {
  const _StepRow({
    required this.index,
    required this.data,
    required this.isLast,
  });

  final int index;
  final _StepData data;
  final bool isLast;

  @override
  Widget build(BuildContext context) {
    // Connector reads "done" green once this step is complete; otherwise muted.
    final lineColor = data.state == _StepState.done
        ? kAccentDirect.withValues(alpha: 0.45)
        : kBorder;

    return IntrinsicHeight(
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Column(
            children: [
              _StepNode(state: data.state, index: index),
              if (!isLast)
                Expanded(
                  child: Container(width: 2, color: lineColor),
                ),
            ],
          ),
          const SizedBox(width: 12),
          Expanded(
            child: Padding(
              padding: EdgeInsets.only(bottom: isLast ? 0 : 18),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Padding(
                    padding: const EdgeInsets.only(top: 1),
                    child: Text(
                      data.title,
                      style: wispSans(
                        fontSize: 13.5,
                        fontWeight: FontWeight.w600,
                        color: data.state == _StepState.pending ? kMuted : kInk,
                      ),
                    ),
                  ),
                  const SizedBox(height: 3),
                  Text(
                    data.detail,
                    style: wispSans(fontSize: 12.5, color: kMuted, height: 1.4),
                  ),
                  if (data.trailing != null) ...[
                    const SizedBox(height: 10),
                    data.trailing!,
                  ],
                ],
              ),
            ),
          ),
        ],
      ),
    );
  }
}

class _StepNode extends StatelessWidget {
  const _StepNode({required this.state, required this.index});

  final _StepState state;
  final int index;

  @override
  Widget build(BuildContext context) {
    const size = 26.0;
    switch (state) {
      case _StepState.active:
        return SizedBox(
          width: size,
          height: size,
          child: Center(
            child: SizedBox(
              width: 18,
              height: 18,
              child: CircularProgressIndicator(
                strokeWidth: 2,
                color: state.color,
              ),
            ),
          ),
        );
      case _StepState.done:
        return Icon(Icons.check_circle_rounded, size: size, color: state.color);
      case _StepState.failed:
        return Icon(Icons.error_rounded, size: size, color: state.color);
      case _StepState.pending:
        return Container(
          width: size,
          height: size,
          alignment: Alignment.center,
          decoration: BoxDecoration(
            shape: BoxShape.circle,
            border: Border.all(color: kBorder, width: 1.5),
          ),
          child: Text(
            '$index',
            style: wispSans(
              fontSize: 12,
              fontWeight: FontWeight.w700,
              color: kMuted,
            ),
          ),
        );
    }
  }
}
