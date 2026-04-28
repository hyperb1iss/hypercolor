//! Native device backend boundary shared by core and built-in drivers.

use std::sync::Arc;

use anyhow::{Result, bail};
use hypercolor_types::device::{
    DeviceId, DeviceInfo, DisplayFrameFormat, OwnedDisplayFramePayload,
};
use serde::{Deserialize, Serialize};

/// Static metadata describing a device backend.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackendInfo {
    /// Unique backend identifier used in configuration and feature gating.
    pub id: String,
    /// Human-readable backend name for logging and UI display.
    pub name: String,
    /// Short description of what this backend supports.
    pub description: String,
}

/// Core device communication trait.
#[async_trait::async_trait]
pub trait DeviceBackend: Send + Sync {
    /// Static metadata about this backend.
    fn info(&self) -> BackendInfo;

    /// Scan for devices reachable via this backend's transport.
    ///
    /// # Errors
    ///
    /// Returns an error if the transport is unavailable or the scan fails.
    async fn discover(&mut self) -> Result<Vec<DeviceInfo>>;

    /// Return refreshed metadata for a connected device, if available.
    ///
    /// # Errors
    ///
    /// Returns an error if the device is connected but metadata retrieval
    /// fails. The default implementation reports no refreshed metadata.
    async fn connected_device_info(&self, id: &DeviceId) -> Result<Option<DeviceInfo>> {
        let _ = id;
        Ok(None)
    }

    /// Establish a connection to a specific device.
    ///
    /// # Errors
    ///
    /// Returns an error if the device is not found, permissions are denied,
    /// or the transport-level connection fails.
    async fn connect(&mut self, id: &DeviceId) -> Result<()>;

    /// Cleanly disconnect from a device.
    ///
    /// # Errors
    ///
    /// Returns an error if the disconnect operation fails.
    async fn disconnect(&mut self, id: &DeviceId) -> Result<()>;

    /// Push LED color data to a connected device.
    ///
    /// # Errors
    ///
    /// Returns an error if the device is disconnected or the write fails.
    async fn write_colors(&mut self, id: &DeviceId, colors: &[[u8; 3]]) -> Result<()>;

    /// Push shared LED color data to a connected device.
    ///
    /// # Errors
    ///
    /// Returns an error if the device is disconnected or the write fails.
    async fn write_colors_shared(
        &mut self,
        id: &DeviceId,
        colors: Arc<Vec<[u8; 3]>>,
    ) -> Result<()> {
        self.write_colors(id, colors.as_slice()).await
    }

    /// Push a JPEG-compressed display frame to a connected device, if supported.
    ///
    /// # Errors
    ///
    /// Returns an error if display output is unsupported or the write fails.
    async fn write_display_frame(&mut self, id: &DeviceId, jpeg_data: &[u8]) -> Result<()> {
        let _ = (id, jpeg_data);
        bail!(
            "backend '{}' does not support device display output",
            self.info().id
        );
    }

    /// Push an owned JPEG-compressed display frame to a connected device.
    ///
    /// # Errors
    ///
    /// Returns an error if display output is unsupported or the write fails.
    async fn write_display_frame_owned(
        &mut self,
        id: &DeviceId,
        jpeg_data: Arc<Vec<u8>>,
    ) -> Result<()> {
        self.write_display_frame(id, jpeg_data.as_slice()).await
    }

    /// Push an owned display payload to a connected device.
    ///
    /// # Errors
    ///
    /// Returns an error if display output is unsupported or the write fails.
    async fn write_display_payload_owned(
        &mut self,
        id: &DeviceId,
        payload: Arc<OwnedDisplayFramePayload>,
    ) -> Result<()> {
        match payload.format {
            DisplayFrameFormat::Jpeg => {
                self.write_display_frame_owned(id, Arc::clone(&payload.data))
                    .await
            }
            DisplayFrameFormat::Rgb => bail!(
                "backend '{}' does not support RGB display output",
                self.info().id
            ),
        }
    }

    /// Adjust hardware brightness for a connected device, if supported.
    ///
    /// # Errors
    ///
    /// Returns an error if device-level brightness is unsupported or the write
    /// fails.
    async fn set_brightness(&mut self, id: &DeviceId, brightness: u8) -> Result<()> {
        let _ = (id, brightness);
        bail!(
            "backend '{}' does not support device brightness control",
            self.info().id
        );
    }

    /// Preferred output frame rate for a connected device.
    #[must_use]
    fn target_fps(&self, id: &DeviceId) -> Option<u32> {
        let _ = id;
        None
    }

    /// Non-destructive health probe for a connected device.
    ///
    /// # Errors
    ///
    /// Returns an error only if probing fails unexpectedly.
    async fn health_check(&self, id: &DeviceId) -> Result<HealthStatus> {
        let _ = id;
        Ok(HealthStatus::Healthy)
    }
}

/// High-level connectivity state reported by [`DeviceBackend::health_check`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthStatus {
    /// Device is reachable and behaving normally.
    Healthy,
    /// Device is reachable but exhibiting partial failure.
    Degraded,
    /// Device is currently unreachable.
    Unreachable,
}
