use serde_json::{Value, json};

use crate::api::AppState;
use hypercolor_types::device::{DeviceInfo, DeviceState};

pub(crate) fn inventory_device_payload(
    state: &AppState,
    info: &DeviceInfo,
    device_state: &DeviceState,
) -> Value {
    json!({
        "id": info.id.to_string(),
        "name": &info.name,
        "vendor": &info.vendor,
        "family": info.family.to_string(),
        "origin": &info.origin,
        "presentation": crate::network::device_presentation(state.driver_registry.as_ref(), info),
        "transport": info.origin.transport.as_id(),
        "state": device_state.variant_name(),
        "led_count": info.total_led_count(),
        "zones": info.zones.len()
    })
}
