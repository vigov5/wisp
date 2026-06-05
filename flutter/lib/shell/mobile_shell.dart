import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../features/receive/application/controller.dart';
import '../features/receive/application/service.dart';
import '../features/receive/presentation/qr_pairing_page.dart';
import '../features/receive/presentation/receive_transfer_route_gate.dart';
import '../features/receive/presentation/widgets/receiver_error_banner.dart';
import '../features/send/presentation/send_selection_source_sheet.dart';
import '../app/app_router.dart';
import '../theme/wisp_theme.dart';
import 'widgets/android_permission_bootstrap.dart';
import 'widgets/mobile_identity_card.dart';
import 'widgets/select_files_card.dart';
import 'widgets/shell_picking_actions.dart';

class MobileShell extends ConsumerWidget with ShellPickingActions {
  const MobileShell({super.key});

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final receiverState = ref.watch(receiverIdleViewStateProvider);
    final receiverError = ref.watch(
      receiverServiceProvider.select((s) => s.error),
    );

    return AndroidPermissionBootstrap(
      child: ReceiveTransferRouteGate(
        child: Scaffold(
          backgroundColor: kBg,
          body: CustomScrollView(
            physics: const BouncingScrollPhysics(),
            slivers: [
              SliverToBoxAdapter(
                child: SafeArea(
                  bottom: false,
                  child: Padding(
                    padding: const EdgeInsets.fromLTRB(16, 8, 16, 0),
                    child: Row(
                      children: [
                        const Spacer(),
                        IconButton(
                          onPressed: () {
                            Navigator.of(context).push(
                              MaterialPageRoute<void>(
                                builder: (_) => const QrPairingPage(),
                              ),
                            );
                          },
                          icon: const Icon(Icons.qr_code_rounded),
                          tooltip: 'Pair via QR',
                        ),
                        IconButton(
                          onPressed: () => context.goSettings(),
                          icon: const Icon(Icons.tune_rounded),
                          tooltip: 'Settings',
                        ),
                      ],
                    ),
                  ),
                ),
              ),
              if (receiverError != null)
                SliverPadding(
                  padding: const EdgeInsets.fromLTRB(20, 8, 20, 0),
                  sliver: SliverToBoxAdapter(
                    child: ReceiverErrorBanner(
                      error: receiverError,
                      onDismiss: () => ref
                          .read(receiverServiceProvider.notifier)
                          .clearError(),
                    ),
                  ),
                ),
              SliverPadding(
                padding: const EdgeInsets.symmetric(horizontal: 20),
                sliver: SliverList(
                  delegate: SliverChildListDelegate([
                    const SizedBox(height: 8),
                    MobileIdentityCard(
                      state: receiverState,
                      onRefreshCode: () {
                        unawaited(
                          ref
                              .read(receiverServiceProvider.notifier)
                              .ensureRegistered(),
                        );
                      },
                    ),
                    const SizedBox(height: 32),
                    SelectFilesCard(
                      icon: Icons.insert_drive_file_outlined,
                      title: 'Share file',
                      subtitle: 'Send files or a folder.',
                      onTap: () {
                        showSendSelectionSourceSheet(
                          context,
                          onChooseFiles: () => pickFiles(context, ref),
                          onChooseFolder: () => pickFolder(context, ref),
                        );
                      },
                    ),
                    const SizedBox(height: 12),
                    SelectFilesCard(
                      icon: Icons.notes_rounded,
                      title: 'Share text',
                      subtitle: 'Type or paste text to send.',
                      onTap: () => shareText(context, ref),
                    ),
                    const SizedBox(height: 12),
                    SelectFilesCard(
                      icon: Icons.content_paste_rounded,
                      title: 'Share clipboard',
                      subtitle: 'Send what\'s on your clipboard.',
                      onTap: () => shareClipboard(context, ref),
                    ),
                    const SizedBox(height: 24),
                  ]),
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }
}
