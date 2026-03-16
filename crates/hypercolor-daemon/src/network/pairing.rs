use std::collections::HashMap;
use std::net::IpAddr;

use hypercolor_driver_api::DriverHost;
use hypercolor_types::device::DeviceId;
use tracing::warn;

pub(super) async fn activate_if_requested(
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

pub(super) async fn disconnect_after_unpair(
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

pub(super) fn metadata_value<'a>(
    metadata: Option<&'a HashMap<String, String>>,
    key: &str,
) -> Option<&'a str> {
    metadata
        .and_then(|values| values.get(key))
        .map(String::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

pub(super) fn network_ip_from_metadata(
    metadata: Option<&HashMap<String, String>>,
) -> Option<IpAddr> {
    metadata
        .and_then(|values| values.get("ip"))
        .and_then(|value| value.parse::<IpAddr>().ok())
}

pub(super) fn push_lookup_key(keys: &mut Vec<String>, key: String) {
    if !keys.iter().any(|existing| existing == &key) {
        keys.push(key);
    }
}
