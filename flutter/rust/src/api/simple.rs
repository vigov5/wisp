use std::sync::OnceLock;

const TRANSFER_TELEMETRY_TARGET: &str = "wisp_transfer_telemetry";
type FilterReloadHandle =
    tracing_subscriber::reload::Handle<tracing_subscriber::EnvFilter, tracing_subscriber::Registry>;
static BASE_LOG_FILTER: OnceLock<String> = OnceLock::new();
static LOG_FILTER_RELOAD: OnceLock<FilterReloadHandle> = OnceLock::new();

#[cfg(target_os = "android")]
mod android_telemetry {
    use std::ffi::{c_char, c_int, CString};

    use serde_json::{Map, Number, Value};
    use tracing::{
        field::{Field, Visit},
        Event, Subscriber,
    };
    use tracing_subscriber::{layer::Context, Layer};

    const ANDROID_LOG_DEBUG: c_int = 3;
    const LOGCAT_TAG: &[u8] = b"wisp\0";

    #[link(name = "log")]
    unsafe extern "C" {
        fn __android_log_write(priority: c_int, tag: *const c_char, text: *const c_char) -> c_int;
    }

    #[derive(Default)]
    struct JsonFields(Map<String, Value>);

    impl JsonFields {
        fn insert(&mut self, field: &Field, value: Value) {
            self.0.insert(field.name().to_owned(), value);
        }
    }

    impl Visit for JsonFields {
        fn record_f64(&mut self, field: &Field, value: f64) {
            if let Some(value) = Number::from_f64(value) {
                self.insert(field, Value::Number(value));
            }
        }

        fn record_i64(&mut self, field: &Field, value: i64) {
            self.insert(field, Value::Number(value.into()));
        }

        fn record_u64(&mut self, field: &Field, value: u64) {
            self.insert(field, Value::Number(value.into()));
        }

        fn record_bool(&mut self, field: &Field, value: bool) {
            self.insert(field, Value::Bool(value));
        }

        fn record_str(&mut self, field: &Field, value: &str) {
            self.insert(field, Value::String(value.to_owned()));
        }

        fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
            self.insert(field, Value::String(format!("{value:?}")));
        }
    }

    #[derive(Clone, Copy, Debug, Default)]
    pub(super) struct TelemetryLayer;

    impl<S> Layer<S> for TelemetryLayer
    where
        S: Subscriber,
    {
        fn on_event(&self, event: &Event<'_>, _context: Context<'_, S>) {
            let mut fields = JsonFields::default();
            event.record(&mut fields);

            let mut entry = Map::new();
            entry.insert(
                "level".to_owned(),
                Value::String(event.metadata().level().as_str().to_owned()),
            );
            entry.insert("fields".to_owned(), Value::Object(fields.0));
            entry.insert(
                "target".to_owned(),
                Value::String(event.metadata().target().to_owned()),
            );
            let Ok(payload) = serde_json::to_vec(&Value::Object(entry)) else {
                return;
            };
            let Ok(message) = CString::new(payload) else {
                return;
            };

            // Telemetry fields are bounded and exclude span context, so each
            // pseudonymous JSON event remains below logcat's line limit.
            unsafe {
                __android_log_write(
                    ANDROID_LOG_DEBUG,
                    LOGCAT_TAG.as_ptr().cast(),
                    message.as_ptr(),
                );
            };
        }
    }
}

/// Log filter override read from the `debug.wisp.log` Android system property.
///
/// `RUST_LOG` is the normal way in, but nothing sets an environment variable for
/// an app launched from the launcher, so on-device diagnosis otherwise means
/// rebuilding with a different default. `adb shell setprop debug.wisp.log
/// "warn,iroh::socket=debug"` followed by a restart of the app is enough, and
/// `debug.`-prefixed properties are writable from adb without root.
///
/// Values are capped at bionic's `PROP_VALUE_MAX` (92 bytes), which fits the
/// filters worth typing; anything longer is silently truncated by the platform,
/// so a truncated directive is dropped by `EnvFilter` rather than misparsed.
#[cfg(target_os = "android")]
fn property_log_filter() -> Option<tracing_subscriber::EnvFilter> {
    use std::ffi::{c_char, c_int, CStr, CString};

    const PROP_NAME: &str = "debug.wisp.log";
    const PROP_VALUE_MAX: usize = 92;

    unsafe extern "C" {
        fn __system_property_get(name: *const c_char, value: *mut c_char) -> c_int;
    }

    let name = CString::new(PROP_NAME).ok()?;
    // One byte over PROP_VALUE_MAX so a maximum-length value stays NUL-terminated.
    let mut buffer = [0u8; PROP_VALUE_MAX + 1];
    let written = unsafe { __system_property_get(name.as_ptr(), buffer.as_mut_ptr().cast()) };
    if written <= 0 {
        return None;
    }
    let value = CStr::from_bytes_until_nul(&buffer)
        .ok()?
        .to_str()
        .ok()?
        .trim();
    if value.is_empty() {
        return None;
    }
    // Reject a filter we cannot parse rather than starting with no filter at all.
    tracing_subscriber::EnvFilter::try_new(value).ok()
}

#[cfg(not(target_os = "android"))]
fn property_log_filter() -> Option<tracing_subscriber::EnvFilter> {
    None
}

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
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .ok()
        .or_else(property_log_filter)
        .unwrap_or_else(|| {
            tracing_subscriber::EnvFilter::new("warn,wisp_core=info,wisp_app=info,wisp_bridge=info")
        });
    let base_filter = filter.to_string();
    let (filter, reload_handle) = tracing_subscriber::reload::Layer::new(filter);

    #[cfg(target_os = "android")]
    {
        use tracing_subscriber::prelude::*;
        let regular_log_filter = tracing_subscriber::filter::filter_fn(|metadata| {
            metadata.target() != TRANSFER_TELEMETRY_TARGET
        });
        let telemetry_target_filter = tracing_subscriber::filter::filter_fn(|metadata| {
            metadata.target() == TRANSFER_TELEMETRY_TARGET
        });
        let telemetry_layer =
            android_telemetry::TelemetryLayer.with_filter(telemetry_target_filter);
        let initialized = tracing_subscriber::registry()
            .with(filter)
            .with(
                tracing_android::layer("wisp")
                    .unwrap()
                    .with_filter(regular_log_filter),
            )
            .with(telemetry_layer)
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

/// Enables the pseudonymous transfer benchmark target for this process.
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
