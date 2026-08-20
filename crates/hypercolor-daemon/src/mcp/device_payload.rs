use crate::api::AppState;
use crate::mcp::results::DeviceInventoryItem;
use hypercolor_types::device::{DeviceInfo, DeviceState};

pub(crate) fn inventory_device_payload(
    state: &AppState,
    info: &DeviceInfo,
    device_state: &DeviceState,
) -> DeviceInventoryItem {
    DeviceInventoryItem {
        id: info.id.to_string(),
        name: info.name.clone(),
        vendor: info.vendor.clone(),
        family: info.family.to_string(),
        origin: info.origin.clone(),
        presentation: crate::network::device_presentation(state.driver_registry.as_ref(), info),
        transport: info.origin.transport.as_id().to_owned(),
        state: device_state.clone(),
        led_count: info.total_led_count(),
        segments: info.segments.len(),
    }
}
