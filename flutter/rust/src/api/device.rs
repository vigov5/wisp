#[flutter_rust_bridge::frb(sync)]
pub fn random_device_name() -> String {
    wisp_core::util::random_device_name()
}
