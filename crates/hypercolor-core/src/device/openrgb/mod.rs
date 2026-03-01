//! `OpenRGB` SDK backend for direct communication with `OpenRGB` servers.
//!
//! This module implements the binary wire protocol (TCP port 6742),
//! controller enumeration, LED color updates, and transport scanning
//! for devices managed by `OpenRGB`.
//!
//! # Architecture
//!
//! ```text
//! OpenRgbScanner (TransportScanner)
//!     |
//!     | TCP probe + enumerate
//!     v
//! OpenRgbClient (SDK wire protocol)
//!     |
//!     | TCP port 6742
//!     v
//! OpenRGB Server
//! ```
//!
//! # Modules
//!
//! - [`proto`] — Binary wire protocol serialization/deserialization
//! - [`client`] — TCP client with handshake, enumeration, and reconnection
//! - [`backend`] — [`DeviceBackend`](super::DeviceBackend) implementation
//! - [`scanner`] — [`TransportScanner`](super::TransportScanner) implementation

pub mod backend;
pub mod client;
pub mod proto;
pub mod scanner;

pub use backend::OpenRgbBackend;
pub use client::{ClientConfig, ConnectionState, OpenRgbClient, ReconnectPolicy};
pub use proto::{
    Command, ControllerData, HEADER_SIZE, MAGIC, PacketHeader, RgbColor, ZoneData, ZoneType,
};
pub use scanner::{OpenRgbScanner, ScannerConfig};

use crate::types::device::{
    ConnectionType, DeviceCapabilities, DeviceColorFormat, DeviceFamily, DeviceId, DeviceInfo,
    DeviceTopologyHint, ZoneInfo,
};

// ── Shared Mapping ────────────────────────────────────────────────────────

/// Map an `OpenRGB` [`ZoneData`] to a Hypercolor [`ZoneInfo`].
///
/// Shared between the backend and scanner to avoid duplicating the
/// zone-type-to-topology conversion logic.
fn map_zone(zone: &proto::ZoneData) -> ZoneInfo {
    let topology = match zone.zone_type {
        proto::ZoneType::Single => {
            if zone.leds_count == 1 {
                DeviceTopologyHint::Point
            } else {
                DeviceTopologyHint::Custom
            }
        }
        proto::ZoneType::Linear => DeviceTopologyHint::Strip,
        proto::ZoneType::Matrix => DeviceTopologyHint::Matrix {
            rows: zone.matrix_height,
            cols: zone.matrix_width,
        },
    };

    ZoneInfo {
        name: zone.name.clone(),
        led_count: zone.leds_count,
        topology,
        color_format: DeviceColorFormat::Rgb,
    }
}

/// Build a [`DeviceInfo`] from an `OpenRGB` controller.
///
/// Shared between the backend and scanner to avoid duplicating the
/// controller-to-device mapping logic.
fn build_device_info(controller: &proto::ControllerData) -> DeviceInfo {
    let zones: Vec<ZoneInfo> = controller.zones.iter().map(map_zone).collect();
    let total_leds: u32 = zones.iter().map(|z| z.led_count).sum();

    DeviceInfo {
        id: DeviceId::new(),
        name: controller.name.clone(),
        vendor: controller.vendor.clone(),
        family: DeviceFamily::OpenRgb,
        connection_type: ConnectionType::Network,
        zones,
        firmware_version: if controller.version.is_empty() {
            None
        } else {
            Some(controller.version.clone())
        },
        capabilities: DeviceCapabilities {
            led_count: total_leds,
            supports_direct: true,
            supports_brightness: false,
            max_fps: 60,
        },
    }
}
