import 'dart:async';
import 'dart:io';

import 'package:flutter/foundation.dart';

import '../../../features/receive/application/pairing_cache.dart';
import '../../../features/receive/application/state.dart';
import '../../../features/transfers/application/state.dart';
import '../../../src/rust/api/lan.dart' as rust_lan;
import '../../../src/rust/api/receiver.dart' as rust_receiver;
import '../../android_media_store.dart';
import '../rendezvous_defaults.dart';
import 'mapper.dart';
import 'source.dart';

typedef ReceiverPairingStreamFactory =
    Stream<rust_receiver.ReceiverPairingState> Function({
      String? serverUrl,
      required String downloadRoot,
      required String deviceName,
      required String deviceType,
    });

typedef ReceiverTransferStreamFactory =
    Stream<rust_receiver.ReceiverTransferEvent> Function({
      String? serverUrl,
      required String downloadRoot,
      required String deviceName,
      required String deviceType,
    });

class RustReceiverServiceSource implements ReceiverServiceSource {
  RustReceiverServiceSource({
    required this.deviceName,
    required this.downloadRoot,
    this.serverUrl,
    this.androidReceiveCacheDir,
    this.pairingCache,
    ReceiverPairingStreamFactory? pairingStreamFactory,
    ReceiverTransferStreamFactory? transferStreamFactory,
  }) : _pairingStreamFactory =
           pairingStreamFactory ?? rust_receiver.watchReceiverPairing,
       _transferStreamFactory =
           transferStreamFactory ??
           rust_receiver.startReceiverTransferListener {
    final seeded = pairingCache?.loadIfFresh(
      identity: PairingCacheRepository.buildIdentity(
        deviceName: deviceName,
        serverUrl: serverUrl,
      ),
    );
    if (seeded != null && seeded.isAvailable) {
      _currentState = ReceiverServiceState.ready(
        code: seeded.normalizedCode,
        expiresAt: seeded.expiresAt,
      );
      debugPrint(
        '[receiver] seeded ready state from pairing cache code="${seeded.formattedCode}"',
      );
    }
  }

  String deviceName;
  String downloadRoot;
  String? serverUrl;

  /// On Android, Rust writes received files here instead of the user-configured
  /// [downloadRoot].  After each transfer completes, files are moved to the
  /// public Downloads/Wisp/ folder via MediaStore.
  final String? androidReceiveCacheDir;

  /// Optional persistent cache for the most recent successful pairing code.
  /// When supplied, the source seeds [currentState] from it on construction
  /// so the UI can show "Ready" immediately on cold start instead of waiting
  /// for the network roundtrip to the rendezvous server.
  final PairingCacheRepository? pairingCache;

  /// When the user has chosen a save folder via [AndroidMediaStore.pickSaveFolder],
  /// this holds the persisted SAF tree URI.  If null, files are saved to the
  /// default `Downloads/Wisp/` via MediaStore.
  String? androidSaveUri;

  /// Android only: per-file `content://` URIs from the most recent transfer's
  /// post-download save step, keyed by the transfer-relative path (forward
  /// slashes). Populated by [_saveFilesToMediaStore] as each file lands; read
  /// by [savedReceivedFileUri] so the finish screen can open an individual file
  /// at its exact final URI (immune to MediaStore collision renames).
  final Map<String, String> _savedReceivedUris = {};

  /// The `content://` URI a received file was saved to, or `null` if it hasn't
  /// finished saving yet (the Android save runs in the background after the
  /// `completed` event) or wasn't part of the last transfer. [relativePath] is
  /// the transfer-relative path; separators are normalised to forward slashes.
  String? savedReceivedFileUri(String relativePath) =>
      _savedReceivedUris[relativePath.replaceAll('\\', '/')];

  final ReceiverPairingStreamFactory _pairingStreamFactory;
  final ReceiverTransferStreamFactory _transferStreamFactory;
  final StreamController<ReceiverServiceState> _stateController =
      StreamController<ReceiverServiceState>.broadcast(sync: true);
  final StreamController<rust_receiver.ReceiverTransferEvent>
  _transferController =
      StreamController<rust_receiver.ReceiverTransferEvent>.broadcast(
        sync: true,
      );

  StreamSubscription<rust_receiver.ReceiverPairingState>? _pairingSubscription;
  StreamSubscription<rust_receiver.ReceiverTransferEvent>?
  _transferSubscription;
  int _configGeneration = 0;
  ReceiverServiceState _currentState = const ReceiverServiceState.registering();

  @override
  ReceiverServiceState get currentState => _currentState;

  @override
  Stream<ReceiverServiceState> watchState() {
    _ensurePairingSubscription();
    return _stateController.stream;
  }

  @override
  Stream<rust_receiver.ReceiverTransferEvent> watchIncomingTransfers() {
    _ensureTransferSubscription();
    return _transferController.stream;
  }

  @override
  Future<void> setup({String? serverUrl}) async {
    debugPrint(
      '[receiver] setup request '
      'device="$deviceName" '
      'downloadRoot="$downloadRoot" '
      'serverUrl="${serverUrl ?? _resolvedServerUrl}"',
    );
    await rust_receiver.registerReceiver(
      serverUrl: serverUrl ?? _resolvedServerUrl,
      deviceName: deviceName,
    );
    debugPrint('[receiver] setup complete');
  }

  @override
  Future<void> ensureRegistered({String? serverUrl}) async {
    debugPrint(
      '[receiver] ensureRegistered request '
      'device="$deviceName" '
      'serverUrl="${serverUrl ?? _resolvedServerUrl}"',
    );
    await rust_receiver.ensureReceiverRegistration(
      serverUrl: serverUrl ?? _resolvedServerUrl,
      deviceName: deviceName,
    );
    debugPrint('[receiver] ensureRegistered complete');
  }

  @override
  Future<void> updateIdentity({
    required String deviceName,
    required String downloadRoot,
    String? serverUrl,
  }) async {
    final previousDeviceName = this.deviceName;
    final previousDownloadRoot = this.downloadRoot;
    final previousServerUrl = this.serverUrl;
    this.deviceName = deviceName;
    // On Android the receive cache dir is fixed; ignore the user-configured
    // downloadRoot so Rust always writes to the temp cache.
    if (androidReceiveCacheDir == null) {
      this.downloadRoot = downloadRoot;
    } else {
      // Track the SAF URI (or clear it) so the post-transfer save step uses it.
      androidSaveUri = AndroidMediaStore.isSafUri(downloadRoot)
          ? downloadRoot
          : null;
    }
    this.serverUrl = serverUrl;
    debugPrint(
      '[receiver] updateIdentity '
      'from device="$previousDeviceName" downloadRoot="$previousDownloadRoot" '
      'serverUrl="${previousServerUrl ?? _resolvedServerUrl}" '
      'to device="$deviceName" downloadRoot="$downloadRoot" '
      'serverUrl="${serverUrl ?? _resolvedServerUrl}"',
    );
    final identityChanged =
        previousDeviceName != deviceName || previousServerUrl != serverUrl;
    if (identityChanged) {
      // Old code is no longer valid against the new identity — invalidate the
      // cache and force the visual state back to Registering until the new
      // pairing stream emits a fresh code.
      final cache = pairingCache;
      if (cache != null) {
        unawaited(cache.clear());
      }
    }

    final generation = ++_configGeneration;

    if (_pairingSubscription != null) {
      debugPrint('[receiver] restarting pairing stream');
      _restartPairingSubscription(
        generation: generation,
        forceReset: identityChanged,
      );
    }
    if (_transferSubscription != null) {
      debugPrint('[receiver] restarting transfer stream');
      _restartTransferSubscription(generation: generation);
    }
  }

  @override
  Future<void> setDiscoverable({required bool enabled}) {
    debugPrint('[receiver] discoverable ${enabled ? 'enabled' : 'disabled'}');
    return rust_receiver.setReceiverDiscoverable(enabled: enabled);
  }

  @override
  Future<void> respondToOffer({required bool accept}) {
    return rust_receiver.respondToReceiverOffer(accept: accept);
  }

  @override
  Future<SavedTextLocation> saveInlineText({
    required String suggestedName,
    required String contents,
  }) async {
    // Rust writes the .txt to its download root: the user's real folder on
    // desktop, the temp receive cache on Android. The returned path's basename
    // is the actual name on disk (post sanitize + de-dup), so use it verbatim.
    final written = await rust_receiver.saveTextFile(
      suggestedName: suggestedName,
      contents: contents,
    );
    final fileName = written.split(RegExp(r'[\\/]')).last;
    // Desktop/iOS: the download root is already the user-visible folder.
    if (!Platform.isAndroid || androidReceiveCacheDir == null) {
      final sepIdx = written.lastIndexOf(RegExp(r'[\\/]'));
      final folder = sepIdx > 0 ? written.substring(0, sepIdx) : written;
      return SavedTextLocation(fileName: fileName, folderLabel: folder);
    }
    // Android: the .txt only reached the temp cache. Export it to the chosen
    // SAF folder / Downloads via MediaStore — the same path completed file
    // transfers take — so it actually lands somewhere the user can find.
    final safUri = androidSaveUri;
    final saved = safUri != null
        ? await AndroidMediaStore.saveToSafUri(written, fileName, safUri)
        : await AndroidMediaStore.saveToDownloads(written, fileName);
    if (saved != null) {
      // Drop the temp copy so the cache doesn't accumulate stray .txt files.
      try {
        await File(written).delete();
      } catch (_) {}
    }
    return SavedTextLocation(
      fileName: fileName,
      folderLabel: AndroidMediaStore.readableDestinationLabel(safUri),
    );
  }

  @override
  Future<void> cancelTransfer() {
    return rust_receiver.cancelReceiverTransfer();
  }

  @override
  Future<List<NearbyReceiver>> scanNearby({required Duration timeout}) async {
    final peers = await rust_lan.scanNearbyReceivers(
      timeoutSecs: BigInt.from(timeout.inSeconds.clamp(1, 60).toInt()),
    );
    return peers
        .map(
          (peer) => NearbyReceiver(
            fullname: peer.fullname,
            label: peer.label,
            deviceType: peer.deviceType,
            code: peer.code,
            ticket: peer.ticket,
            endpointId: peer.endpointId,
            overUsb: peer.overUsb,
          ),
        )
        .toList(growable: false);
  }

  @override
  Future<void> shutdown() async {
    debugPrint('[receiver] shutdown');
    unawaited(_pairingSubscription?.cancel());
    unawaited(_transferSubscription?.cancel());
    _pairingSubscription = null;
    _transferSubscription = null;
    await rust_receiver.setReceiverDiscoverable(enabled: false);
  }

  void _restartPairingSubscription({
    required int generation,
    bool forceReset = false,
  }) {
    final oldSubscription = _pairingSubscription;
    debugPrint('[receiver] pairing stream generation=$generation start');
    _pairingSubscription = null;
    // Preserve a cached/live Ready code across reconnects; only flip to the
    // visual Registering state if there's nothing usable to show or the caller
    // explicitly requested a reset (e.g. identity changed).
    if (forceReset || !_currentState.pairingCode.isAvailable) {
      _currentState = const ReceiverServiceState.registering();
      if (!_stateController.isClosed) {
        _stateController.add(_currentState);
      }
    }
    final stream = _pairingStreamFactory(
      serverUrl: _resolvedServerUrl,
      downloadRoot: downloadRoot,
      deviceName: deviceName,
      deviceType: _deviceType,
    );
    _pairingSubscription = stream.listen(
      (pairing) {
        if (generation != _configGeneration) {
          debugPrint(
            '[receiver] pairing stream generation=$generation ignored stale event',
          );
          return;
        }
        final nextState = mapReceiverPairingState(pairing);
        // Don't downgrade a usable cached/live code to Unavailable on transient
        // empty events from the stream — keep the existing Ready state and
        // wait for a real refresh. Only accept transitions that actually carry
        // a code, plus terminal failures via onError.
        if (!nextState.pairingCode.isAvailable &&
            _currentState.pairingCode.isAvailable) {
          debugPrint(
            '[receiver] pairing stream generation=$generation ignored empty event '
            '(keeping cached ready code)',
          );
          return;
        }
        _currentState = nextState;
        if (!_stateController.isClosed) {
          _stateController.add(nextState);
        }
        if (nextState.pairingCode.isAvailable) {
          final cache = pairingCache;
          if (cache != null) {
            unawaited(
              cache.save(
                identity: PairingCacheRepository.buildIdentity(
                  deviceName: deviceName,
                  serverUrl: serverUrl,
                ),
                code: nextState.pairingCode.normalizedCode,
                expiresAt: nextState.pairingCode.expiresAt,
              ),
            );
          }
        }
      },
      onError: (_) {
        if (generation != _configGeneration) {
          return;
        }
        debugPrint('[receiver] pairing stream generation=$generation error');
        // Same anti-flicker rule: don't blow away a cached Ready code just
        // because the stream errored — the next reconnect will refresh it.
        if (_currentState.pairingCode.isAvailable) {
          return;
        }
        _currentState = const ReceiverServiceState.unavailable();
        if (!_stateController.isClosed) {
          _stateController.add(_currentState);
        }
      },
    );
    unawaited(oldSubscription?.cancel());
  }

  void _restartTransferSubscription({required int generation}) {
    final oldSubscription = _transferSubscription;
    debugPrint('[receiver] transfer stream generation=$generation start');
    _transferSubscription = null;
    final stream = _transferStreamFactory(
      serverUrl: _resolvedServerUrl,
      downloadRoot: downloadRoot,
      deviceName: deviceName,
      deviceType: _deviceType,
    );
    _transferSubscription = stream.listen(
      (event) {
        if (generation != _configGeneration) {
          debugPrint(
            '[receiver] transfer stream generation=$generation ignored stale event',
          );
          return;
        }
        // Emit the event to UI first so the completion state is shown
        // immediately, then run the Android MediaStore save in the background.
        if (!_transferController.isClosed) {
          _transferController.add(event);
        }
        if (Platform.isAndroid &&
            androidReceiveCacheDir != null &&
            event.phase == rust_receiver.ReceiverTransferPhase.completed) {
          unawaited(_saveFilesToMediaStore(event, androidReceiveCacheDir!));
        }
      },
      onError: (error) {
        if (generation != _configGeneration) {
          return;
        }
        debugPrint(
          '[receiver] transfer stream generation=$generation error: $error',
        );
        _emitFailedState(error);
      },
    );
    unawaited(oldSubscription?.cancel());
  }

  /// Emit a [ReceiverServiceState.failedWith] carrying a user-visible message
  /// + suggested remediation action so the shell can render a banner. Without
  /// this the underlying stream error is logged but the UI keeps showing the
  /// idle "waiting for a code" state, which on a fresh install looked
  /// identical to a working receiver — the user had no way to learn that
  /// anything was wrong.
  void _emitFailedState(Object error) {
    if (_stateController.isClosed) return;
    final message = _humanizeError(error);
    final action = _categorizeAction(message);
    _currentState = ReceiverServiceState.failedWith(
      ReceiverServiceError(message: message, action: action),
    );
    _stateController.add(_currentState);
  }

  String _humanizeError(Object error) {
    final raw = error.toString();
    // Trim Dart's `Instance of '…'` boilerplate when the FFI side hands us a
    // structured error that lacks a useful toString. Falls through to the raw
    // string when the message is meaningful.
    if (raw.startsWith("Instance of '")) {
      return 'Receiver service stopped responding. Run the connection test '
          'to find the cause.';
    }
    return raw;
  }

  ReceiverErrorAction _categorizeAction(String message) {
    final lower = message.toLowerCase();
    if (lower.contains('folder') ||
        lower.contains('directory') ||
        lower.contains('save location') ||
        lower.contains('download root') ||
        lower.contains('permission denied') ||
        lower.contains('read-only')) {
      return ReceiverErrorAction.openSettings;
    }
    return ReceiverErrorAction.openConnectionTest;
  }

  /// Moves all received files from [cacheRoot] to the final destination:
  /// - [androidSaveUri] (user-picked SAF folder) if set, or
  /// - default `Downloads/Wisp/` via MediaStore.
  /// Then deletes the temp cache.
  Future<void> _saveFilesToMediaStore(
    rust_receiver.ReceiverTransferEvent event,
    String cacheRoot,
  ) async {
    final safUri = androidSaveUri;
    // The completed event has files: [] but plan.files has all paths.
    final planFiles = event.plan?.files ?? [];
    // Fresh transfer — drop any URIs cached from a previous one so the finish
    // screen's open buttons can't resolve to a stale file.
    _savedReceivedUris.clear();
    debugPrint(
      '[receiver] Android post-transfer: saving ${planFiles.length} file(s) '
      '${safUri != null ? "to SAF folder" : "to MediaStore Downloads/Wisp"}',
    );
    for (final file in planFiles) {
      final relativePath = file.path.replaceAll('\\', '/');
      final srcPath = '$cacheRoot/$relativePath';
      final String? saved;
      if (safUri != null) {
        saved = await AndroidMediaStore.saveToSafUri(
          srcPath,
          relativePath,
          safUri,
        );
      } else {
        saved = await AndroidMediaStore.saveToDownloads(srcPath, relativePath);
      }
      if (saved != null) {
        // Record the exact final URI so an individual file can be opened from
        // the finish screen without re-deriving (and possibly mis-guessing) it.
        _savedReceivedUris[relativePath] = saved;
        debugPrint('[receiver] Android saved: $relativePath → $saved');
      } else {
        debugPrint('[receiver] Android save failed for: $relativePath');
      }
    }
    await AndroidMediaStore.cleanupReceiveCache(cacheRoot);
    debugPrint('[receiver] Android post-transfer: done');
  }

  void _ensurePairingSubscription() {
    if (_pairingSubscription != null) {
      return;
    }
    _restartPairingSubscription(generation: _configGeneration);
  }

  void _ensureTransferSubscription() {
    if (_transferSubscription != null) {
      return;
    }
    _restartTransferSubscription(generation: _configGeneration);
  }

  String get _resolvedServerUrl => serverUrl ?? defaultRendezvousUrl;

  static String get _deviceType => switch (defaultTargetPlatform) {
    TargetPlatform.android || TargetPlatform.iOS => 'phone',
    TargetPlatform.macOS ||
    TargetPlatform.windows ||
    TargetPlatform.linux => 'laptop',
    TargetPlatform.fuchsia => 'laptop',
  };
}
