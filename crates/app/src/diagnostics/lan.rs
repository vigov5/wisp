use std::net::{IpAddr, Ipv4Addr};
use std::time::{Duration, Instant};

use flume::RecvTimeoutError;
use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};

use super::{CheckGroup, CheckResult, CheckStatus};

const SERVICE_TYPE: &str = "_wisp-diag._udp.local.";
const SCAN_TIMEOUT: Duration = Duration::from_secs(3);
const POLL_INTERVAL: Duration = Duration::from_millis(150);
const ID: &str = "lan.self_scan";

pub(super) async fn check_self_scan() -> CheckResult {
    let instance = format!("wisp-diag-{:08x}", rand::random::<u32>());
    let result = tokio::task::spawn_blocking(move || run_self_scan(&instance)).await;
    match result {
        Ok(Ok(latency)) => CheckResult {
            id: ID.to_owned(),
            group: CheckGroup::Lan,
            status: CheckStatus::Pass,
            label: "mDNS self-scan".to_owned(),
            detail: format!("Saw own advertisement in {} ms", latency.as_millis()),
            hint: None,
            action: None,
        },
        Ok(Err(error)) => CheckResult {
            id: ID.to_owned(),
            group: CheckGroup::Lan,
            status: CheckStatus::Fail,
            label: "mDNS self-scan failed".to_owned(),
            detail: error,
            hint: Some(
                "Wi-Fi may be in client isolation mode, or mDNS is blocked. On iOS, \
                 ensure Local Network permission is granted for Wisp."
                    .to_owned(),
            ),
            action: None,
        },
        Err(join_error) => CheckResult {
            id: ID.to_owned(),
            group: CheckGroup::Lan,
            status: CheckStatus::Warn,
            label: "mDNS self-scan".to_owned(),
            detail: format!("Probe task panicked: {join_error}"),
            hint: None,
            action: None,
        },
    }
}

fn run_self_scan(instance: &str) -> Result<Duration, String> {
    let daemon = ServiceDaemon::new().map_err(|e| format!("Couldn't create mDNS daemon: {e}"))?;

    let service = ServiceInfo::new(
        SERVICE_TYPE,
        instance,
        "wisp-diag.local.",
        IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)),
        47480,
        &[("probe", "self-scan")] as &[(&str, &str)],
    )
    .map_err(|e| format!("Couldn't build mDNS service info: {e}"))?
    .enable_addr_auto();

    let fullname = service.get_fullname().to_owned();

    let browse_rx = daemon
        .browse(SERVICE_TYPE)
        .map_err(|e| format!("Couldn't start mDNS browse: {e}"))?;

    daemon
        .register(service)
        .map_err(|e| format!("Couldn't register mDNS service: {e}"))?;

    let started = Instant::now();
    let deadline = started + SCAN_TIMEOUT;
    let mut latency: Option<Duration> = None;

    while Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let wait = POLL_INTERVAL.min(remaining);
        if wait.is_zero() {
            break;
        }
        match browse_rx.recv_timeout(wait) {
            Ok(ServiceEvent::ServiceResolved(info)) => {
                if info.get_fullname() == fullname {
                    latency = Some(started.elapsed());
                    break;
                }
            }
            Ok(ServiceEvent::ServiceFound(_ty, name)) => {
                if name == fullname {
                    latency = Some(started.elapsed());
                    break;
                }
            }
            Ok(_) => {}
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => {
                return Err("mDNS browse channel disconnected".to_owned());
            }
        }
    }

    let _ = daemon.unregister(&fullname);
    let _ = daemon.shutdown();

    match latency {
        Some(d) => Ok(d),
        None => Err(format!(
            "Did not see own advertisement within {} ms",
            SCAN_TIMEOUT.as_millis()
        )),
    }
}
