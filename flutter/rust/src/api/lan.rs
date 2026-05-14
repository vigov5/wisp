//! mDNS LAN discovery for Flutter send UI.

use super::RUNTIME;
use drift_app::NearbyReceiver;

#[derive(Debug, Clone)]
pub struct NearbyReceiverInfo {
    pub fullname: String,
    pub label: String,
    pub device_type: String,
    pub code: String,
    pub ticket: String,
    pub endpoint_id: String,
}

pub fn scan_nearby_receivers(
    timeout_secs: u64,
) -> Result<Vec<NearbyReceiverInfo>, crate::api::error::UserFacingErrorData> {
    RUNTIME.block_on(super::receiver::scan_nearby_with_receiver(timeout_secs))
}

pub(crate) fn map_nearby_receiver(item: NearbyReceiver) -> NearbyReceiverInfo {
    NearbyReceiverInfo {
        fullname: item.fullname,
        label: item.label,
        device_type: item.device_type,
        code: item.code,
        ticket: item.ticket,
        endpoint_id: item.endpoint_id,
    }
}

#[derive(Debug, Clone)]
pub struct DecodedTicketData {
    pub endpoint_id: String,
    pub device_name: String,
    pub device_type: String,
}

/// Pure decode (no network) of a ticket string. Used by the QR scanner so
/// the sender UI can show the receiver's name/type/pubkey before dialing.
#[flutter_rust_bridge::frb(sync)]
pub fn decode_ticket_info(
    ticket: String,
) -> Result<DecodedTicketData, crate::api::error::UserFacingErrorData> {
    drift_core::util::decode_ticket_info(&ticket)
        .map(|info| DecodedTicketData {
            endpoint_id: info.endpoint_addr.id.to_string(),
            device_name: info.device_name,
            device_type: info.device_type,
        })
        .map_err(|e| {
            crate::api::error::internal_user_facing_error("Couldn't read QR ticket", e.to_string())
        })
}
