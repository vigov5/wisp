#[flutter_rust_bridge::frb(init)]
pub fn init_app() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        tracing_subscriber::EnvFilter::new("warn,drift_core=info,drift_app=info,drift_bridge=info")
    });

    #[cfg(target_os = "android")]
    {
        use tracing_subscriber::prelude::*;
        let _ = tracing_subscriber::registry()
            .with(filter)
            .with(tracing_android::layer("drift").unwrap())
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
