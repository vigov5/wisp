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
import '../features/update/application/update_providers.dart';
import '../features/update/domain/update_status.dart';
import '../features/update/presentation/update_available_dialog.dart';
import 'app_router.dart';
import '../theme/wisp_theme.dart';
import '../features/settings/settings_providers.dart';
import '../features/settings/application/controller.dart';
import '../platform/android/keepalive_lifecycle_observer.dart';
import '../features/usb_cable/application/usb_cable_controller.dart';
import '../platform/android_share_intent.dart';
import '../platform/desktop_integration.dart';
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

class _WispAppState extends ConsumerState<WispApp> with WidgetsBindingObserver {
  late final GoRouter _router;
  late final ReceiverServiceSource _receiverService;
  late final KeepaliveLifecycleObserver _keepaliveObserver;
  StreamSubscription<List<String>>? _shareIntentSub;
  StreamSubscription<String>? _shareTextSub;
  StreamSubscription<List<String>>? _windowsSendSub;
  bool _discoverableEnabled = false;
  bool _isForeground = true;
  // True while the USB-cable IP tunnel is up. Forces discoverability on so the
  // device advertises over the cable and EITHER phone can send to the other
  // (the cable peer is found via unicast discovery to the tunnel gateway).
  // Sourced from usbCableControllerProvider (single AOA event subscription).
  bool _usbCableTunnelUp = false;

  @override
  void initState() {
    super.initState();
    _router = buildAppRouter(
      observers: [DiscoveryRouterObserver(_applyDiscoverability)],
    );
    _receiverService = ref.read(receiverServiceSourceProvider);
    _keepaliveObserver = KeepaliveLifecycleObserver(
      hasActiveTransfer: _hasActiveTransfer,
    );
    WidgetsBinding.instance.addObserver(_keepaliveObserver);
    WidgetsBinding.instance.addObserver(this);
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted) {
        _applyDiscoverability();
        _wireShareIntent();
        _wireWindowsSendIntent();
        // Fire-and-forget startup side-effects. Guard each so a failure (a
        // missing plugin/bridge under widget tests, or a transient runtime
        // error) stays contained instead of surfacing as an unhandled async
        // error — neither is essential to a usable first frame.
        unawaited(
          _maybePromptContextMenu().catchError(
            (Object error) =>
                debugPrint('[app] context-menu prompt skipped: $error'),
          ),
        );
        unawaited(
          ref
              .read(updateControllerProvider.notifier)
              .checkForUpdates()
              .catchError(
                (Object error) =>
                    debugPrint('[app] update check skipped: $error'),
              ),
        );
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
    // Drive the controller state ourselves rather than relying on the draft
    // route's builder to seed via `beginDraft`: when the draft route already
    // sits in the stack, navigating back to it reuses the existing page and its
    // `initState` never re-runs, so the forwarded file would be dropped.
    final notifier = ref.read(sendControllerProvider.notifier);
    if (ref.read(sendControllerProvider) is SendStateDrafting) {
      // Already drafting — the draft screen is up. Fold the items in; only
      // navigate if we've somehow drifted off it. (Guard avoids churning the
      // route on rapid multi-select forwards.)
      notifier.appendDraftItems(files);
      final currentPath = _router.routeInformationProvider.value.uri.path;
      if (currentPath != AppRoutePaths.sendDraft) {
        _router.go(AppRoutePaths.sendDraft, extra: const <SendPickedFile>[]);
      }
    } else {
      // Idle / transferring / a finished result. Start a fresh draft and
      // navigate UNCONDITIONALLY. The transfer route is reached via `push`, and
      // with go_router's optionURLReflectsImperativeAPIs = false the reported
      // path still reads `/send/draft` while that pushed route is on top — so a
      // path-guarded `go` would be skipped and strand us on the transfer route
      // (stuck on "Gathering transfer details…"). `go` rebuilds the stack from
      // the URL and drops the imperatively-pushed route. Empty `extra` so the
      // draft route reuses the state we just set instead of re-seeding.
      notifier.beginDraft(files);
      _router.go(AppRoutePaths.sendDraft, extra: const <SendPickedFile>[]);
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
            style: FilledButton.styleFrom(backgroundColor: kAccentCyanStrong),
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

  // Receiver LAN discovery follows the Settings "discoverable" toggle and the
  // app's foreground state — NOT the current route. As long as the user keeps
  // discovery on and the app is foreground, this device advertises on every
  // screen (Home, Send composer, Settings…), so a sender can always find it.
  // This is what makes PC→phone work over an offline link like USB tethering:
  // the phone stays discoverable instead of going dark the moment it leaves
  // Home.
  //
  // Re-applied (not set once) on settings change, lifecycle resume, navigation,
  // and whenever the receiver service (re)starts. The re-apply matters: the
  // Rust bridge silently no-ops `setDiscoverable` until the receiver service is
  // registered, so a single startup call can race ahead of service init and be
  // lost. Re-asserting on the service-state stream closes that gap. Repeated
  // `setDiscoverable(true)` is cheap — Rust keeps an existing advertisement
  // alive instead of rebuilding it.
  void _applyDiscoverability() {
    // The cable tunnel forces discoverability (even if the Settings default is
    // off, or the app isn't foreground) so a freshly-plugged cable lets either
    // side send without the user toggling anything.
    final wanted =
        (ref.read(settingsControllerProvider).settings.discoverableByDefault &&
            _isForeground) ||
        _usbCableTunnelUp;
    if (wanted != _discoverableEnabled) {
      _discoverableEnabled = wanted;
      debugPrint('[app] receiver discovery ${wanted ? 'enabled' : 'disabled'}');
    }
    unawaited(_receiverService.setDiscoverable(enabled: wanted));
  }

  @override
  void didChangeAppLifecycleState(AppLifecycleState state) {
    final foreground = state == AppLifecycleState.resumed;
    if (foreground == _isForeground) return;
    _isForeground = foreground;
    _applyDiscoverability();
  }

  @override
  void dispose() {
    unawaited(_shareIntentSub?.cancel());
    unawaited(_shareTextSub?.cancel());
    unawaited(_windowsSendSub?.cancel());
    WidgetsBinding.instance.removeObserver(_keepaliveObserver);
    WidgetsBinding.instance.removeObserver(this);
    unawaited(_receiverService.setDiscoverable(enabled: false));
    super.dispose();
  }

  // Surfaces the "update available" dialog when a check (startup or manual)
  // transitions into the `available` phase. This is the single source of truth
  // for the prompt — the Settings/About manual check only shows inline
  // feedback for the up-to-date / error outcomes.
  void _maybeShowUpdateDialog(UpdateState? prev, UpdateState next) {
    final becameAvailable =
        prev?.phase != UpdatePhase.available &&
        next.phase == UpdatePhase.available;
    if (!becameAvailable || next.release == null) return;
    final navContext = _router.routerDelegate.navigatorKey.currentContext;
    if (navContext == null || !navContext.mounted) return;
    unawaited(showUpdateAvailableDialog(navContext, ref, next.release!));
  }

  @override
  Widget build(BuildContext context) {
    ref.listen<UpdateState>(updateControllerProvider, _maybeShowUpdateDialog);

    // Keep LAN discoverability in sync with the Settings toggle, and re-assert
    // it whenever the receiver service (re)starts so a startup race can't leave
    // the device silently un-advertised. See `_applyDiscoverability`.
    ref.listen(
      settingsControllerProvider.select(
        (s) => s.settings.discoverableByDefault,
      ),
      (_, _) => _applyDiscoverability(),
    );
    ref.listen(receiverServiceProvider, (_, _) => _applyDiscoverability());

    // Keep the USB-cable controller alive (single AOA event subscription +
    // auto-establish) and force discoverability while its tunnel is up so
    // either phone can send over the cable.
    ref.listen(usbCableControllerProvider.select((s) => s.tunnelUp), (_, up) {
      _usbCableTunnelUp = up;
      _applyDiscoverability();
    });

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

        // Desktop: alert the user with a native toast when an offer arrives
        // while Wisp isn't focused (hidden in the tray, minimized, or behind
        // another window). Clicking the toast brings the window forward to the
        // confirm prompt. No-op when the window is already focused.
        if (DesktopIntegration.isSupported) {
          final offer = next.offer;
          final sender = offer?.displaySenderName ?? 'Someone';
          final isText = offer?.isTextOffer ?? false;
          // File offers get one-tap Accept / Decline buttons on the toast.
          // Text offers omit them — accepting text needs a Copy-vs-Save choice
          // that only makes sense in the in-app prompt, so the toast just
          // opens the window.
          final notifier = ref.read(transfersServiceProvider.notifier);
          unawaited(
            DesktopIntegration.instance.notifyIncomingTransfer(
              title: isText
                  ? '$sender is sending you text'
                  : '$sender is sending you files',
              body: 'Click to review and accept in Wisp.',
              onAccept: isText ? null : () => unawaited(notifier.acceptOffer()),
              onDecline: isText
                  ? null
                  : () => unawaited(notifier.declineOffer()),
            ),
          );
        }

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

    final themeMode = ref.watch(
      settingsControllerProvider.select((s) => s.settings.themeMode),
    );

    return MaterialApp.router(
      title: 'Wisp',
      debugShowCheckedModeBanner: false,
      theme: buildWispTheme(Brightness.light),
      darkTheme: buildWispTheme(Brightness.dark),
      themeMode: themeMode,
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
