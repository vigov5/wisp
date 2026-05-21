use std::collections::BTreeMap;
use std::time::Duration;

use crate::error::{AppError, AppResult};
use crate::types::NearbyReceiver;

pub async fn scan_nearby_receivers(timeout_secs: u64) -> AppResult<Vec<NearbyReceiver>> {
    let secs = timeout_secs.max(1);
    let receivers = tokio::task::spawn_blocking(move || {
        wisp_core::lan::browse_nearby_receivers(Duration::from_secs(secs), None)
    })
    .await
    .map_err(|_| AppError::DiscoveryFailed)?
    .map_err(|_| AppError::DiscoveryFailed)?;

    let mut by_fullname = BTreeMap::new();
    for receiver in receivers {
        // Best effort: extract EndpointId from the ticket so the UI can show a
        // pubkey-derived color/badge. A malformed ticket leaves it empty —
        // dial would have failed anyway, so the empty pubkey is informational.
        let endpoint_id = match wisp_core::util::decode_ticket(&receiver.ticket) {
            Ok(addr) => addr.id.to_string(),
            Err(err) => {
                tracing::debug!(
                    target: "wisp_app::nearby",
                    receiver = %receiver.fullname,
                    error = %err,
                    "decode_ticket failed for nearby receiver — pubkey badge will be empty"
                );
                String::new()
            }
        };
        by_fullname.insert(
            receiver.fullname.clone(),
            NearbyReceiver {
                fullname: receiver.fullname,
                label: receiver.label,
                device_type: match receiver.device_type {
                    wisp_core::protocol::DeviceType::Phone => "phone".to_owned(),
                    wisp_core::protocol::DeviceType::Laptop => "laptop".to_owned(),
                },
                code: receiver.code,
                ticket: receiver.ticket,
                endpoint_id,
            },
        );
    }

    Ok(by_fullname.into_values().collect())
}
