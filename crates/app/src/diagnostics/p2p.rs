use std::time::Duration;

use iroh::Endpoint;

use super::{CheckGroup, CheckResult, CheckStatus};

const ID: &str = "p2p.transport";
const ONLINE_TIMEOUT: Duration = Duration::from_secs(5);

/// Strips scheme + trailing slash so a relay URL like
/// `https://aps1-1.relay.n0.iroh-canary.iroh.link./` becomes
/// `aps1-1.relay.n0.iroh-canary.iroh.link` — short enough to fit one
/// line in the diagnostic detail row without ellipsizing.
fn shorten_relay_url(url: &str) -> String {
    url.strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url)
        .split('/')
        .next()
        .unwrap_or(url)
        .trim_end_matches('.')
        .to_owned()
}

/// Verifies the iroh transport stack is up.
///
/// We can't actually self-dial — iroh's QUIC layer rejects `connect()` to one's
/// own endpoint id with "Connecting to ourself is not supported" (confirmed in
/// `tests::diag_self_dial`).  Instead we verify what's actually observable on
/// the live endpoint:
///
/// 1. UDP sockets are bound.
/// 2. At least one non-loopback local IP is in the endpoint addr (so peers
///    on the LAN have something to dial).
/// 3. The relay handshake completed within `ONLINE_TIMEOUT` (so internet
///    P2P fallback works).  This is the closest substitute we have for "the
///    iroh stack actually works end-to-end".
pub(crate) async fn check_loopback(endpoint: Option<Endpoint>) -> CheckResult {
    let Some(endpoint) = endpoint else {
        return CheckResult::skipped(
            ID,
            CheckGroup::P2p,
            "iroh transport",
            "Receiver service not running — open the Receive tab once to bring up the \
             iroh stack.",
        );
    };

    let bound = endpoint.bound_sockets();
    if bound.is_empty() {
        return CheckResult {
            id: ID.to_owned(),
            group: CheckGroup::P2p,
            status: CheckStatus::Fail,
            label: "iroh endpoint has no bound sockets".to_owned(),
            detail: "Transport layer didn't bind to any UDP socket.".to_owned(),
            hint: Some(
                "Restart the app or check that nothing else is holding the UDP port.".to_owned(),
            ),
            action: None,
        };
    }

    let addr = endpoint.addr();
    let routable_ips: Vec<_> = addr
        .ip_addrs()
        .filter(|ip| !ip.ip().is_loopback() && !ip.ip().is_unspecified())
        .collect();

    let online_outcome = tokio::time::timeout(ONLINE_TIMEOUT, endpoint.online()).await;
    let relay_url = addr
        .relay_urls()
        .next()
        .map(|u| shorten_relay_url(u.as_str()));

    match online_outcome {
        Ok(()) if !routable_ips.is_empty() && relay_url.is_some() => CheckResult {
            id: ID.to_owned(),
            group: CheckGroup::P2p,
            status: CheckStatus::Pass,
            label: "iroh transport ready".to_owned(),
            detail: format!(
                "{} socket{} bound · {} LAN IP{} · relay {}",
                bound.len(),
                if bound.len() == 1 { "" } else { "s" },
                routable_ips.len(),
                if routable_ips.len() == 1 { "" } else { "s" },
                relay_url.as_deref().unwrap_or("?"),
            ),
            hint: None,
            action: None,
        },
        Ok(()) if relay_url.is_none() => CheckResult {
            id: ID.to_owned(),
            group: CheckGroup::P2p,
            status: CheckStatus::Warn,
            label: "iroh online but no relay URL".to_owned(),
            detail: format!(
                "{} socket{} bound · {} LAN IP{} · relay unknown",
                bound.len(),
                if bound.len() == 1 { "" } else { "s" },
                routable_ips.len(),
                if routable_ips.len() == 1 { "" } else { "s" },
            ),
            hint: Some(
                "LAN transfers still work, but peers off the LAN may not be reachable \
                 until relay discovery completes."
                    .to_owned(),
            ),
            action: None,
        },
        Ok(()) => CheckResult {
            id: ID.to_owned(),
            group: CheckGroup::P2p,
            status: CheckStatus::Warn,
            label: "No LAN-routable IP".to_owned(),
            detail: format!(
                "{} socket{} bound · relay {}",
                bound.len(),
                if bound.len() == 1 { "" } else { "s" },
                relay_url.as_deref().unwrap_or("?"),
            ),
            hint: Some(
                "Endpoint has no non-loopback local IP. Direct LAN dials will fail; \
                 transfers will fall back to relay."
                    .to_owned(),
            ),
            action: None,
        },
        Err(_) => CheckResult {
            id: ID.to_owned(),
            group: CheckGroup::P2p,
            status: CheckStatus::Warn,
            label: "iroh relay handshake timed out".to_owned(),
            detail: format!(
                "No relay handshake within {} ms · {} socket{} bound · {} LAN IP{}",
                ONLINE_TIMEOUT.as_millis(),
                bound.len(),
                if bound.len() == 1 { "" } else { "s" },
                routable_ips.len(),
                if routable_ips.len() == 1 { "" } else { "s" },
            ),
            hint: Some(
                "iroh couldn't reach its relay server. LAN transfers should still work; \
                 internet P2P may fail until connectivity recovers."
                    .to_owned(),
            ),
            action: None,
        },
    }
}

// ---------------------------------------------------------------------------
// Diagnostic isolation tests.
//
// These are opt-in (`#[ignore]`) because they need real UDP + relay reachability.
// They exist to disambiguate "iroh self-dial doesn't work" from "env blocks
// iroh entirely" when the production loopback check (`check_loopback`) fails
// in the live app.
//
// Run with:
//     cargo test -p wisp-app --test=lib diagnostics::p2p::tests -- --ignored --nocapture
//
// Interpretation matrix:
//   self_dial PASS + cross_dial PASS → iroh works; production failure is
//       elsewhere (live receiver endpoint state, ALPN routing, etc).
//   self_dial FAIL + cross_dial PASS → iroh refuses self-loopback in this env;
//       reframe the production check to only verify `bound_sockets()` rather
//       than performing an actual self-handshake.
//   self_dial FAIL + cross_dial FAIL → env-level issue (UDP / firewall / relay
//       all blocked); iroh transport cannot work here at all.
//   self_dial PASS + cross_dial FAIL → unusual; relay path may be broken while
//       direct UDP works.
// ---------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use iroh::endpoint::Connection;
    use iroh::protocol::{AcceptError, ProtocolHandler, Router};
    use iroh::{Endpoint, SecretKey};
    use wisp_core::protocol::ALPN;

    const TEST_TIMEOUT: Duration = Duration::from_secs(15);
    const ONLINE_TIMEOUT: Duration = Duration::from_secs(8);

    #[derive(Debug, Clone)]
    struct AcceptHandler;

    impl ProtocolHandler for AcceptHandler {
        async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
            let _ = connection.closed().await;
            Ok(())
        }
    }

    async fn try_bind() -> Option<Endpoint> {
        match Endpoint::builder(iroh::endpoint::presets::N0)
            .secret_key(SecretKey::from_bytes(&rand::random()))
            .alpns(vec![ALPN.to_vec()])
            .bind()
            .await
        {
            Ok(endpoint) => Some(endpoint),
            Err(error) => {
                let chain = format!("{error:#}");
                if chain.contains("Failed to bind sockets")
                    || chain.contains("Operation not permitted")
                {
                    eprintln!("SKIP: sandbox cannot bind iroh sockets — {chain}");
                    None
                } else {
                    panic!("unexpected bind failure: {chain}");
                }
            }
        }
    }

    /// Regression: iroh refuses self-loopback. This test pins that behavior so
    /// production code never tries `endpoint.connect(endpoint.addr(), ALPN)`
    /// expecting it to succeed.
    #[tokio::test]
    #[ignore = "requires real UDP + relay; run with --ignored --nocapture"]
    #[should_panic(expected = "Connecting to ourself is not supported")]
    async fn diag_self_dial() {
        let Some(endpoint) = try_bind().await else {
            return;
        };
        let router = Router::builder(endpoint.clone())
            .accept(ALPN, AcceptHandler)
            .spawn();

        let _ = tokio::time::timeout(ONLINE_TIMEOUT, endpoint.online()).await;
        let addr = endpoint.addr();
        eprintln!(
            "self-dial: target addr id={} direct={} relay={:?}",
            addr.id,
            addr.ip_addrs().count(),
            addr.relay_urls().next(),
        );

        let started = Instant::now();
        let outcome =
            tokio::time::timeout(TEST_TIMEOUT, endpoint.connect(addr.clone(), ALPN)).await;
        let elapsed = started.elapsed();
        let _ = router.shutdown().await;

        match outcome {
            Ok(Ok(_)) => eprintln!("SELF-DIAL PASS in {} ms", elapsed.as_millis()),
            Ok(Err(error)) => {
                panic!("SELF-DIAL FAIL after {} ms: {error:#}", elapsed.as_millis())
            }
            Err(_) => panic!("SELF-DIAL TIMEOUT after {} ms", elapsed.as_millis()),
        }
    }

    #[tokio::test]
    #[ignore = "requires real UDP + relay; run with --ignored --nocapture"]
    async fn diag_cross_dial() {
        let Some(alice) = try_bind().await else {
            return;
        };
        let Some(bob) = try_bind().await else { return };
        let router = Router::builder(alice.clone())
            .accept(ALPN, AcceptHandler)
            .spawn();

        let _ = tokio::time::timeout(ONLINE_TIMEOUT, alice.online()).await;
        let _ = tokio::time::timeout(ONLINE_TIMEOUT, bob.online()).await;
        let alice_addr = alice.addr();
        eprintln!(
            "cross-dial: alice id={} direct={} relay={:?}",
            alice_addr.id,
            alice_addr.ip_addrs().count(),
            alice_addr.relay_urls().next(),
        );

        let started = Instant::now();
        let outcome =
            tokio::time::timeout(TEST_TIMEOUT, bob.connect(alice_addr.clone(), ALPN)).await;
        let elapsed = started.elapsed();
        let _ = router.shutdown().await;

        match outcome {
            Ok(Ok(_)) => eprintln!("CROSS-DIAL PASS in {} ms", elapsed.as_millis()),
            Ok(Err(error)) => {
                panic!(
                    "CROSS-DIAL FAIL after {} ms: {error:#}",
                    elapsed.as_millis()
                )
            }
            Err(_) => panic!("CROSS-DIAL TIMEOUT after {} ms", elapsed.as_millis()),
        }
    }
}
