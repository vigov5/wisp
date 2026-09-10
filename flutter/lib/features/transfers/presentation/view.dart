import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../application/controller.dart';
import '../application/manifest.dart';
import '../application/received_file_opener.dart';
import '../application/saved_folder_opener.dart';
import '../application/service.dart';
import '../application/result_view_data.dart';
import '../application/state.dart';
import '../../settings/feature.dart';
import 'widgets/connecting_card.dart';
import 'widgets/receiving_card.dart';
import 'widgets/offer_card.dart';
import 'widgets/transfer_result_card.dart';

class TransfersFeature extends ConsumerWidget {
  const TransfersFeature({super.key});

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final state = ref.watch(transfersViewStateProvider);
    final animateReview = ref.watch(transferReviewAnimationProvider);
    final settings = ref.watch(settingsControllerProvider).settings;
    final platform = ref.watch(transferTargetPlatformProvider);
    final canOpenSavedFolderAction = canOpenSavedFolder(platform: platform);
    final openSavedFolderLabel = savedFolderOpenLabel(platform: platform);

    return SizedBox.expand(
      child: AnimatedSwitcher(
        duration: const Duration(milliseconds: 400),
        switchInCurve: Curves.easeOut,
        switchOutCurve: Curves.easeIn,
        // The card being replaced stays in the tree, fading, for the whole
        // 400 ms — and a fade does not stop it receiving taps: `FadeTransition`
        // is `Opacity`, which hit-tests at any opacity, including zero. The
        // default layout stacks the outgoing card *under* the incoming one, so
        // a tap that lands where the new card has nothing to absorb it falls
        // through to whatever the old card had at that spot.
        //
        // A user double-tapping Accept therefore triggered two accepts, 400 ms
        // being squarely inside a human double tap. The second one released the
        // 1911 descriptors the first one's transfer was already writing into
        // and killed it 140 ms in, which read as an intermittent fetch bug for
        // hours. The service refuses a second accept now, but the window is not
        // specific to Accept: the same stray tap could reach Decline on a card
        // that is on its way out, or any other action on any phase change.
        //
        // Ignoring pointers on the outgoing children only, so the incoming card
        // stays responsive from its first frame rather than going deaf for
        // 400 ms.
        layoutBuilder: (currentChild, previousChildren) => Stack(
          alignment: Alignment.center,
          children: <Widget>[
            ...previousChildren.map((child) => IgnorePointer(child: child)),
            ?currentChild,
          ],
        ),
        child: switch (state.phase) {
          TransferSessionPhase.connecting => ConnectingCard(
            key: const ValueKey('connecting'),
            offer: state.incomingOffer!,
            animate: animateReview,
            onCancel: () =>
                ref.read(transfersServiceProvider.notifier).cancelTransfer(),
          ),
          TransferSessionPhase.offerPending => OfferCard(
            key: const ValueKey('offer'),
            offer: state.incomingOffer!,
            animate: animateReview,
            onAccept: () =>
                ref.read(transfersServiceProvider.notifier).acceptOffer(),
            onAcceptText: (mode, saved) => ref
                .read(transfersServiceProvider.notifier)
                .acceptOffer(textDelivery: mode, savedText: saved),
            onDecline: () =>
                ref.read(transfersServiceProvider.notifier).declineOffer(),
          ),
          TransferSessionPhase.receiving => ReceivingCard(
            key: const ValueKey('receiving'),
            offer: state.incomingOffer!,
            progress: state.progress!,
            animate: animateReview,
            onCancel: () =>
                ref.read(transfersServiceProvider.notifier).cancelTransfer(),
          ),
          TransferSessionPhase.completed ||
          TransferSessionPhase.cancelled ||
          TransferSessionPhase.failed => _buildTransferResultCard(
            viewData: buildTransferResultViewData(state),
            onDone: () => ref
                .read(transfersServiceProvider.notifier)
                .dismissTransferResult(),
            onOpenFile: (item) async {
              final messenger = ScaffoldMessenger.of(context);
              final status = await openReceivedFile(
                relativePath: item.path,
                downloadRoot: settings.downloadRoot,
                source: ref.read(transfersServiceSourceProvider),
              );
              final message = switch (status) {
                ReceivedFileOpenStatus.opened => null,
                ReceivedFileOpenStatus.notReady =>
                  'Still saving this file — try again in a moment.',
                ReceivedFileOpenStatus.notFound => "Couldn't find this file.",
                ReceivedFileOpenStatus.noHandler =>
                  'No app on this device can open this file type.',
                ReceivedFileOpenStatus.unsupported =>
                  "Opening files isn't supported here.",
                ReceivedFileOpenStatus.error => "Couldn't open this file.",
              };
              if (message != null) {
                messenger.showSnackBar(SnackBar(content: Text(message)));
              }
            },
            onOpenSavedFolder:
                state.phase == TransferSessionPhase.completed &&
                    canOpenSavedFolderAction
                ? () {
                    unawaited(
                      ref.read(savedFolderOpenerProvider)(
                        settings.downloadRoot,
                      ),
                    );
                  }
                : null,
            openSavedFolderLabel:
                state.phase == TransferSessionPhase.completed &&
                    canOpenSavedFolderAction
                ? openSavedFolderLabel
                : null,
          ),
          _ => const SizedBox.shrink(key: ValueKey('idle')),
        },
      ),
    );
  }
}

Widget _buildTransferResultCard({
  required TransferResultViewData viewData,
  required VoidCallback onDone,
  required VoidCallback? onOpenSavedFolder,
  required String? openSavedFolderLabel,
  required void Function(TransferManifestItem item) onOpenFile,
}) {
  return TransferResultCard(
    viewData: viewData,
    onPrimary: onDone,
    onSecondary: onOpenSavedFolder,
    secondaryLabel: openSavedFolderLabel,
    onOpenFile: onOpenFile,
  );
}
