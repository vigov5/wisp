import 'package:flutter/foundation.dart';
import 'package:permission_handler/permission_handler.dart';

import '../domain/check_result.dart';

class PermissionProbe {
  const PermissionProbe();

  static const String notificationCheckId = 'permissions.notifications';

  bool get supportsNotificationCheck {
    if (kIsWeb) return false;
    switch (defaultTargetPlatform) {
      case TargetPlatform.android:
      case TargetPlatform.iOS:
      case TargetPlatform.macOS:
        return true;
      case TargetPlatform.windows:
      case TargetPlatform.linux:
      case TargetPlatform.fuchsia:
        return false;
    }
  }

  List<CheckResult> initialPendingChecks() {
    if (!supportsNotificationCheck) return const [];
    return const [
      CheckResult(
        id: notificationCheckId,
        group: CheckGroup.permissions,
        status: CheckStatus.running,
        label: 'Notifications',
      ),
    ];
  }

  Future<List<CheckResult>> runChecks() async {
    if (!supportsNotificationCheck) return const [];
    final results = <CheckResult>[];
    results.add(await _checkNotifications());
    return results;
  }

  Future<CheckResult> _checkNotifications() async {
    PermissionStatus status;
    try {
      status = await Permission.notification.status;
    } catch (error) {
      return CheckResult(
        id: notificationCheckId,
        group: CheckGroup.permissions,
        status: CheckStatus.warn,
        label: 'Notifications',
        detail: 'Could not query status: $error',
      );
    }
    switch (status) {
      case PermissionStatus.granted:
      case PermissionStatus.provisional:
        return const CheckResult(
          id: notificationCheckId,
          group: CheckGroup.permissions,
          status: CheckStatus.pass,
          label: 'Notifications granted',
        );
      case PermissionStatus.denied:
        return const CheckResult(
          id: notificationCheckId,
          group: CheckGroup.permissions,
          status: CheckStatus.warn,
          label: 'Notifications not granted yet',
          detail:
              'Transfers will still work, but you won\'t get an alert when '
              'one arrives.',
          hint:
              'Tap "Open app settings" to allow notifications, or accept the '
              'system prompt the next time it appears.',
          action: CheckAction(
            label: 'Open app settings',
            kind: CheckActionKind.openAppSettings,
          ),
        );
      case PermissionStatus.permanentlyDenied:
        return const CheckResult(
          id: notificationCheckId,
          group: CheckGroup.permissions,
          status: CheckStatus.warn,
          label: 'Notifications denied',
          detail:
              'Transfers will still work, but you won\'t get an alert when '
              'one arrives.',
          hint:
              'You\'ll need to enable Notifications for Drift in system '
              'settings.',
          action: CheckAction(
            label: 'Open app settings',
            kind: CheckActionKind.openAppSettings,
          ),
        );
      case PermissionStatus.restricted:
      case PermissionStatus.limited:
        return CheckResult(
          id: notificationCheckId,
          group: CheckGroup.permissions,
          status: CheckStatus.warn,
          label: 'Notifications $status',
          detail:
              'System policy is limiting notifications. Some alerts may not '
              'reach you.',
        );
    }
  }
}
