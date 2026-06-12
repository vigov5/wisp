import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../platform/identity_storage.dart';
import 'application/identity_backup_codec.dart';

/// The app's [IdentityStorage], constructed once in [loadAppBootstrap] and
/// injected here so the backup/import screens read and overwrite the *same*
/// persisted secret key the engine was started with. Overridden at bootstrap.
final identityStorageProvider = Provider<IdentityStorage>((ref) {
  throw UnimplementedError(
    'identityStorageProvider must be overridden at bootstrap',
  );
});

/// Stateless codec for backup payloads. Cheap to construct; shared so the
/// PBKDF2/AES-GCM algorithm objects are reused across encode/decode calls.
final identityBackupCodecProvider = Provider<IdentityBackupCodec>((ref) {
  return IdentityBackupCodec();
});
