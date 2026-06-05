//! Native device backend boundary shared by core and built-in drivers.

use std::sync::Arc;
use std::time::Duration;

use crate::discovery::DiscoveredDevice;
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

/// Result of accepting a color frame into a device output lane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceWriteOutcome {
    /// The frame was sent to the transport or handed to an async transport worker.
    Sent,
    /// The lane intentionally skipped an identical frame.
    SuppressedDuplicate,
    /// The lane intentionally skipped a frame inside its cadence window.
    SuppressedCadence,
}

impl DeviceWriteOutcome {
    /// Whether this outcome represents bytes accepted for transport output.
    #[must_use]
    pub const fn is_sent(self) -> bool {
        matches!(self, Self::Sent)
    }
}

/// Preferred output cadence for a connected device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutputCadence {
    min_interval: Option<Duration>,
    target_fps: u32,
}

impl OutputCadence {
    /// Build cadence from an integer FPS cap. `0` means unpaced.
    #[must_use]
    pub fn from_fps(target_fps: u32) -> Self {
        if target_fps == 0 {
            return Self {
                min_interval: None,
                target_fps,
            };
        }

        Self {
            min_interval: Some(Duration::from_secs_f64(1.0 / f64::from(target_fps))),
            target_fps,
        }
    }

    /// Build cadence from a concrete minimum interval.
    #[must_use]
    pub const fn from_min_interval(min_interval: Duration, target_fps: u32) -> Self {
        Self {
            min_interval: Some(min_interval),
            target_fps,
        }
    }

    /// Minimum interval between output attempts, or `None` for unpaced output.
    #[must_use]
    pub const fn min_interval(self) -> Option<Duration> {
        self.min_interval
    }

    /// Legacy integer target FPS for displays that cannot represent sub-Hz rates.
    #[must_use]
    pub const fn target_fps(self) -> u32 {
        self.target_fps
    }

    /// Concrete cadence interval in milliseconds for telemetry.
    #[must_use]
    pub fn interval_ms(self) -> Option<u64> {
        self.min_interval.map(|interval| {
            let millis = interval.as_millis();
            u64::try_from(millis).unwrap_or(u64::MAX)
        })
    }
}

impl Default for OutputCadence {
    fn default() -> Self {
        Self::from_fps(60)
    }
}

/// Cloneable hot-path output lane for one connected device.
#[async_trait::async_trait]
pub trait DeviceFrameSink: Send + Sync {
    /// Push shared LED color data to this device's output lane.
    ///
    /// # Errors
    ///
    /// Returns an error if the device output lane is no longer available or
    /// the driver has observed an asynchronous transport failure.
    async fn write_colors_shared(&self, colors: Arc<Vec<[u8; 3]>>) -> Result<()>;

    /// Push shared LED color data and report whether the lane actually sent it.
    ///
    /// # Errors
    ///
    /// Returns an error if the device output lane is no longer available or
    /// the driver has observed an asynchronous transport failure.
    async fn write_colors_shared_outcome(
        &self,
        colors: Arc<Vec<[u8; 3]>>,
    ) -> Result<DeviceWriteOutcome> {
        self.write_colors_shared(colors)
            .await
            .map(|()| DeviceWriteOutcome::Sent)
    }
}

/// Cloneable hot-path display output lane for one connected, display-capable device.
///
/// Successful writes only mean the sink accepted the latest payload; the
/// backend may still deliver the bytes asynchronously.
#[async_trait::async_trait]
pub trait DeviceDisplaySink: Send + Sync {
    /// Push an owned display payload to this device's output lane.
    ///
    /// # Errors
    ///
    /// Returns an error if the device output lane is no longer available or
    /// the driver has observed an asynchronous transport failure.
    async fn write_display_payload_owned(
        &self,
        payload: Arc<OwnedDisplayFramePayload>,
    ) -> Result<()>;
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

    /// Prime any backend-local discovery cache from a scanner result.
    ///
    /// Host transport backends can use this to carry scanner metadata into
    /// `connect()` without running a second hardware discovery pass.
    fn remember_discovered_device(&mut self, discovered: &DiscoveredDevice) {
        let _ = discovered;
    }

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

    /// Push shared LED color data and report whether the backend actually sent it.
    ///
    /// # Errors
    ///
    /// Returns an error if the device is disconnected or the write fails.
    async fn write_colors_shared_outcome(
        &mut self,
        id: &DeviceId,
        colors: Arc<Vec<[u8; 3]>>,
    ) -> Result<DeviceWriteOutcome> {
        self.write_colors_shared(id, colors)
            .await
            .map(|()| DeviceWriteOutcome::Sent)
    }

    /// Return a cloneable hot-path frame sink for a connected device.
    #[must_use]
    fn frame_sink(&self, id: &DeviceId) -> Option<Arc<dyn DeviceFrameSink>> {
        let _ = id;
        None
    }

    /// Return a cloneable hot-path display sink for a healthy connected display device.
    #[must_use]
    fn display_sink(&self, id: &DeviceId) -> Option<Arc<dyn DeviceDisplaySink>> {
        let _ = id;
        None
    }

    /// Whether this backend can briefly connect a known, currently idle device
    /// for direct-control operations such as identify flashes.
    #[must_use]
    fn supports_temporary_direct_control(&self, info: &DeviceInfo) -> bool {
        let _ = info;
        false
    }

    /// Whether this backend consumes host-managed attachment profiles when
    /// preparing a device connection.
    #[must_use]
    fn supports_host_attachment_profiles(&self, info: &DeviceInfo) -> bool {
        let _ = info;
        false
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

    /// Preferred output cadence for a connected device.
    #[must_use]
    fn output_cadence(&self, id: &DeviceId) -> Option<OutputCadence> {
        self.target_fps(id).map(OutputCadence::from_fps)
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
