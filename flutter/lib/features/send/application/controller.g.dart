// GENERATED CODE - DO NOT MODIFY BY HAND

part of 'controller.dart';

// **************************************************************************
// RiverpodGenerator
// **************************************************************************

// GENERATED CODE - DO NOT MODIFY BY HAND
// ignore_for_file: type=lint, type=warning

@ProviderFor(SendController)
final sendControllerProvider = SendControllerProvider._();

final class SendControllerProvider
    extends $NotifierProvider<SendController, SendState> {
  SendControllerProvider._()
    : super(
        from: null,
        argument: null,
        retry: null,
        name: r'sendControllerProvider',
        isAutoDispose: false,
        dependencies: null,
        $allTransitiveDependencies: null,
      );

  @override
  String debugGetCreateSourceHash() => _$sendControllerHash();

  @$internal
  @override
  SendController create() => SendController();

  /// {@macro riverpod.override_with_value}
  Override overrideWithValue(SendState value) {
    return $ProviderOverride(
      origin: this,
      providerOverride: $SyncValueProvider<SendState>(value),
    );
  }
}

String _$sendControllerHash() => r'8df0c0e2940b9d0fc817426c25c5ee4f925a44b3';

abstract class _$SendController extends $Notifier<SendState> {
  SendState build();
  @$mustCallSuper
  @override
  void runBuild() {
    final ref = this.ref as $Ref<SendState, SendState>;
    final element =
        ref.element
            as $ClassProviderElement<
              AnyNotifier<SendState, SendState>,
              SendState,
              Object?,
              Object?
            >;
    element.handleCreate(ref, build);
  }
}
