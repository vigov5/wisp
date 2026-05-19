use std::time::{Duration, Instant};

use super::{CheckGroup, CheckResult, CheckStatus};

const PROBE_URL: &str = "https://www.gstatic.com/generate_204";
const PROBE_TIMEOUT: Duration = Duration::from_secs(2);

pub(super) async fn check_internet() -> CheckResult {
    let client = match reqwest::Client::builder().timeout(PROBE_TIMEOUT).build() {
        Ok(client) => client,
        Err(error) => {
            return CheckResult {
                id: "network.internet".to_owned(),
                group: CheckGroup::Network,
                status: CheckStatus::Fail,
                label: "Internet reachable".to_owned(),
                detail: format!("Couldn't build HTTP client: {error}"),
                hint: None,
                action: None,
            };
        }
    };

    let started = Instant::now();
    let response = client.head(PROBE_URL).send().await;
    let elapsed = started.elapsed();

    match response {
        Ok(resp) if resp.status().is_success() || resp.status().as_u16() == 204 => CheckResult {
            id: "network.internet".to_owned(),
            group: CheckGroup::Network,
            status: CheckStatus::Pass,
            label: "Internet reachable".to_owned(),
            detail: format!("{} ms", elapsed.as_millis()),
            hint: None,
            action: None,
        },
        Ok(resp) => CheckResult {
            id: "network.internet".to_owned(),
            group: CheckGroup::Network,
            status: CheckStatus::Warn,
            label: "Internet reachable".to_owned(),
            detail: format!("Unexpected HTTP {}", resp.status().as_u16()),
            hint: Some(
                "Internet appears reachable but a captive portal or proxy may \
                 be intercepting requests."
                    .to_owned(),
            ),
            action: None,
        },
        Err(error) => {
            let timed_out = error.is_timeout() || elapsed >= PROBE_TIMEOUT;
            CheckResult {
                id: "network.internet".to_owned(),
                group: CheckGroup::Network,
                status: CheckStatus::Fail,
                label: "Internet unreachable".to_owned(),
                detail: if timed_out {
                    format!("Timed out after {} ms", elapsed.as_millis())
                } else {
                    error.to_string()
                },
                hint: Some(
                    "Check Wi-Fi or mobile data. Code-based pairing needs a working internet \
                     connection."
                        .to_owned(),
                ),
                action: None,
            }
        }
    }
}
