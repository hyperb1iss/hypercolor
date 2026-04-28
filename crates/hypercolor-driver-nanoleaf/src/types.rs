//! Nanoleaf API and discovery types.

use std::collections::HashMap;
use std::net::IpAddr;

use serde::Deserialize;

use hypercolor_driver_api::{DiscoveredDevice, DiscoveryConnectBehavior};
use hypercolor_types::device::{
    ConnectionType, DeviceCapabilities, DeviceColorFormat, DeviceFamily, DeviceFeatures,
    DeviceFingerprint, DeviceInfo, DeviceOrigin, DeviceTopologyHint, ZoneInfo,
};

use super::topology::NanoleafShapeType;

/// Top-level Nanoleaf device info returned by `GET /api/v1/{token}`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NanoleafDeviceInfo {
    pub name: String,
    pub model: String,
    pub serial_no: String,
    pub firmware_version: String,
}

/// Panel layout returned by `GET /api/v1/{token}/panelLayout/layout`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NanoleafPanelLayoutResponse {
    #[serde(default)]
    pub position_data: Vec<NanoleafPanelLayout>,
}

/// One Nanoleaf panel/controller position entry.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NanoleafPanelLayout {
    pub panel_id: u16,
    #[serde(default)]
    pub x: i32,
    #[serde(default)]
    pub y: i32,
    #[serde(default)]
    pub o: i16,
    pub shape_type: u8,
}

impl NanoleafPanelLayout {
    /// Parsed shape type when Hypercolor recognizes the raw ID.
    #[must_use]
    pub fn shape_kind(&self) -> Option<NanoleafShapeType> {
        NanoleafShapeType::from_raw(self.shape_type)
    }

    /// Whether this layout entry should become an addressable zone.
    #[must_use]
    pub fn has_leds(&self) -> bool {
        self.shape_kind().is_some_and(NanoleafShapeType::has_leds)
    }

    /// Hypercolor topology hint for this panel, defaulting to `Point`.
    #[must_use]
    pub fn topology_hint(&self) -> DeviceTopologyHint {
        self.shape_kind().map_or(
            DeviceTopologyHint::Point,
            NanoleafShapeType::to_topology_hint,
        )
    }
}

/// Rich Nanoleaf discovery result shared between the scanner and backend.
#[derive(Debug, Clone)]
pub struct NanoleafDiscoveredDevice {
    pub device_key: String,
    pub ip: IpAddr,
    pub api_port: u16,
    pub info: DeviceInfo,
    pub panel_ids: Vec<u16>,
    pub connect_behavior: DiscoveryConnectBehavior,
    pub metadata: HashMap<String, String>,
}

impl NanoleafDiscoveredDevice {
    /// Convert into the generic discovery representation.
    #[must_use]
    pub fn into_discovered(self) -> DiscoveredDevice {
        let fingerprint = DeviceFingerprint(format!("nanoleaf:{}", self.device_key));

        DiscoveredDevice {
            connection_type: ConnectionType::Network,
            origin: self.info.origin.clone(),
            name: self.info.name.clone(),
            family: DeviceFamily::new_static("nanoleaf", "Nanoleaf"),
            fingerprint,
            connect_behavior: self.connect_behavior,
            info: self.info,
            metadata: self.metadata,
        }
    }
}

/// Build `DeviceInfo` from Nanoleaf metadata and panel layout.
#[must_use]
pub fn build_device_info(
    device_key: &str,
    name: &str,
    model: Option<&str>,
    firmware: Option<&str>,
    panels: &[NanoleafPanelLayout],
) -> DeviceInfo {
    let fingerprint = DeviceFingerprint(format!("nanoleaf:{device_key}"));
    let device_id = fingerprint.stable_device_id();

    let zones: Vec<ZoneInfo> = panels
        .iter()
        .filter(|panel| panel.has_leds())
        .map(|panel| ZoneInfo {
            name: format!("Panel {}", panel.panel_id),
            led_count: 1,
            topology: panel.topology_hint(),
            color_format: DeviceColorFormat::Rgb,
        })
        .collect();
    let panel_count = u32::try_from(zones.len()).unwrap_or(u32::MAX);

    DeviceInfo {
        id: device_id,
        name: if name.is_empty() {
            format!("Nanoleaf {device_key}")
        } else {
            name.to_owned()
        },
        vendor: "Nanoleaf".to_owned(),
        family: DeviceFamily::new_static("nanoleaf", "Nanoleaf"),
        model: model
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned),
        connection_type: ConnectionType::Network,
        origin: DeviceOrigin::native("nanoleaf", "nanoleaf", ConnectionType::Network),
        zones,
        firmware_version: firmware
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned),
        capabilities: DeviceCapabilities {
            led_count: panel_count,
            supports_direct: true,
            supports_brightness: true,
            has_display: false,
            display_resolution: None,
            max_fps: 10,
            color_space: hypercolor_types::device::DeviceColorSpace::default(),
            features: DeviceFeatures::default(),
        },
    }
}

/// Extract the ordered list of addressable panel IDs from a layout response.
#[must_use]
pub fn panel_ids_from_layout(panels: &[NanoleafPanelLayout]) -> Vec<u16> {
    panels
        .iter()
        .filter(|panel| panel.has_leds())
        .map(|panel| panel.panel_id)
        .collect()
}
