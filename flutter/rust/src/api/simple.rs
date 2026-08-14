use std::sync::OnceLock;

const TRANSFER_TELEMETRY_TARGET: &str = "wisp_transfer_telemetry";
type FilterReloadHandle =
    tracing_subscriber::reload::Handle<tracing_subscriber::EnvFilter, tracing_subscriber::Registry>;
static BASE_LOG_FILTER: OnceLock<String> = OnceLock::new();
static LOG_FILTER_RELOAD: OnceLock<FilterReloadHandle> = OnceLock::new();

fn log_filter(base: &str, transfer_telemetry_enabled: bool) -> tracing_subscriber::EnvFilter {
    let filter = tracing_subscriber::EnvFilter::new(base);
    if transfer_telemetry_enabled {
        filter.add_directive(
            format!("{TRANSFER_TELEMETRY_TARGET}=debug")
                .parse()
                .expect("static transfer telemetry directive must be valid"),
        )
    } else {
        filter
    }
}

#[flutter_rust_bridge::frb(init)]
pub fn init_app() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        tracing_subscriber::EnvFilter::new("warn,wisp_core=info,wisp_app=info,wisp_bridge=info")
    });
    let base_filter = filter.to_string();
    let (filter, reload_handle) = tracing_subscriber::reload::Layer::new(filter);

    #[cfg(target_os = "android")]
    {
        use tracing_subscriber::prelude::*;
        let initialized = tracing_subscriber::registry()
            .with(filter)
            .with(tracing_android::layer("wisp").unwrap())
            .try_init()
            .is_ok();
        if initialized {
            let _ = BASE_LOG_FILTER.set(base_filter);
            let _ = LOG_FILTER_RELOAD.set(reload_handle);
        }
    }
    #[cfg(not(target_os = "android"))]
    {
        use tracing_subscriber::prelude::*;
        let initialized = tracing_subscriber::registry()
            .with(filter)
            .with(tracing_subscriber::fmt::layer())
            .try_init();
        if initialized.is_ok() {
            let _ = BASE_LOG_FILTER.set(base_filter);
            let _ = LOG_FILTER_RELOAD.set(reload_handle);
        }
    }
    // Ensures RUNTIME and static setup are touched when the Dart side initializes Rust.
}

/// Enables the anonymous transfer benchmark target for this process.
///
/// The Flutter host calls this immediately after Rust initialization using its
/// compile-time benchmark flag. Other debug targets remain on the normal log
/// filter, and the default remains disabled so production transfers do not pay
/// the telemetry sampler cost.
#[flutter_rust_bridge::frb(sync)]
pub fn set_transfer_telemetry_enabled(enabled: bool) {
    let (Some(base), Some(handle)) = (BASE_LOG_FILTER.get(), LOG_FILTER_RELOAD.get()) else {
        return;
    };
    let _ = handle.reload(log_filter(base, enabled));
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

#[cfg(test)]
mod tests {
    use super::{log_filter, TRANSFER_TELEMETRY_TARGET};

    #[test]
    fn transfer_telemetry_directive_is_opt_in() {
        let base = "warn,wisp_core=info,wisp_app=info";

        let disabled = log_filter(base, false).to_string();
        assert!(!disabled.contains(TRANSFER_TELEMETRY_TARGET));
        assert!(disabled.contains("wisp_core=info"));
        assert!(disabled.contains("wisp_app=info"));

        let enabled = log_filter(base, true).to_string();
        assert!(enabled.contains("wisp_transfer_telemetry=debug"));
        assert!(enabled.contains("wisp_core=info"));
        assert!(enabled.contains("wisp_app=info"));
    }

    #[test]
    fn disabling_telemetry_preserves_an_explicit_base_directive() {
        let filter = log_filter("warn,wisp_transfer_telemetry=trace", false).to_string();

        assert!(filter.contains("wisp_transfer_telemetry=trace"));
    }
}
