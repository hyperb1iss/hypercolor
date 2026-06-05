use std::collections::HashMap;
use std::net::IpAddr;

use hypercolor_types::device::DeviceId;
use tracing::warn;

use crate::DriverHost;
use crate::validation::{validate_ip, validate_port};

/// Best-effort immediate activation after pairing.
pub async fn activate_if_requested(
    host: &dyn DriverHost,
    activate_after_pair: bool,
    device_id: DeviceId,
    backend_id: &str,
) -> bool {
    if !activate_after_pair {
        return false;
    }

    match host.runtime().activate_device(device_id, backend_id).await {
        Ok(activated) => activated,
        Err(error) => {
            warn!(
                error = %error,
                device_id = %device_id,
                backend_id = %backend_id,
                "paired device activation failed"
            );
            false
        }
    }
}

/// Best-effort disconnect after credentials are removed.
pub async fn disconnect_after_unpair(
    host: &dyn DriverHost,
    device_id: DeviceId,
    backend_id: &str,
) -> bool {
    match host
        .runtime()
        .disconnect_device(device_id, backend_id, false)
        .await
    {
        Ok(disconnected) => disconnected,
        Err(error) => {
            warn!(
                error = %error,
                device_id = %device_id,
                backend_id = %backend_id,
                "paired device disconnect failed"
            );
            false
        }
    }
}

/// Extract a trimmed metadata value if present and non-empty.
#[must_use]
pub fn metadata_value<'a>(
    metadata: Option<&'a HashMap<String, String>>,
    key: &str,
) -> Option<&'a str> {
    metadata
        .and_then(|values| values.get(key))
        .map(String::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

/// Parse a routable network IP from the standard `ip` metadata key.
///
/// Returns `None` if the key is missing, unparseable, or points at a
/// non-routable address such as loopback, multicast, or broadcast. See
/// [`crate::validation::validate_ip`] for the full list of rejected ranges.
#[must_use]
pub fn network_ip_from_metadata(metadata: Option<&HashMap<String, String>>) -> Option<IpAddr> {
    metadata
        .and_then(|values| values.get("ip"))
        .and_then(|value| value.parse::<IpAddr>().ok())
        .and_then(|ip| validate_ip(ip).ok())
}

/// Parse a validated port from a metadata key.
///
/// Returns `None` if the key is missing, unparseable, or fails
/// [`crate::validation::validate_port`] (port 0 or privileged ports).
#[must_use]
pub fn network_port_from_metadata(
    metadata: Option<&HashMap<String, String>>,
    key: &str,
) -> Option<u16> {
    metadata
        .and_then(|values| values.get(key))
        .and_then(|value| value.parse::<u16>().ok())
        .and_then(|port| validate_port(port).ok())
}

/// Push a credential lookup key if it is not already present.
pub fn push_lookup_key(keys: &mut Vec<String>, key: String) {
    if !keys.iter().any(|existing| existing == &key) {
        keys.push(key);
    }
}
