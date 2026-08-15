use std::time::Duration;

use iroh::endpoint::{QuicTransportConfig, VarInt};
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

/// Override for [`STREAM_RECEIVE_WINDOW_BYTES`], in MiB, read from the
/// `debug.wisp.stream_win_mib` system property on Android.
///
/// This exists because the right window is path-dependent and one static value
/// cannot serve both cases. Sized for the relay (ceiling = window / RTT at
/// ~100 ms) the window is appropriate; on a USB tether whose idle RTT is ~2 ms
/// the same value is ~840x the bandwidth-delay product, and measured sender RTT
/// runs p50 82 ms / max 487 ms with cwnd reaching 25 MB. Sweeping the window is
/// how we find out whether capping it trades away throughput.
///
/// Debug-only knob, same mechanism as `debug.wisp.log`: absent or unparseable
/// leaves the compiled-in value, and the value is clamped so a typo cannot
/// produce a degenerate config.
#[cfg(target_os = "android")]
fn stream_window_override_mib() -> Option<u32> {
    use std::ffi::{CStr, CString, c_char, c_int};

    const PROP_NAME: &str = "debug.wisp.stream_win_mib";
    const PROP_VALUE_MAX: usize = 92;
    unsafe extern "C" {
        fn __system_property_get(name: *const c_char, value: *mut c_char) -> c_int;
    }

    let name = CString::new(PROP_NAME).ok()?;
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
    value.parse::<u32>().ok().map(|mib| mib.clamp(1, 64))
}

#[cfg(not(target_os = "android"))]
fn stream_window_override_mib() -> Option<u32> {
    None
}

/// The window actually used, after any debug override.
fn stream_receive_window_bytes() -> u32 {
    match stream_window_override_mib() {
        Some(mib) => mib * 1024 * 1024,
        None => STREAM_RECEIVE_WINDOW_BYTES,
    }
}

pub(crate) fn blob_transport_profile() -> BlobTransportProfile {
    // Must report the window in force, not the compiled-in default, or the
    // telemetry disagrees with the transport during a sweep.
    let window = stream_receive_window_bytes();
    BlobTransportProfile::new(
        u64::from(window),
        u64::from(VarInt::MAX),
        8u64 * u64::from(window),
        "cubic",
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
/// - `send_window` — raised to `8 ×` the stream window (mirroring quinn's
///   default send/stream ratio) so the serving side can keep the larger receive
///   window full and isn't the new bottleneck.
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
    // `from_u32` is infallible for our values (both well under VarInt::MAX), and
    // `send_window` is a plain u64 — 8× the stream window.
    let window = stream_receive_window_bytes();
    let stream_receive_window = VarInt::from_u32(window);
    let send_window = 8u64 * u64::from(window);

    QuicTransportConfig::builder()
        .default_path_max_idle_timeout(Duration::from_millis(6_000))
        .default_path_keep_alive_interval(Duration::from_millis(4_500))
        .stream_receive_window(stream_receive_window)
        .send_window(send_window)
        .build()
}

#[cfg(test)]
mod tests {
    use super::{
        STREAM_RECEIVE_WINDOW_BYTES, blob_transport_profile, build_transport_config,
        stream_receive_window_bytes,
    };

    /// Off Android there is no property to read, so the compiled-in value must
    /// survive untouched — the override must never affect desktop builds.
    #[test]
    fn stream_window_defaults_to_the_compiled_in_value() {
        assert_eq!(stream_receive_window_bytes(), STREAM_RECEIVE_WINDOW_BYTES);
    }

    /// The profile is what telemetry reports; it has to agree with the window
    /// actually in force, or a sweep records the wrong configuration against
    /// its own numbers.
    #[test]
    fn profile_reports_the_window_in_force() {
        use iroh::endpoint::VarInt;
        use wisp_core::blobs::receive::BlobTransportProfile;

        let window = u64::from(stream_receive_window_bytes());
        assert_eq!(
            blob_transport_profile(),
            BlobTransportProfile::new(window, u64::from(VarInt::MAX), 8 * window, "cubic"),
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
