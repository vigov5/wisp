#[flutter_rust_bridge::frb(init)]
pub fn init_app() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        tracing_subscriber::EnvFilter::new("warn,wisp_core=info,wisp_app=info,wisp_bridge=info")
    });

    #[cfg(target_os = "android")]
    {
        use tracing_subscriber::prelude::*;
        let _ = tracing_subscriber::registry()
            .with(filter)
            .with(tracing_android::layer("wisp").unwrap())
            .try_init();
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = tracing_subscriber::fmt().with_env_filter(filter).try_init();
    }
    // Ensures RUNTIME and static setup are touched when the Dart side initializes Rust.
}

#[flutter_rust_bridge::frb(sync)]
pub fn greet(name: String) -> String {
    format!("Hello, {name}")
}

/// Installs the persistent app secret key supplied by the host. Bytes are the
/// raw 32-byte iroh secret key, generated/persisted on the Flutter side.
/// Should be called exactly once during app bootstrap, before any sender or
/// receiver session starts.  Returns an error string when the byte length is
/// wrong; otherwise `Ok(())`.
#[flutter_rust_bridge::frb(sync)]
pub fn set_app_identity(secret_key_bytes: Vec<u8>) -> Result<(), String> {
    let bytes: [u8; 32] = secret_key_bytes
        .try_into()
        .map_err(|_| "secret key must be exactly 32 bytes".to_owned())?;
    wisp_app::identity::set_secret_key(bytes);
    Ok(())
}

/// Returns the base32-encoded EndpointId derived from the installed secret
/// key. Stable for the lifetime of the install. Surfaced for the settings
/// screen so the user can copy/share their identity.
#[flutter_rust_bridge::frb(sync)]
pub fn current_endpoint_id() -> String {
    wisp_app::identity::current_secret_key()
        .public()
        .to_string()
}
