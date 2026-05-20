import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';
import 'package:permission_handler/permission_handler.dart';

/// Fires the Android 13+ POST_NOTIFICATIONS prompt once per process so the
/// receiver's foreground-service notification can render. Without this the
/// system silently rejects the FGS notification on a fresh install, which
/// hides incoming-transfer alerts and makes the receive flow look broken.
///
/// Renders [child] unchanged; the request runs in a post-frame callback so
/// it does not block first paint and does not show during widget tests.
class AndroidPermissionBootstrap extends StatefulWidget {
  const AndroidPermissionBootstrap({super.key, required this.child});

  final Widget child;

  @override
  State<AndroidPermissionBootstrap> createState() =>
      _AndroidPermissionBootstrapState();
}

class _AndroidPermissionBootstrapState
    extends State<AndroidPermissionBootstrap> {
  static bool _requestedThisProcess = false;

  @override
  void initState() {
    super.initState();
    if (!kIsWeb &&
        defaultTargetPlatform == TargetPlatform.android &&
        !_requestedThisProcess) {
      _requestedThisProcess = true;
      WidgetsBinding.instance.addPostFrameCallback((_) => _maybeRequest());
    }
  }

  Future<void> _maybeRequest() async {
    try {
      final status = await Permission.notification.status;
      if (status.isDenied) {
        await Permission.notification.request();
      }
    } catch (_) {
      // Plugin missing (e.g., widget tests) — silent ignore is fine since
      // the static guard prevents repeated attempts.
    }
  }

  @override
  Widget build(BuildContext context) => widget.child;
}
