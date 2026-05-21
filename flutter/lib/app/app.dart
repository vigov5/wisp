import 'dart:async';
import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:go_router/go_router.dart';

import '../features/receive/application/service.dart';
import '../features/send/application/controller.dart';
import '../features/send/application/model.dart';
import '../features/send/application/state.dart';
import '../features/transfers/application/controller.dart';
import '../features/transfers/application/service.dart';
import '../features/transfers/application/state.dart' as transfer_state;
import 'app_router.dart';
import '../theme/wisp_theme.dart';
import '../platform/android/keepalive_lifecycle_observer.dart';
import '../platform/android_share_intent.dart';
import '../platform/rust/receiver/source.dart';

class WispApp extends ConsumerStatefulWidget {
  const WispApp({super.key});

  @override
  ConsumerState<WispApp> createState() => _WispAppState();
}

class _WispAppState extends ConsumerState<WispApp> {
  late final GoRouter _router;
  late final ReceiverServiceSource _receiverService;
  late final KeepaliveLifecycleObserver _keepaliveObserver;
  StreamSubscription<List<String>>? _shareIntentSub;
  bool _discoverableEnabled = false;

  @override
  void initState() {
    super.initState();
    _router = buildAppRouter(
      observers: [DiscoveryRouterObserver(_syncReceiverDiscovery)],
    );
    _receiverService = ref.read(receiverServiceSourceProvider);
    _keepaliveObserver = KeepaliveLifecycleObserver(
      hasActiveTransfer: _hasActiveTransfer,
    );
    WidgetsBinding.instance.addObserver(_keepaliveObserver);
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted) {
        _syncReceiverDiscovery();
        _wireShareIntent();
      }
    });
  }

  // Wires the Android ACTION_SEND / ACTION_SEND_MULTIPLE intent into the
  // Send draft route.  On cold start the cached files are pulled once;
  // on warm start they arrive via the broadcast stream.  No-op on
  // non-Android platforms.
  void _wireShareIntent() {
    if (!Platform.isAndroid) return;
    unawaited(
      AndroidShareIntent.getInitialSharedFiles().then((paths) {
        if (!mounted) return;
        _openSendDraftWith(paths);
      }),
    );
    _shareIntentSub = AndroidShareIntent.onSharedFiles.listen((paths) {
      if (!mounted) return;
      _openSendDraftWith(paths);
    });
  }

  void _openSendDraftWith(List<String> paths) {
    if (paths.isEmpty) return;
    final files = paths.map((path) {
      final file = File(path);
      return SendPickedFile(
        path: path,
        name: SendPickedFile.fromPath(path).name,
        sizeBytes: file.existsSync() ? BigInt.from(file.lengthSync()) : null,
      );
    }).toList(growable: false);
    _router.go(AppRoutePaths.sendDraft, extra: files);
  }

  bool _hasActiveTransfer() {
    final sendState = ref.read(sendControllerProvider);
    final sendActive =
        sendState is SendStateTransferring && !sendState.transfer.isTerminal;
    final receiveActive =
        ref.read(transfersServiceProvider).phase ==
        transfer_state.TransferSessionPhase.receiving;
    return sendActive || receiveActive;
  }

  void _syncReceiverDiscovery() {
    final routePath = _router.routeInformationProvider.value.uri.path;
    final enabled = routePath == AppRoutePaths.home;
    if (enabled == _discoverableEnabled) {
      return;
    }
    _discoverableEnabled = enabled;
    debugPrint(
      '[app] receiver discovery ${enabled ? 'enabled' : 'disabled'} '
      'route="$routePath"',
    );
    unawaited(_receiverService.setDiscoverable(enabled: enabled));
  }

  @override
  void dispose() {
    unawaited(_shareIntentSub?.cancel());
    WidgetsBinding.instance.removeObserver(_keepaliveObserver);
    unawaited(_receiverService.setDiscoverable(enabled: false));
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    // When an incoming offer arrives while the user is on any other screen
    // (Settings, Saved devices, QR pairing/scan, deep dialogs, etc.), pop
    // anything stacked on top of the GoRouter and push the receive-transfer
    // route so the user sees the confirm prompt immediately.
    ref.listen<transfer_state.TransferSessionState>(transfersViewStateProvider, (
      prev,
      next,
    ) {
      final wasIdle =
          prev == null || prev.phase == transfer_state.TransferSessionPhase.idle;
      final nowActive =
          next.phase != transfer_state.TransferSessionPhase.idle;
      if (!wasIdle || !nowActive) return;

      final currentPath = _router.routeInformationProvider.value.uri.path;
      if (currentPath == AppRoutePaths.receiveTransfer) return;

      // Dismiss any modal routes pushed via Navigator (QrPairingPage,
      // QrScanPage, etc.) so the receive-transfer route lands on top.
      final navContext = _router.routerDelegate.navigatorKey.currentContext;
      if (navContext != null) {
        Navigator.of(navContext).popUntil((route) => route.isFirst);
      }
      _router.push(AppRoutePaths.receiveTransfer);
    });

    return MaterialApp.router(
      title: 'Wisp',
      debugShowCheckedModeBanner: false,
      theme: buildWispTheme(),
      routerConfig: _router,
    );
  }
}

class DiscoveryRouterObserver extends NavigatorObserver {
  DiscoveryRouterObserver(this._sync);

  final VoidCallback _sync;

  void _scheduleSync() {
    WidgetsBinding.instance.addPostFrameCallback((_) => _sync());
  }

  @override
  void didPush(Route<dynamic> route, Route<dynamic>? previousRoute) {
    super.didPush(route, previousRoute);
    _scheduleSync();
  }

  @override
  void didPop(Route<dynamic> route, Route<dynamic>? previousRoute) {
    super.didPop(route, previousRoute);
    _scheduleSync();
  }

  @override
  void didRemove(Route<dynamic> route, Route<dynamic>? previousRoute) {
    super.didRemove(route, previousRoute);
    _scheduleSync();
  }

  @override
  void didReplace({Route<dynamic>? newRoute, Route<dynamic>? oldRoute}) {
    super.didReplace(newRoute: newRoute, oldRoute: oldRoute);
    _scheduleSync();
  }
}
