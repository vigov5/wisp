use std::time::Duration;

use std::sync::Arc;

use iroh::endpoint::{ControllerFactory, QuicTransportConfig, VarInt};
use wisp_core::blobs::receive::BlobTransportProfile;

/// Per-stream flow-control receive window advertised by this endpoint.
///
/// A drift transfer pulls a whole collection over **one** QUIC stream, so
/// throughput is hard-capped at `stream_receive_window / RTT`. quinn's default
/// is only 1.25 MB ("tuned for a 100 Mbps × 100 ms link"), which throttles the
/// relay path to ~12.5 MB/s regardless of real bandwidth. We raise it so the
/// window stops being the ceiling on high-RTT paths:
///
/// - **Desktop: 16 MiB** — lifts the relay cap to ~160 MB/s @100 ms RTT.
/// - **Mobile (Android): 8 MiB** — lifts it to ~80 MB/s @100 ms; ~8 MB RAM per
///   active transfer (one stream), safe on modern phones.
///
/// Tiered by build target so the heavier window only costs RAM where there's
/// headroom for it, with no call-site signature churn.
#[cfg(target_os = "android")]
const STREAM_RECEIVE_WINDOW_BYTES: u32 = 8 * 1024 * 1024;

#[cfg(not(target_os = "android"))]
const STREAM_RECEIVE_WINDOW_BYTES: u32 = 16 * 1024 * 1024;

/// Bytes the sender may keep in flight, independent of the receive window.
///
/// Derived as `8 x` the stream window until it was measured. That coupling was
/// an accident: raising the receive window for the relay's sake also raised the
/// in-flight cap to 64 MiB, and the two knobs answer different questions.
/// `stream_receive_window` is advertised for streams this endpoint *receives*,
/// so on a phone → desktop transfer it does not govern the bulk at all;
/// `send_window` is what bounds queueing on the way out.
///
/// Left at the derived value for now. A 2 MiB cap measures better on both paths
/// tested — on the tether -5% throughput for 99.8% less loss and a much tighter
/// spread, on the relay no established throughput cost for ~80% less loss — but
/// that is `quic_baseline`, and the app has to agree before the default moves.
/// Use `debug.wisp.send_win_kib` to test it. Do not pick 1 MiB on the tether
/// numbers alone: it is the best of the sweep there and costs ~27% on the relay,
/// whose bandwidth-delay product is about 0.9 MiB.
fn default_send_window_bytes() -> u64 {
    8u64 * u64::from(STREAM_RECEIVE_WINDOW_BYTES)
}

/// Reads a `u32` from an Android system property, clamped to `range`.
///
/// Absent or unparseable leaves the caller's default, and clamping stops a typo
/// producing a degenerate config. Returns `None` off Android, where there is no
/// property store.
///
/// **Debug builds only.** Unlike `debug.wisp.log`, which only widens logging,
/// these knobs reconfigure the transport, and a release build should not be
/// silently retunable by anyone who can reach it over adb. The sweeps that use
/// them run against a debug APK anyway.
#[cfg(all(target_os = "android", debug_assertions))]
fn property_u32(name: &str, lo: u32, hi: u32) -> Option<u32> {
    use std::ffi::{CStr, CString, c_char, c_int};

    const PROP_VALUE_MAX: usize = 92;
    unsafe extern "C" {
        fn __system_property_get(name: *const c_char, value: *mut c_char) -> c_int;
    }

    let name = CString::new(name).ok()?;
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
    value.parse::<u32>().ok().map(|v| v.clamp(lo, hi))
}

#[cfg(not(all(target_os = "android", debug_assertions)))]
fn property_u32(_name: &str, _lo: u32, _hi: u32) -> Option<u32> {
    None
}

/// The stream receive window actually used, after any debug override.
fn stream_receive_window_bytes() -> u32 {
    match property_u32("debug.wisp.stream_win_mib", 1, 64) {
        Some(mib) => mib * 1024 * 1024,
        None => STREAM_RECEIVE_WINDOW_BYTES,
    }
}

/// The send window actually used, after any debug override.
///
/// In KiB rather than MiB so the sweep can reach the sub-MiB range where the
/// path's bandwidth-delay product actually lives.
fn send_window_bytes() -> u64 {
    match property_u32("debug.wisp.send_win_kib", 64, 262_144) {
        Some(kib) => u64::from(kib) * 1024,
        None => default_send_window_bytes(),
    }
}

/// Congestion controller override, read from `debug.wisp.cc` on Android.
///
/// Shipping default is unchanged — absent property means iroh's own default,
/// CUBIC, and nothing is installed. The knob exists because E2 requires a
/// benchmark-only override rather than a changed default, and because the
/// controller in noq-proto 1.1 is **BBR3**, not the BBR that was enabled in
/// `d386240` and reverted in `05c9e4d` for phone-to-phone Wi-Fi stutter. That
/// reversal was of a different algorithm, so BBR3 needs measuring on its own —
/// and the stutter symptom lives in p10/CV on Wi-Fi, not in the median on a
/// steady tether, so a trial has to cover both links.
fn congestion_override() -> Option<Arc<dyn ControllerFactory + Send + Sync>> {
    match property_string("debug.wisp.cc")?.as_str() {
        "bbr3" => Some(Arc::new(noq_proto::congestion::Bbr3Config::default())),
        "cubic" => Some(Arc::new(noq_proto::congestion::CubicConfig::default())),
        _ => None,
    }
}

/// The controller name reported in telemetry, so a sweep cannot record the
/// wrong configuration against its own numbers.
fn congestion_controller_name() -> &'static str {
    match property_string("debug.wisp.cc").as_deref() {
        Some("bbr3") => "bbr3",
        _ => "cubic",
    }
}

#[cfg(all(target_os = "android", debug_assertions))]
fn property_string(name: &str) -> Option<String> {
    use std::ffi::{CStr, CString, c_char, c_int};

    const PROP_VALUE_MAX: usize = 92;
    unsafe extern "C" {
        fn __system_property_get(name: *const c_char, value: *mut c_char) -> c_int;
    }

    let name = CString::new(name).ok()?;
    let mut buffer = [0u8; PROP_VALUE_MAX + 1];
    let written = unsafe { __system_property_get(name.as_ptr(), buffer.as_mut_ptr().cast()) };
    if written <= 0 {
        return None;
    }
    let value = CStr::from_bytes_until_nul(&buffer)
        .ok()?
        .to_str()
        .ok()?
        .trim()
        .to_owned();
    (!value.is_empty()).then_some(value)
}

#[cfg(not(all(target_os = "android", debug_assertions)))]
fn property_string(_name: &str) -> Option<String> {
    None
}

pub(crate) fn blob_transport_profile() -> BlobTransportProfile {
    // Must report the window in force, not the compiled-in default, or the
    // telemetry disagrees with the transport during a sweep.
    BlobTransportProfile::new(
        u64::from(stream_receive_window_bytes()),
        u64::from(VarInt::MAX),
        send_window_bytes(),
        congestion_controller_name(),
    )
}

/// Tuned QUIC transport config for drift: keepalive (Android-friendly) plus the
/// throughput knobs that lift the single-stream ceiling on high-latency paths.
///
/// **Keepalive** — iroh caps the QUIC-level path idle/keepalive at 6.5s / 5s
/// respectively (anything larger is logged as a warning and ignored). Higher-
/// level resilience — surviving Doze pauses, NAT churn — is handled by iroh's
/// path migration + relay fallback, not these knobs. We pick values just under
/// the cap so the QUIC layer keeps NAT mappings warm during active transfers
/// without tripping iroh's clamp.
///
/// - `default_path_max_idle_timeout = 6_000ms` — peer must respond within 6s
///   or QUIC tears the path; iroh then re-establishes via its own logic.
/// - `default_path_keep_alive_interval = 4_500ms` — sub-5s ping keeps the
///   common NAT 30-60s binding window alive while transfers are in flight.
///
/// **Throughput**
/// - `stream_receive_window` — see [`STREAM_RECEIVE_WINDOW_BYTES`]; the single
///   biggest win on the relay path.
/// - `send_window` — see [`default_send_window_bytes`]. Independent of the
///   stream window: it bounds bytes in flight on the way out, where the stream
///   window governs streams this endpoint receives.
///
/// Congestion control is left at iroh's default (**CUBIC**). BBR was tried but
/// made phone-to-phone Wi-Fi throughput visibly stutter: its ProbeBW gain
/// cycling and periodic ProbeRTT cwnd collapses oscillate the rate on low-RTT
/// direct paths. The relay win comes from the window (ceiling = window / RTT),
/// not the controller, so keeping CUBIC costs nothing on relay while restoring a
/// steady Wi-Fi rate.
///
/// `initial_mtu` (1200) and MTU discovery are left at iroh's defaults — safe on
/// every path; `receive_window` (connection-level) stays at iroh's
/// `VarInt::MAX` default, so it never limits.
pub(crate) fn build_transport_config() -> QuicTransportConfig {
    // `from_u32` is infallible for our values (well under VarInt::MAX);
    // `send_window` is a plain u64 and no longer derived from the stream window.
    let stream_receive_window = VarInt::from_u32(stream_receive_window_bytes());
    let send_window = send_window_bytes();

    let mut builder = QuicTransportConfig::builder()
        .default_path_max_idle_timeout(Duration::from_millis(6_000))
        .default_path_keep_alive_interval(Duration::from_millis(4_500))
        .stream_receive_window(stream_receive_window)
        .send_window(send_window);
    if let Some(factory) = congestion_override() {
        builder = builder.congestion_controller_factory(factory);
    }
    builder.build()
}

#[cfg(test)]
mod tests {
    use super::{
        STREAM_RECEIVE_WINDOW_BYTES, blob_transport_profile, build_transport_config,
        default_send_window_bytes, send_window_bytes, stream_receive_window_bytes,
    };

    /// Off Android there is no property to read, so the compiled-in value must
    /// survive untouched — the override must never affect desktop builds.
    #[test]
    fn stream_window_defaults_to_the_compiled_in_value() {
        assert_eq!(stream_receive_window_bytes(), STREAM_RECEIVE_WINDOW_BYTES);
    }

    /// The profile is what telemetry reports; it has to agree with the windows
    /// actually in force, or a sweep records the wrong configuration against
    /// its own numbers.
    #[test]
    fn profile_reports_the_windows_in_force() {
        use iroh::endpoint::VarInt;
        use wisp_core::blobs::receive::BlobTransportProfile;

        assert_eq!(
            blob_transport_profile(),
            BlobTransportProfile::new(
                u64::from(stream_receive_window_bytes()),
                u64::from(VarInt::MAX),
                send_window_bytes(),
                "cubic",
            ),
        );
    }

    /// Off Android there is no property store, so no controller is installed
    /// and iroh's default (CUBIC) stands — the override must never change a
    /// desktop build, and the reported name must agree with that.
    #[test]
    fn congestion_override_is_absent_without_the_property() {
        assert!(super::congestion_override().is_none());
        assert_eq!(super::congestion_controller_name(), "cubic");
    }

    /// The default must stay exactly where it was until the app-side
    /// measurement lands: decoupling the knobs is not meant to change behaviour.
    #[test]
    fn send_window_default_is_unchanged() {
        assert_eq!(send_window_bytes(), default_send_window_bytes());
        assert_eq!(
            default_send_window_bytes(),
            8 * u64::from(STREAM_RECEIVE_WINDOW_BYTES)
        );
    }

    #[test]
    fn build_transport_config_runs_without_panic() {
        // Smoke test: the builder accepts the chosen Durations and windows and
        // produces a value. We can't assert internal state because iroh doesn't
        // expose getters on QuicTransportConfig — but a compile-and-run check
        // guards against API drift on iroh upgrades.
        let _ = build_transport_config();
        let _ = blob_transport_profile();
    }
}
