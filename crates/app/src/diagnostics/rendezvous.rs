use std::time::{Duration, Instant};

use super::{CheckGroup, CheckResult, CheckStatus};

const HEALTH_TIMEOUT: Duration = Duration::from_secs(3);
const ID: &str = "rendezvous.health";

pub(super) async fn check_health(server_url: &str) -> CheckResult {
    let trimmed = server_url.trim().trim_end_matches('/');
    let url = format!("{trimmed}/healthz");
    let parsed = match reqwest::Url::parse(&url) {
        Ok(url) => url,
        Err(error) => {
            return CheckResult {
                id: ID.to_owned(),
                group: CheckGroup::Rendezvous,
                status: CheckStatus::Fail,
                label: "Rendezvous URL invalid".to_owned(),
                detail: format!("Could not parse `{server_url}`: {error}"),
                hint: Some("Open Settings → Discovery Server and fix or clear the URL.".to_owned()),
                action: None,
            };
        }
    };

    let client = match reqwest::Client::builder().timeout(HEALTH_TIMEOUT).build() {
        Ok(client) => client,
        Err(error) => {
            return CheckResult {
                id: ID.to_owned(),
                group: CheckGroup::Rendezvous,
                status: CheckStatus::Fail,
                label: "Server /healthz".to_owned(),
                detail: format!("Couldn't build HTTP client: {error}"),
                hint: None,
                action: None,
            };
        }
    };

    let started = Instant::now();
    let response = client.get(parsed).send().await;
    let elapsed = started.elapsed();

    match response {
        Ok(resp) if resp.status().is_success() => CheckResult {
            id: ID.to_owned(),
            group: CheckGroup::Rendezvous,
            status: CheckStatus::Pass,
            label: "Server /healthz".to_owned(),
            detail: format!("{} ms", elapsed.as_millis()),
            hint: None,
            action: None,
        },
        Ok(resp) => CheckResult {
            id: ID.to_owned(),
            group: CheckGroup::Rendezvous,
            status: CheckStatus::Fail,
            label: "Server /healthz".to_owned(),
            detail: format!("HTTP {} from rendezvous", resp.status().as_u16()),
            hint: Some(
                "The rendezvous server is reachable but isn't returning a healthy response. \
                 Code-based pairing may fail."
                    .to_owned(),
            ),
            action: None,
        },
        Err(error) => {
            let timed_out = error.is_timeout() || elapsed >= HEALTH_TIMEOUT;
            CheckResult {
                id: ID.to_owned(),
                group: CheckGroup::Rendezvous,
                status: CheckStatus::Fail,
                label: "Server /healthz".to_owned(),
                detail: if timed_out {
                    format!("Timed out after {} ms", elapsed.as_millis())
                } else {
                    error.to_string()
                },
                hint: Some(
                    "Code-based pairing won't work until the rendezvous server is reachable. \
                     QR-code pairing still works on the same LAN."
                        .to_owned(),
                ),
                action: None,
            }
        }
    }
}
