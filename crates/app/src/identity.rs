use std::sync::Mutex;

use iroh::SecretKey;
use rand::RngCore;

static SECRET_KEY: Mutex<Option<SecretKey>> = Mutex::new(None);

/// Install a persistent secret key supplied by the host (Flutter side reads
/// from secure storage, generates once on first run).  Idempotent:
/// repeated calls with the same bytes are no-ops; calls with different bytes
/// overwrite the previously installed key.
pub fn set_secret_key(bytes: [u8; 32]) {
    let mut guard = SECRET_KEY.lock().expect("identity mutex poisoned");
    *guard = Some(SecretKey::from_bytes(&bytes));
}

/// Returns the installed identity. Caller is responsible for installing one
/// first (Flutter does this in bootstrap before any sender/receiver session
/// starts; CLI/tests use [`set_secret_key_for_tests`]).
///
/// On first access without a prior install, logs a warning and installs a
/// freshly-generated key. We don't panic: the previous behaviour silently
/// generated random keys, and panicking now would break every Flutter app
/// that races a service start before bootstrap finishes. But the warning
/// surfaces the bug — a session that begins with the warning will produce
/// a different EndpointId than one started after bootstrap completes,
/// which causes "channel closed" / "unknown sender" handshake failures.
pub fn current_secret_key() -> SecretKey {
    let mut guard = SECRET_KEY.lock().expect("identity mutex poisoned");
    if let Some(key) = guard.as_ref() {
        return key.clone();
    }
    tracing::warn!(
        target: "drift_app::identity",
        "current_secret_key() called before set_secret_key(); generating ephemeral key — \
         the resulting EndpointId will not match the one persisted by the host"
    );
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    let key = SecretKey::from_bytes(&bytes);
    *guard = Some(key.clone());
    key
}

/// Test helper: clear the installed identity. Production code must not call
/// this. Allows tests to validate the warning path or to swap identities
/// between subtests.
#[cfg(test)]
pub fn clear_secret_key_for_tests() {
    let mut guard = SECRET_KEY.lock().expect("identity mutex poisoned");
    *guard = None;
}
