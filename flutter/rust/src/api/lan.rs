//! mDNS LAN discovery for Flutter send UI.

use super::RUNTIME;
use wisp_app::NearbyReceiver;

#[derive(Debug, Clone)]
pub struct NearbyReceiverInfo {
    pub fullname: String,
    pub label: String,
    pub device_type: String,
    pub code: String,
    pub ticket: String,
    pub endpoint_id: String,
    /// True when the receiver advertises a USB-cable address (AOA tunnel or
    /// USB-tether), so the send UI can badge its nearby tile as a cable peer.
    pub over_usb: bool,
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
        over_usb: item.over_usb,
    }
}

/// Heuristic info about a detected USB-tethering (IP-over-USB) link, surfaced
/// to the Flutter UI so it can confirm a cable link is present and guide setup.
#[derive(Debug, Clone)]
pub struct UsbLinkData {
    pub local_ip: String,
    /// True when this device is the tethering host (the phone).
    pub is_host: bool,
    /// The peer's gateway IP (the phone) when inferable, else null.
    pub gateway_ip: Option<String>,
}

/// Detect a likely USB-tethering link on a local interface (no network I/O).
/// Returns null when no USB-over-IP link is present.
#[flutter_rust_bridge::frb(sync)]
pub fn detect_usb_link() -> Option<UsbLinkData> {
    wisp_core::lan::detect_usb_link().map(|l| UsbLinkData {
        local_ip: l.local_ip.to_string(),
        is_host: l.is_host,
        gateway_ip: l.gateway_ip.map(|ip| ip.to_string()),
    })
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
    wisp_core::util::decode_ticket_info(&ticket)
        .map(|info| DecodedTicketData {
            endpoint_id: info.endpoint_addr.id.to_string(),
            device_name: info.device_name,
            device_type: info.device_type,
        })
        .map_err(|e| {
            crate::api::error::internal_user_facing_error("Couldn't read QR ticket", e.to_string())
        })
}
