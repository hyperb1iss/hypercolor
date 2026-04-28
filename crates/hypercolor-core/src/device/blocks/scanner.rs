//! Discovery scanner for ROLI Blocks devices via blocksd.
//!
//! Connects to the blocksd Unix domain socket and translates its discover
//! response into `DiscoveredDevice` entries for the orchestrator.

use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::Result;
use tracing::debug;

use crate::device::{DiscoveredDevice, DiscoveryConnectBehavior, TransportScanner};
use crate::types::device::{
    ConnectionType, DeviceCapabilities, DeviceColorFormat, DeviceFamily, DeviceFeatures,
    DeviceFingerprint, DeviceInfo, DeviceOrigin, DeviceTopologyHint, ZoneInfo,
};

use super::connection::{self, BlocksConnection};
use super::types::{BlocksDeviceResponse, RoliBlockType};

/// Transport scanner that discovers ROLI Blocks devices via blocksd.
pub struct BlocksScanner {
    socket_path: PathBuf,
}

impl BlocksScanner {
    /// Create a scanner targeting the given blocksd socket path.
    #[must_use]
    pub fn new(socket_path: PathBuf) -> Self {
        Self { socket_path }
    }

    /// Create a scanner using the default socket path.
    #[must_use]
    pub fn with_default_path() -> Self {
        Self {
            socket_path: connection::default_socket_path(),
        }
    }
}

#[async_trait::async_trait]
impl TransportScanner for BlocksScanner {
    fn name(&self) -> &'static str {
        "ROLI Blocks (blocksd)"
    }

    async fn scan(&mut self) -> Result<Vec<DiscoveredDevice>> {
        if !connection::socket_exists(&self.socket_path) {
            debug!("blocksd socket not found, skipping scan");
            return Ok(vec![]);
        }

        let mut conn = BlocksConnection::connect(&self.socket_path).await?;
        let response = conn.discover().await?;

        let mut discovered = Vec::with_capacity(response.devices.len());
        for dev in &response.devices {
            discovered.push(build_discovered_device(dev));
        }

        debug!(count = discovered.len(), "blocksd scan complete");
        Ok(discovered)
    }
}

fn build_discovered_device(dev: &BlocksDeviceResponse) -> DiscoveredDevice {
    let block_type = RoliBlockType::from_api(&dev.block_type);
    let serial_short = if dev.serial.len() >= 6 {
        &dev.serial[..6]
    } else {
        &dev.serial
    };

    let fingerprint = DeviceFingerprint(format!("bridge:blocksd:{}", dev.uid));
    let device_id = fingerprint.stable_device_id();

    let rows = dev.grid_height;
    let cols = dev.grid_width;
    let led_count = rows * cols;

    let info = DeviceInfo {
        id: device_id,
        name: format!("{} ({serial_short})", block_type.display_name()),
        vendor: "ROLI".to_owned(),
        family: DeviceFamily::new_static("roli", "ROLI"),
        model: Some(block_type.display_name().to_owned()),
        connection_type: ConnectionType::Bridge,
        origin: DeviceOrigin::native("roli", "blocks", ConnectionType::Bridge),
        zones: vec![ZoneInfo {
            name: "Grid".to_owned(),
            led_count,
            topology: DeviceTopologyHint::Matrix { rows, cols },
            color_format: DeviceColorFormat::Rgb,
        }],
        firmware_version: dev.firmware_version.clone(),
        capabilities: DeviceCapabilities {
            led_count,
            supports_direct: true,
            supports_brightness: true,
            has_display: false,
            display_resolution: None,
            max_fps: 25,
            color_space: hypercolor_types::device::DeviceColorSpace::default(),
            features: DeviceFeatures::default(),
        },
    };

    let mut metadata = HashMap::new();
    metadata.insert("uid".to_owned(), dev.uid.to_string());
    metadata.insert("serial".to_owned(), dev.serial.clone());
    metadata.insert("block_type".to_owned(), dev.block_type.clone());

    DiscoveredDevice {
        connection_type: ConnectionType::Bridge,
        origin: info.origin.clone(),
        name: info.name.clone(),
        family: DeviceFamily::new_static("roli", "ROLI"),
        fingerprint,
        connect_behavior: DiscoveryConnectBehavior::AutoConnect,
        info,
        metadata,
    }
}
