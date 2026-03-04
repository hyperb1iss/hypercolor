//! `DeviceBackend` implementation for `OpenRGB` SDK communication.
//!
//! [`OpenRgbBackend`] connects directly to an `OpenRGB` SDK server over TCP,
//! enumerates controllers as Hypercolor devices, and pushes LED color
//! data via the binary wire protocol.

use std::collections::HashMap;

use anyhow::{Context, Result, bail};
use tracing::{debug, info, warn};

use crate::device::traits::{BackendInfo, DeviceBackend};
use crate::types::device::{DeviceId, DeviceInfo};

use super::client::{ClientConfig, OpenRgbClient};

// ── Controller Mapping ───────────────────────────────────────────────────

/// Cached mapping between a Hypercolor `DeviceId` and an `OpenRGB` controller index.
#[derive(Debug, Clone)]
struct ControllerMapping {
    /// `OpenRGB` controller index (0-based).
    device_index: u32,
    /// Controller name for diagnostics.
    name: String,
}

// ── OpenRgbBackend ───────────────────────────────────────────────────────

/// Device backend that communicates directly with an `OpenRGB` SDK server.
///
/// Implements the [`DeviceBackend`] trait, translating Hypercolor's
/// device lifecycle (discover, connect, disconnect, write) into `OpenRGB`
/// SDK protocol operations.
pub struct OpenRgbBackend {
    /// TCP client for the `OpenRGB` SDK server.
    client: OpenRgbClient,

    /// Mapping from `DeviceId` to `OpenRGB` controller index.
    device_map: HashMap<DeviceId, ControllerMapping>,

    /// Reverse mapping from `OpenRGB` controller index to `DeviceId`.
    index_to_id: HashMap<u32, DeviceId>,

    /// Set of `DeviceId`s that have been "connected" (switched to Direct mode).
    connected_devices: HashMap<DeviceId, bool>,
}

impl OpenRgbBackend {
    /// Create a new backend with the given client configuration.
    #[must_use]
    pub fn new(config: ClientConfig) -> Self {
        Self {
            client: OpenRgbClient::new(config),
            device_map: HashMap::new(),
            index_to_id: HashMap::new(),
            connected_devices: HashMap::new(),
        }
    }

    /// Create a backend with default configuration (localhost:6742).
    #[must_use]
    pub fn with_defaults() -> Self {
        Self::new(ClientConfig::default())
    }

    /// Map an `OpenRGB` controller to a Hypercolor [`DeviceInfo`].
    fn map_controller(controller: &super::proto::ControllerData) -> DeviceInfo {
        super::build_device_info(controller)
    }
}

#[async_trait::async_trait]
impl DeviceBackend for OpenRgbBackend {
    fn info(&self) -> BackendInfo {
        BackendInfo {
            id: "openrgb".to_owned(),
            name: "OpenRGB (SDK)".to_owned(),
            description: "Direct connection to OpenRGB SDK server over TCP".to_owned(),
        }
    }

    async fn discover(&mut self) -> Result<Vec<DeviceInfo>> {
        // Connect if not already connected
        if !self.client.is_connected() {
            self.client
                .connect()
                .await
                .context("failed to connect to OpenRGB SDK server")?;
        }

        // Enumerate controllers
        let count = self
            .client
            .enumerate_controllers()
            .await
            .context("failed to enumerate controllers")?;

        info!(count, "OpenRGB controllers discovered");

        // Build device mappings
        self.device_map.clear();
        self.index_to_id.clear();

        let mut devices = Vec::with_capacity(usize::try_from(count).unwrap_or(0));

        for (&index, controller) in self.client.controllers() {
            let device_info = Self::map_controller(controller);
            let device_id = device_info.id;

            self.device_map.insert(
                device_id,
                ControllerMapping {
                    device_index: index,
                    name: controller.name.clone(),
                },
            );
            self.index_to_id.insert(index, device_id);

            debug!(
                device_id = %device_id,
                index,
                name = %controller.name,
                leds = device_info.total_led_count(),
                zones = controller.zones.len(),
                "Mapped OpenRGB controller to device"
            );

            devices.push(device_info);
        }

        Ok(devices)
    }

    async fn connect(&mut self, id: &DeviceId) -> Result<()> {
        let mapped_ids = self
            .device_map
            .keys()
            .take(4)
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        let mapping = self
            .device_map
            .get(id)
            .with_context(|| {
                format!(
                    "device {id} not found in OpenRGB backend map (mapped_count={}, sample_ids=[{}])",
                    self.device_map.len(),
                    mapped_ids
                )
            })?
            .clone();

        // Switch the controller to Direct/Custom mode
        self.client
            .set_custom_mode(mapping.device_index)
            .await
            .with_context(|| {
                format!(
                    "failed to set custom mode for controller {} ({})",
                    mapping.device_index, mapping.name
                )
            })?;

        self.connected_devices.insert(*id, true);

        info!(
            device_id = %id,
            controller = mapping.device_index,
            name = %mapping.name,
            "OpenRGB controller connected in Direct mode"
        );

        Ok(())
    }

    async fn disconnect(&mut self, id: &DeviceId) -> Result<()> {
        if self.connected_devices.remove(id).is_none() {
            warn!(device_id = %id, "Attempted to disconnect unknown OpenRGB device");
        }
        Ok(())
    }

    async fn write_colors(&mut self, id: &DeviceId, colors: &[[u8; 3]]) -> Result<()> {
        if !self.connected_devices.contains_key(id) {
            bail!("device {id} is not connected");
        }

        let mapping = self
            .device_map
            .get(id)
            .context("device mapping not found")?;

        self.client
            .update_leds(mapping.device_index, colors)
            .await
            .with_context(|| {
                format!(
                    "failed to update LEDs for controller {} ({})",
                    mapping.device_index, mapping.name
                )
            })
    }
}
