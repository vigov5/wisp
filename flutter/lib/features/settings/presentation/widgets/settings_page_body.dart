import 'dart:async';
import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../../../app/app_router.dart';
import '../../../../platform/android_media_store.dart';
import '../../../../platform/windows_context_menu.dart';
import '../../../../src/rust/api/simple.dart' as rust_simple;
import '../../../../theme/wisp_theme.dart';
import '../../../../platform/rust/rendezvous_defaults.dart';
import '../../../transfers/application/pubkey_visual.dart';
import '../../../update/application/update_providers.dart';
import '../../../update/domain/update_status.dart';
import '../../application/controller.dart';
import '../../settings_providers.dart';
import 'reliability_settings_section.dart';
import 'settings_error_banner.dart';
import 'settings_download_root_field.dart';
import 'settings_section_field.dart';
import 'settings_path_display.dart';
import 'settings_storage_section.dart';
import 'settings_toggle_field.dart';

class SettingsPageBody extends ConsumerStatefulWidget {
  const SettingsPageBody({super.key});

  @override
  ConsumerState<SettingsPageBody> createState() => _SettingsPageBodyState();
}

class _SettingsPageBodyState extends ConsumerState<SettingsPageBody> {
  late final TextEditingController _deviceNameController;
  late final TextEditingController _downloadRootController;
  late final TextEditingController _serverUrlController;
  late String _initialDeviceName;
  late String _initialDownloadRoot;
  late String _downloadRootValue;
  late String _initialServerUrl;
  late bool _initialDiscoverable;
  bool _discoverable = true;
  late bool _initialSkipClipboardConfirm;
  bool _skipClipboardConfirm = false;
  bool _saving = false;
  String _endpointId = '';
  // Windows "Send via Wisp" context-menu state. The registry is the source of
  // truth, so this mirrors the live registration status and applies changes
  // immediately (independent of the Save button / _isDirty).
  bool _contextMenuEnabled = false;
  bool _contextMenuBusy = false;
  // Update-checker state. The toggle mirrors the persisted preference; the flag
  // gates inline snackbar feedback to user-initiated checks only.
  bool _checkOnStartup = true;
  bool _awaitingManualUpdateResult = false;

  @override
  void initState() {
    super.initState();
    final settings = ref.read(settingsControllerProvider).settings;
    _initialDeviceName = settings.deviceName;
    _initialDownloadRoot = settings.downloadRoot;
    _downloadRootValue = settings.downloadRoot;
    _initialServerUrl = settings.discoveryServerUrl ?? '';
    _initialDiscoverable = settings.discoverableByDefault;
    _discoverable = _initialDiscoverable;
    _initialSkipClipboardConfirm = settings.skipClipboardConfirm;
    _skipClipboardConfirm = _initialSkipClipboardConfirm;
    _deviceNameController = TextEditingController(text: _initialDeviceName);
    _downloadRootController = TextEditingController(
      text: _downloadRootDisplayText(_initialDownloadRoot),
    );
    _serverUrlController = TextEditingController(text: _initialServerUrl);
    _deviceNameController.addListener(_onFieldChanged);
    _serverUrlController.addListener(_onFieldChanged);
    try {
      _endpointId = rust_simple.currentEndpointId();
    } catch (_) {
      // Bridge not yet initialized — stays empty, badge hides itself.
    }
    if (Platform.isWindows) {
      unawaited(_refreshContextMenuStatus());
    }
    _checkOnStartup = ref
        .read(updateControllerProvider.notifier)
        .checkOnStartup();
  }

  void _checkForUpdates() {
    _awaitingManualUpdateResult = true;
    ref.read(updateControllerProvider.notifier).checkForUpdates(manual: true);
  }

  // Surfaces inline feedback for a user-initiated check. The "available"
  // outcome is handled by the global dialog in WispApp, so here we only report
  // the up-to-date / error results.
  void _onUpdateState(UpdateState? prev, UpdateState next) {
    if (!_awaitingManualUpdateResult) return;
    if (next.phase == UpdatePhase.upToDate) {
      _awaitingManualUpdateResult = false;
      _showUpdateSnack("You're on the latest version.");
    } else if (next.phase == UpdatePhase.error) {
      _awaitingManualUpdateResult = false;
      _showUpdateSnack(next.errorMessage ?? 'Could not check for updates.');
    } else if (next.phase == UpdatePhase.available) {
      _awaitingManualUpdateResult = false;
    }
  }

  void _showUpdateSnack(String message) {
    if (!mounted) return;
    ScaffoldMessenger.of(
      context,
    ).showSnackBar(SnackBar(content: Text(message)));
  }

  // Reads the live context-menu registration status from the registry and
  // updates the toggle to match reality (handles manual registry edits or a
  // failed write). Windows only.
  Future<void> _refreshContextMenuStatus() async {
    final enabled = await WindowsContextMenu.isRegistered();
    if (mounted) {
      setState(() => _contextMenuEnabled = enabled);
    }
  }

  // Applies the context-menu toggle immediately, then re-reads the registry so
  // the displayed value reflects the actual outcome.
  Future<void> _toggleContextMenu(bool value) async {
    if (_contextMenuBusy) return;
    setState(() => _contextMenuBusy = true);
    try {
      if (value) {
        await WindowsContextMenu.register();
      } else {
        await WindowsContextMenu.unregister();
      }
      await _refreshContextMenuStatus();
    } finally {
      if (mounted) {
        setState(() => _contextMenuBusy = false);
      }
    }
  }

  @override
  void dispose() {
    _deviceNameController.removeListener(_onFieldChanged);
    _serverUrlController.removeListener(_onFieldChanged);
    _deviceNameController.dispose();
    _downloadRootController.dispose();
    _serverUrlController.dispose();
    super.dispose();
  }

  bool get _isDirty {
    return _deviceNameController.text.trim() != _initialDeviceName.trim() ||
        _downloadRootValue.trim() != _initialDownloadRoot.trim() ||
        _serverUrlController.text.trim() != _initialServerUrl.trim() ||
        _discoverable != _initialDiscoverable ||
        _skipClipboardConfirm != _initialSkipClipboardConfirm;
  }

  void _onFieldChanged() {
    if (mounted) {
      setState(() {});
    }
  }

  Future<void> _handleBack() async {
    if (_isDirty) {
      final shouldDiscard = await _confirmDiscardChanges();
      if (!shouldDiscard || !mounted) {
        return;
      }
    }

    if (mounted) {
      Navigator.of(context).pop();
    }
  }

  Future<bool> _confirmDiscardChanges() async {
    final result = await showDialog<bool>(
      context: context,
      builder: (context) {
        return AlertDialog(
          title: const Text('Discard changes?'),
          content: const Text(
            'You have unsaved changes. Leave this page without saving them?',
          ),
          actions: [
            TextButton(
              onPressed: () => Navigator.of(context).pop(false),
              child: const Text('Stay'),
            ),
            FilledButton(
              onPressed: () => Navigator.of(context).pop(true),
              style: FilledButton.styleFrom(
                backgroundColor: kAccentCyanStrong,
              ),
              child: const Text('Discard'),
            ),
          ],
        );
      },
    );
    return result ?? false;
  }

  Future<void> _saveSettings() async {
    if (_saving || !_isDirty) {
      return;
    }

    setState(() => _saving = true);
    await ref
        .read(settingsControllerProvider.notifier)
        .saveSettings(
          deviceName: _deviceNameController.text,
          downloadRoot: _downloadRootValue,
          serverUrl: _serverUrlController.text,
          discoverableByDefault: _discoverable,
          skipClipboardConfirm: _skipClipboardConfirm,
        );

    if (!mounted) {
      return;
    }

    final state = ref.read(settingsControllerProvider);
    setState(() {
      _saving = false;
      if (state.errorMessage == null) {
        _initialDeviceName = state.settings.deviceName;
        _initialDownloadRoot = state.settings.downloadRoot;
        _downloadRootValue = state.settings.downloadRoot;
        _initialServerUrl = state.settings.discoveryServerUrl ?? '';
        _initialDiscoverable = state.settings.discoverableByDefault;
        _initialSkipClipboardConfirm = state.settings.skipClipboardConfirm;
        _skipClipboardConfirm = _initialSkipClipboardConfirm;
        _deviceNameController.text = _initialDeviceName;
        _downloadRootController.text = _downloadRootDisplayText(
          _initialDownloadRoot,
        );
        _serverUrlController.text = _initialServerUrl;
        _discoverable = _initialDiscoverable;
      }
    });

    if (state.errorMessage != null && mounted) {
      ScaffoldMessenger.of(
        context,
      ).showSnackBar(SnackBar(content: Text(state.errorMessage!)));
    } else if (mounted) {
      ScaffoldMessenger.of(
        context,
      ).showSnackBar(const SnackBar(content: Text('Settings saved')));
    }
  }

  /// Returns a user-friendly display string for [root].
  /// On Android, `content://` SAF URIs are shown as just the path portion.
  /// On Android with no SAF folder chosen, files actually land in the
  /// public `Download/Wisp/` folder via MediaStore (not the app-private
  /// path the settings happen to store internally), so show that — the
  /// previous app-private path was misleading because Rust writes to a
  /// temp cache and then `_saveFilesToMediaStore` redirects everything to
  /// `Download/Wisp/` anyway.
  String _downloadRootDisplayText(String root) {
    if (Platform.isAndroid) {
      // Both the SAF-folder and MediaStore-default cases resolve to a friendly
      // label (the actual destination is always Download/Wisp or the chosen
      // SAF folder, never the app-private path the settings store internally).
      return AndroidMediaStore.readableDestinationLabel(root);
    }
    return formatSettingsDownloadRootForDisplay(root);
  }

  /// Helper text shown below the download-root field on Android when the
  /// default app-private path is in use. Explains that the folder is created
  /// on demand and is not the same as the system Downloads folder.
  String? _androidDownloadRootHint() {
    if (!Platform.isAndroid) return null;
    if (AndroidMediaStore.isSafUri(_downloadRootValue)) return null;
    return 'Received files appear in your device Download folder under '
        'Wisp/. Tap Choose to save into a different folder instead.';
  }

  Future<void> _pickDownloadRoot() async {
    if (Platform.isAndroid) {
      final folder = await AndroidMediaStore.pickSaveFolder();
      if (folder == null || !mounted) return;
      setState(() {
        _downloadRootValue = folder.uri;
        _downloadRootController.text = folder.displayName;
      });
      return;
    }
    final currentRoot = _downloadRootValue.trim();
    final selected = await ref
        .read(storageAccessSourceProvider)
        .pickDirectory(
          initialDirectory: currentRoot.isEmpty ? null : currentRoot,
        );

    if (selected == null || selected.trim().isEmpty) {
      return;
    }

    setState(() {
      _downloadRootValue = selected.trim();
      _downloadRootController.text = formatSettingsDownloadRootForDisplay(
        _downloadRootValue,
      );
    });
  }

  @override
  Widget build(BuildContext context) {
    final state = ref.watch(settingsControllerProvider);
    final saving = _saving || state.isSaving;
    ref.listen<UpdateState>(updateControllerProvider, _onUpdateState);
    final checkingForUpdates =
        ref.watch(updateControllerProvider).phase == UpdatePhase.checking;

    return PopScope(
      canPop: !_isDirty,
      onPopInvokedWithResult: (didPop, _) {
        if (!didPop) {
          unawaited(_handleBack());
        }
      },
      child: Scaffold(
        backgroundColor: kBg,
        body: SafeArea(
          child: Padding(
            padding: const EdgeInsets.fromLTRB(24, 24, 24, 16),
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                Row(
                  children: [
                    IconButton(
                      onPressed: _handleBack,
                      icon: const Icon(Icons.arrow_back_rounded),
                      tooltip: 'Back',
                    ),
                    const SizedBox(width: 4),
                    Text(
                      'Settings',
                      style: wispSans(
                        fontSize: 22,
                        fontWeight: FontWeight.w700,
                        color: kInk,
                        letterSpacing: -0.35,
                      ),
                    ),
                  ],
                ),
                const SizedBox(height: 8),
                Expanded(
                  child: SingleChildScrollView(
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.stretch,
                      children: [
                        if (state.errorMessage != null) ...[
                          SettingsErrorBanner(message: state.errorMessage!),
                          const SizedBox(height: 16),
                        ],
                        SettingsSectionField(
                          label: 'Device name',
                          child: TextField(
                            controller: _deviceNameController,
                            decoration: const InputDecoration(
                              hintText: 'Alex\'s MacBook',
                            ),
                          ),
                        ),
                        if (_endpointId.isNotEmpty) ...[
                          const SizedBox(height: 8),
                          _IdentityBadgeRow(endpointId: _endpointId),
                        ],
                        const SizedBox(height: 22),
                        SettingsSectionField(
                          label: 'Save received files to',
                          child: Column(
                            crossAxisAlignment: CrossAxisAlignment.stretch,
                            children: [
                              SettingsDownloadRootField(
                                controller: _downloadRootController,
                                onChoose: _pickDownloadRoot,
                              ),
                              if (_androidDownloadRootHint() != null) ...[
                                const SizedBox(height: 6),
                                Text(
                                  _androidDownloadRootHint()!,
                                  style: wispSans(
                                    fontSize: 11.5,
                                    fontWeight: FontWeight.w400,
                                    color: kMuted,
                                    height: 1.45,
                                  ),
                                ),
                              ],
                            ],
                          ),
                        ),
                        const SizedBox(height: 22),
                        SettingsToggleField(
                          title: 'Nearby discoverability',
                          subtitle:
                              'Make this device visible to others on your network.',
                          value: _discoverable,
                          onChanged: (value) {
                            setState(() => _discoverable = value);
                          },
                        ),
                        const SizedBox(height: 18),
                        SettingsToggleField(
                          title: 'Skip clipboard confirmation',
                          subtitle:
                              'When on, "Share clipboard" sends right away. '
                              'When off, you confirm the text first.',
                          value: _skipClipboardConfirm,
                          onChanged: (value) {
                            setState(() => _skipClipboardConfirm = value);
                          },
                        ),
                        const SizedBox(height: 18),
                        if (Platform.isWindows) ...[
                          SettingsToggleField(
                            title: 'Windows right-click menu',
                            subtitle:
                                "Show 'Send via Wisp' on files and folders "
                                'in File Explorer.',
                            value: _contextMenuEnabled,
                            onChanged: (value) =>
                                unawaited(_toggleContextMenu(value)),
                          ),
                          const SizedBox(height: 18),
                        ],
                        SettingsSectionField(
                          label: 'Connection Test',
                          child: InkWell(
                            onTap: () => context.pushConnectionTest(),
                            borderRadius: BorderRadius.circular(12),
                            child: Container(
                              padding: const EdgeInsets.symmetric(
                                horizontal: 14,
                                vertical: 12,
                              ),
                              decoration: BoxDecoration(
                                color: kSurface,
                                borderRadius: BorderRadius.circular(12),
                                border: Border.all(color: kBorder),
                              ),
                              child: Row(
                                children: [
                                  Expanded(
                                    child: Text(
                                      'Self-diagnose pairing, LAN, and permissions',
                                      style: wispSans(
                                        fontSize: 13,
                                        color: kInk,
                                      ),
                                    ),
                                  ),
                                  const Icon(
                                    Icons.chevron_right_rounded,
                                    color: kMuted,
                                  ),
                                ],
                              ),
                            ),
                          ),
                        ),
                        const SizedBox(height: 18),
                        const ReliabilitySettingsSection(),
                        const SizedBox(height: 18),
                        SettingsSectionField(
                          label: 'Saved devices',
                          child: InkWell(
                            onTap: () => context.pushSavedDevices(),
                            borderRadius: BorderRadius.circular(12),
                            child: Container(
                              padding: const EdgeInsets.symmetric(
                                horizontal: 14,
                                vertical: 12,
                              ),
                              decoration: BoxDecoration(
                                color: kSurface,
                                borderRadius: BorderRadius.circular(12),
                                border: Border.all(color: kBorder),
                              ),
                              child: Row(
                                children: [
                                  Expanded(
                                    child: Text(
                                      'Manage devices used for fast resends',
                                      style: wispSans(
                                        fontSize: 13,
                                        color: kInk,
                                      ),
                                    ),
                                  ),
                                  const Icon(
                                    Icons.chevron_right_rounded,
                                    color: kMuted,
                                  ),
                                ],
                              ),
                            ),
                          ),
                        ),
                        const SizedBox(height: 18),
                        SettingsStorageSection(
                          downloadRoot: _downloadRootValue,
                        ),
                        const SizedBox(height: 28),
                        const Divider(color: kBorder, height: 1),
                        const SizedBox(height: 18),
                        Text(
                          'Advanced',
                          style: wispSans(
                            fontSize: 18,
                            fontWeight: FontWeight.w700,
                            color: kInk,
                            letterSpacing: -0.35,
                          ),
                        ),
                        const SizedBox(height: 8),
                        Text(
                          'Only needed for self-hosted setups.',
                          style: wispSans(
                            fontSize: 12.5,
                            fontWeight: FontWeight.w400,
                            color: kMuted,
                            height: 1.45,
                          ),
                        ),
                        const SizedBox(height: 18),
                        SettingsSectionField(
                          label: 'Discovery Server',
                          child: TextField(
                            controller: _serverUrlController,
                            decoration: const InputDecoration(
                              hintText: defaultRendezvousUrl,
                            ),
                          ),
                        ),
                        const SizedBox(height: 28),
                        const Divider(color: kBorder, height: 1),
                        const SizedBox(height: 18),
                        SettingsSectionField(
                          label: 'About',
                          child: Column(
                            children: [
                              InkWell(
                                onTap: checkingForUpdates
                                    ? null
                                    : _checkForUpdates,
                                borderRadius: BorderRadius.circular(12),
                                child: Container(
                                  padding: const EdgeInsets.symmetric(
                                    horizontal: 14,
                                    vertical: 12,
                                  ),
                                  decoration: BoxDecoration(
                                    color: kSurface,
                                    borderRadius: BorderRadius.circular(12),
                                    border: Border.all(color: kBorder),
                                  ),
                                  child: Row(
                                    children: [
                                      Expanded(
                                        child: Text(
                                          checkingForUpdates
                                              ? 'Checking for updates…'
                                              : 'Check for updates',
                                          style: wispSans(
                                            fontSize: 13,
                                            color: kInk,
                                          ),
                                        ),
                                      ),
                                      if (checkingForUpdates)
                                        const SizedBox(
                                          width: 16,
                                          height: 16,
                                          child: CircularProgressIndicator(
                                            strokeWidth: 2,
                                            color: kAccentCyanStrong,
                                          ),
                                        )
                                      else
                                        const Icon(
                                          Icons.refresh_rounded,
                                          color: kMuted,
                                          size: 20,
                                        ),
                                    ],
                                  ),
                                ),
                              ),
                              const SizedBox(height: 10),
                              Container(
                                padding: const EdgeInsets.symmetric(
                                  horizontal: 14,
                                  vertical: 10,
                                ),
                                decoration: BoxDecoration(
                                  color: kSurface,
                                  borderRadius: BorderRadius.circular(12),
                                  border: Border.all(color: kBorder),
                                ),
                                child: SettingsToggleField(
                                  title: 'Check for updates on startup',
                                  subtitle:
                                      'Notify me when a new version is available',
                                  value: _checkOnStartup,
                                  onChanged: (value) {
                                    setState(() => _checkOnStartup = value);
                                    ref
                                        .read(updateControllerProvider.notifier)
                                        .setCheckOnStartup(value);
                                  },
                                ),
                              ),
                              const SizedBox(height: 10),
                              InkWell(
                                onTap: () => context.pushAbout(),
                                borderRadius: BorderRadius.circular(12),
                                child: Container(
                                  padding: const EdgeInsets.symmetric(
                                    horizontal: 14,
                                    vertical: 12,
                                  ),
                                  decoration: BoxDecoration(
                                    color: kSurface,
                                    borderRadius: BorderRadius.circular(12),
                                    border: Border.all(color: kBorder),
                                  ),
                                  child: Row(
                                    children: [
                                      Expanded(
                                        child: Text(
                                          'Version, source code, and licenses',
                                          style: wispSans(
                                            fontSize: 13,
                                            color: kInk,
                                          ),
                                        ),
                                      ),
                                      const Icon(
                                        Icons.chevron_right_rounded,
                                        color: kMuted,
                                      ),
                                    ],
                                  ),
                                ),
                              ),
                            ],
                          ),
                        ),
                      ],
                    ),
                  ),
                ),
                const SizedBox(height: 12),
                Row(
                  children: [
                    Expanded(
                      child: Text(
                        'Changes apply after you save.',
                        style: wispSans(
                          fontSize: 11.5,
                          fontWeight: FontWeight.w400,
                          color: kMuted,
                        ),
                      ),
                    ),
                    const SizedBox(width: 12),
                    FilledButton(
                      onPressed: _isDirty && !saving ? _saveSettings : null,
                      style: FilledButton.styleFrom(
                        backgroundColor: kAccentCyanStrong,
                        foregroundColor: Colors.white,
                        minimumSize: const Size(0, 48),
                        shape: RoundedRectangleBorder(
                          borderRadius: BorderRadius.circular(12),
                        ),
                      ),
                      child: saving
                          ? const Text('Saving...')
                          : const Text('Save Changes'),
                    ),
                  ],
                ),
              ],
            ),
          ),
        ),
      ),
    );
  }
}

class _IdentityBadgeRow extends StatelessWidget {
  const _IdentityBadgeRow({required this.endpointId});

  final String endpointId;

  @override
  Widget build(BuildContext context) {
    final color = colorFromPubkey(endpointId);
    final textColor = HSLColor.fromColor(color).withLightness(0.32).toColor();
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Text(
          'Public key',
          style: wispSans(
            fontSize: 13.5,
            fontWeight: FontWeight.w600,
            color: kInk,
          ),
        ),
        const SizedBox(height: 10),
        Row(
          mainAxisAlignment: MainAxisAlignment.start,
          children: [
            Tooltip(
              message: endpointId,
              child: Container(
                padding: const EdgeInsets.symmetric(
                  horizontal: 10,
                  vertical: 6,
                ),
                decoration: BoxDecoration(
                  color: color.withValues(alpha: 0.15),
                  borderRadius: BorderRadius.circular(8),
                  border: Border.all(
                    color: color.withValues(alpha: 0.45),
                    width: 0.8,
                  ),
                ),
                child: Text(
                  shortPubkey(endpointId, headChars: 8, tailChars: 8),
                  style: wispSans(
                    fontSize: 12.5,
                    fontWeight: FontWeight.w700,
                    color: textColor,
                    letterSpacing: 0.4,
                  ),
                  maxLines: 1,
                ),
              ),
            ),
            const SizedBox(width: 4),
            IconButton(
              tooltip: 'Copy public key',
              visualDensity: VisualDensity.compact,
              padding: EdgeInsets.zero,
              constraints: const BoxConstraints(minWidth: 36, minHeight: 36),
              icon: const Icon(Icons.copy_rounded, size: 18),
              color: kMuted,
              onPressed: () async {
                await Clipboard.setData(ClipboardData(text: endpointId));
                if (!context.mounted) return;
                ScaffoldMessenger.of(context).showSnackBar(
                  const SnackBar(
                    content: Text('Public key copied'),
                    duration: Duration(seconds: 2),
                  ),
                );
              },
            ),
          ],
        ),
      ],
    );
  }
}
