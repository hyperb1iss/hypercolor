//! Zone helpers for Corsair peripheral devices.

use hypercolor_types::device::SegmentInfo;

use hypercolor_types::device::{DeviceColorFormat, DeviceTopologyHint};

use super::types::{BragiDeviceConfig, CorsairPeripheralTopology};

#[must_use]
pub fn zones_for_bragi(config: &BragiDeviceConfig) -> Vec<SegmentInfo> {
    let led_count = u32::try_from(config.led_count).unwrap_or(u32::MAX);
    if led_count == 0 || config.topology == CorsairPeripheralTopology::None {
        return Vec::new();
    }

    let topology = match config.topology {
        CorsairPeripheralTopology::None => DeviceTopologyHint::Strip,
        other => other.hint(),
    };

    vec![SegmentInfo {
        name: config.class.zone_name().to_owned(),
        led_count,
        topology,
        color_format: DeviceColorFormat::Rgb,
        layout_hint: None,
    }]
}
