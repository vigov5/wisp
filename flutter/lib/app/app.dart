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
import '../features/settings/settings_providers.dart';
import '../platform/android/keepalive_lifecycle_observer.dart';
import '../platform/android_share_intent.dart';
import '../platform/windows_context_menu.dart';
import '../platform/rust/receiver/source.dart';

class WispApp extends ConsumerStatefulWidget {
  const WispApp({super.key, this.initialSendPaths = const []});

  /// Process launch arguments. On Windows this carries a "Send via Wisp"
  /// file/folder path when the app is cold-started from the context menu.
  final List<String> initialSendPaths;

  @override
  ConsumerState<WispApp> createState() => _WispAppState();
}

class _WispAppState extends ConsumerState<WispApp> {
  late final GoRouter _router;
  late final ReceiverServiceSource _receiverService;
  late final KeepaliveLifecycleObserver _keepaliveObserver;
  StreamSubscription<List<String>>? _shareIntentSub;
  StreamSubscription<String>? _shareTextSub;
  StreamSubscription<List<String>>? _windowsSendSub;
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
        _wireWindowsSendIntent();
        unawaited(_maybePromptContextMenu());
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
    unawaited(
      AndroidShareIntent.getInitialSharedText().then((text) {
        if (!mounted || text == null) return;
        _openSendTextDraftWith(text);
      }),
    );
    _shareIntentSub = AndroidShareIntent.onSharedFiles.listen((paths) {
      if (!mounted) return;
      _openSendDraftWith(paths);
    });
    _shareTextSub = AndroidShareIntent.onSharedText.listen((text) {
      if (!mounted) return;
      _openSendTextDraftWith(text);
    });
  }

  // Wires the Windows "Send via Wisp" context-menu integration.  Cold-start
  // paths arrive as launch arguments (widget.initialSendPaths); warm-start
  // paths (forwarded from a second launch via WM_COPYDATA) arrive on the
  // broadcast stream and aggregate into the current draft.  No-op off Windows.
  void _wireWindowsSendIntent() {
    if (!Platform.isWindows) return;
    _handleWindowsSendPaths(widget.initialSendPaths);
    _windowsSendSub = WindowsContextMenu.onSendViaWisp.listen((paths) {
      if (!mounted) return;
      _handleWindowsSendPaths(paths);
    });
  }

  // Opens or extends the Send draft with the given file/folder paths.  When a
  // draft is already in progress the items are appended (multi-select and
  // forwarded launches aggregate into one draft); otherwise a fresh draft is
  // opened.
  void _handleWindowsSendPaths(List<String> paths) {
    final files = _sendPickedFilesFromPaths(paths);
    if (files.isEmpty) return;
    if (ref.read(sendControllerProvider) is SendStateDrafting) {
      ref.read(sendControllerProvider.notifier).appendDraftItems(files);
      final currentPath = _router.routeInformationProvider.value.uri.path;
      if (currentPath != AppRoutePaths.sendDraft) {
        _router.go(AppRoutePaths.sendDraft, extra: files);
      }
    } else {
      _router.go(AppRoutePaths.sendDraft, extra: files);
    }
  }

  // Shows the one-time "add Wisp to the right-click menu?" prompt on first
  // launch (Windows only), unless already prompted or already registered.
  Future<void> _maybePromptContextMenu() async {
    if (!Platform.isWindows) return;
    final repository = ref.read(settingsRepositoryProvider);
    if (repository.contextMenuPrompted()) return;
    if (await WindowsContextMenu.isRegistered()) {
      await repository.markContextMenuPrompted();
      return;
    }
    final navContext = _router.routerDelegate.navigatorKey.currentContext;
    if (navContext == null || !navContext.mounted) return;

    final add = await showDialog<bool>(
      context: navContext,
      builder: (context) => AlertDialog(
        title: const Text('Add "Send via Wisp" to the right-click menu?'),
        content: const Text(
          'Right-click any file or folder in File Explorer to send it '
          'instantly with Wisp. You can change this later in Settings.',
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(context).pop(false),
            child: const Text('Not now'),
          ),
          FilledButton(
            onPressed: () => Navigator.of(context).pop(true),
            style: FilledButton.styleFrom(
              backgroundColor: kAccentCyanStrong,
            ),
            child: const Text('Add'),
          ),
        ],
      ),
    );

    if (add == true) {
      await WindowsContextMenu.register();
    }
    await repository.markContextMenuPrompted();
  }

  void _openSendDraftWith(List<String> paths) {
    final files = _sendPickedFilesFromPaths(paths);
    if (files.isEmpty) return;
    _router.go(AppRoutePaths.sendDraft, extra: files);
  }

  // Converts raw file/folder paths into Send draft entries, using the directory
  // factory for folders so recursive sends are planned correctly.
  List<SendPickedFile> _sendPickedFilesFromPaths(List<String> paths) {
    return paths
        .where((path) => path.trim().isNotEmpty)
        .map((path) {
          if (FileSystemEntity.isDirectorySync(path)) {
            return SendPickedFile.directory(path);
          }
          final file = File(path);
          return SendPickedFile(
            path: path,
            name: SendPickedFile.fromPath(path).name,
            sizeBytes: file.existsSync()
                ? BigInt.from(file.lengthSync())
                : null,
          );
        })
        .toList(growable: false);
  }

  // Routes a shared text/plain payload into the Share-text flow: seed a text
  // draft, then open the draft/destination screen (no files attached).
  void _openSendTextDraftWith(String text) {
    if (text.trim().isEmpty) return;
    ref.read(sendControllerProvider.notifier).beginTextDraft(text);
    _router.go(AppRoutePaths.sendDraft, extra: const <SendPickedFile>[]);
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
    unawaited(_shareTextSub?.cancel());
    unawaited(_windowsSendSub?.cancel());
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
    ref.listen<transfer_state.TransferSessionState>(
      transfersViewStateProvider,
      (prev, next) {
        final wasIdle =
            prev == null ||
            prev.phase == transfer_state.TransferSessionPhase.idle;
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
      },
    );

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
