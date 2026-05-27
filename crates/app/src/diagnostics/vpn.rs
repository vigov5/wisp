//! Detects an active VPN / tunnel interface that can hijack iroh's transport.
//!
//! Symptom this catches (observed on a corporate full-tunnel VPN + Windows):
//! iroh binds `0.0.0.0`, so the OS routing table — now owned by the VPN —
//! sends relay/QUIC datagrams out the tunnel interface (`src_ip` = the VPN
//! address).  Two things then break:
//!
//!   1. The tunnel's reduced MTU rejects iroh's ~1389-byte relay frames
//!      (`WSAEMSGSIZE`), so the relay path dies.
//!   2. Full-tunnel routing swallows even same-subnet LAN traffic, so the
//!      direct path to a peer on the same Wi-Fi dies too.
//!
//! Net result: the control handshake may squeak through but the data
//! transfer stalls, and the only reliable fix today is disconnecting the
//! VPN.  We can't reconfigure iroh's binding safely at runtime (static
//! per-interface binds don't survive network changes), so we surface a
//! Warn telling the user what to do.

use super::{CheckGroup, CheckResult, CheckStatus};

const ID: &str = "p2p.vpn";

/// VPN/tunnel interfaces whose presence we want to warn about.
struct TunnelIface {
    label: String,
    ips: Vec<String>,
    mtu: Option<u32>,
}

pub(super) async fn check_vpn_interference() -> CheckResult {
    let tunnels = tokio::task::spawn_blocking(detect_tunnel_interfaces)
        .await
        .unwrap_or_default();

    if tunnels.is_empty() {
        return CheckResult {
            id: ID.to_owned(),
            group: CheckGroup::P2p,
            status: CheckStatus::Pass,
            label: "No VPN tunnel active".to_owned(),
            detail: "No VPN/tunnel interface detected.".to_owned(),
            hint: None,
            action: None,
        };
    }

    let names: Vec<String> = tunnels.iter().map(|t| t.label.clone()).collect();
    // A tunnel MTU below the ~1400 floor needed for iroh's relay frames is a
    // strong signal the relay path will fail outright; call it out if known.
    let low_mtu = tunnels.iter().filter_map(|t| t.mtu).any(|mtu| mtu < 1400);

    let detail = {
        let mut parts = Vec::new();
        for t in &tunnels {
            let ips = if t.ips.is_empty() {
                String::new()
            } else {
                format!(" ({})", t.ips.join(", "))
            };
            let mtu = t.mtu.map(|m| format!(" MTU {m}")).unwrap_or_default();
            parts.push(format!("{}{ips}{mtu}", t.label));
        }
        parts.join("; ")
    };

    CheckResult {
        id: ID.to_owned(),
        group: CheckGroup::P2p,
        status: CheckStatus::Warn,
        label: if names.len() == 1 {
            "VPN tunnel active".to_owned()
        } else {
            format!("{} VPN tunnels active", names.len())
        },
        detail,
        hint: Some(
            if low_mtu {
                "A VPN tunnel with a small MTU is active. It can route Wisp's traffic through \
                 the tunnel and break both the direct-LAN and relay paths. If transfers stall \
                 or fail, disconnect the VPN and try again."
            } else {
                "A VPN tunnel is active. A full-tunnel VPN can route Wisp's traffic through the \
                 tunnel and break direct-LAN and relay paths. If transfers stall or fail, \
                 disconnect the VPN and try again."
            }
            .to_owned(),
        ),
        action: None,
    }
}

/// Enumerates interfaces and returns those that look like a VPN / tunnel.
///
/// Excludes WSL/Hyper-V/Docker virtual switches: they aren't tunnels and
/// don't capture the default route, so they don't cause the failure mode
/// above (the sender-side presence filter already prunes their unreachable
/// IPs from a dial).
fn detect_tunnel_interfaces() -> Vec<TunnelIface> {
    netdev::get_interfaces()
        .into_iter()
        .filter(|iface| !iface.ipv4.is_empty())
        .filter(is_tunnel_like)
        .map(|iface| {
            let label = iface
                .friendly_name
                .clone()
                .or_else(|| iface.description.clone())
                .unwrap_or_else(|| iface.name.clone());
            TunnelIface {
                label,
                ips: iface
                    .ipv4
                    .iter()
                    .map(|net| net.addr().to_string())
                    .collect(),
                mtu: iface.mtu,
            }
        })
        .collect()
}

fn is_tunnel_like(iface: &netdev::Interface) -> bool {
    use netdev::interface::types::InterfaceType;

    if matches!(iface.if_type, InterfaceType::Tunnel | InterfaceType::Ppp) {
        return true;
    }

    // Some VPN products register as a plain Ethernet adapter; match on the
    // adapter's friendly name / description instead.  Markers are matched
    // case-insensitively as substrings.
    const MARKERS: &[&str] = &[
        "vpn",
        "wireguard",
        "wintun",
        "openvpn",
        "tap-windows",
        "tunnel",
        "anyconnect",
        "globalprotect",
        "pangp",
        "zscaler",
        "tailscale",
        "nordlynx",
        "forticlient",
        "ipsec",
        "softether",
        "zerotier",
    ];
    let haystack = format!(
        "{} {}",
        iface.friendly_name.as_deref().unwrap_or(""),
        iface.description.as_deref().unwrap_or("")
    )
    .to_lowercase();
    MARKERS.iter().any(|m| haystack.contains(m))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marker_match_is_case_insensitive_substring() {
        // Sanity-check the substring logic the real detector relies on.
        let hay = "Ethernet 5 WireGuard Tunnel".to_lowercase();
        assert!(hay.contains("wireguard"));
        assert!(!hay.contains("openvpn"));
    }

    #[tokio::test]
    async fn check_runs_and_returns_p2p_group() {
        // Smoke test: the check executes against the real host without
        // panicking and is classified under the P2p group.  Status depends
        // on whether the host running the test has a VPN up, so we don't
        // assert on it.
        let result = check_vpn_interference().await;
        assert_eq!(result.group, CheckGroup::P2p);
        assert_eq!(result.id, ID);
    }
}
