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

/// How the daemon should execute a backend connect action.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ConnectExecution {
    /// Run the connect action inline with the current lifecycle pass.
    #[default]
    Inline,
    /// Detach the connect action so discovery can keep reporting progress.
    Background,
}

impl ConnectExecution {
    /// Whether this policy asks the lifecycle executor to detach connect work.
    #[must_use]
    pub const fn is_background(self) -> bool {
        matches!(self, Self::Background)
    }
}

/// Daemon lifecycle policy advertised by a backend for one discovered device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeviceLifecyclePolicy {
    connect_timeout: Duration,
    connect_execution: ConnectExecution,
    retry_on_connect_timeout: bool,
}

impl DeviceLifecyclePolicy {
    /// Default timeout for ordinary backend connect calls.
    pub const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

    /// Create a policy with explicit fields.
    #[must_use]
    pub const fn new(
        connect_timeout: Duration,
        connect_execution: ConnectExecution,
        retry_on_connect_timeout: bool,
    ) -> Self {
        Self {
            connect_timeout,
            connect_execution,
            retry_on_connect_timeout,
        }
    }

    /// Timeout applied to backend connect calls after the backend lock is acquired.
    #[must_use]
    pub const fn connect_timeout(self) -> Duration {
        self.connect_timeout
    }

    /// Execution mode for lifecycle connect actions.
    #[must_use]
    pub const fn connect_execution(self) -> ConnectExecution {
        self.connect_execution
    }

    /// Whether timeout failures should feed the lifecycle retry path.
    #[must_use]
    pub const fn retry_on_connect_timeout(self) -> bool {
        self.retry_on_connect_timeout
    }

    /// Return a copy with a different connect timeout.
    #[must_use]
    pub const fn with_connect_timeout(mut self, timeout: Duration) -> Self {
        self.connect_timeout = timeout;
        self
    }

    /// Return a copy with a different connect execution mode.
    #[must_use]
    pub const fn with_connect_execution(mut self, execution: ConnectExecution) -> Self {
        self.connect_execution = execution;
        self
    }

    /// Return a copy that abandons lifecycle retry after connect timeouts.
    #[must_use]
    pub const fn without_connect_timeout_retry(mut self) -> Self {
        self.retry_on_connect_timeout = false;
        self
    }
}

impl Default for DeviceLifecyclePolicy {
    fn default() -> Self {
        Self {
            connect_timeout: Self::DEFAULT_CONNECT_TIMEOUT,
            connect_execution: ConnectExecution::Inline,
            retry_on_connect_timeout: true,
        }
    }
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

/// Queue-qualified identity for one device-frame delivery attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DeviceDeliveryId {
    /// Generation of the output queue that issued this attempt.
    pub queue_generation: u64,
    /// Monotonic sequence within the queue.
    pub sequence: u64,
}

/// Terminal state reported for one device-frame delivery attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceDeliveryStatus {
    /// The transport completed the payload successfully.
    Completed,
    /// The device lane suppressed an unchanged payload.
    SuppressedDuplicate,
    /// The device lane suppressed a payload inside its cadence window.
    SuppressedCadence,
    /// The transport or output lane rejected the attempt.
    Failed,
}

/// Exact acknowledgement for one queue-qualified device-frame delivery.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceDeliveryAck {
    /// Identity copied from the delivery request.
    pub id: DeviceDeliveryId,
    /// Terminal disposition of the attempt.
    pub status: DeviceDeliveryStatus,
    /// Whether transport I/O started for this attempt.
    pub transport_started: bool,
    /// Payload bytes completed by the transport. Zero for every other state.
    pub completed_payload_bytes: u64,
    /// Time spent in actual transport I/O, excluding queue wait.
    pub transport_latency: Duration,
    /// Error reported by a failed attempt.
    pub error: Option<String>,
}

/// Observer notified when a queue-qualified delivery begins transport I/O.
pub trait DeviceDeliveryObserver: Send + Sync {
    /// Record that the matching transport attempt has started.
    fn transport_started(&self, id: DeviceDeliveryId);
}

impl DeviceDeliveryAck {
    /// Build an acknowledgement from a legacy synchronous lane result.
    #[must_use]
    pub fn from_write_result(
        id: DeviceDeliveryId,
        payload_bytes: usize,
        transport_latency: Duration,
        result: Result<DeviceWriteOutcome>,
    ) -> Self {
        match result {
            Ok(DeviceWriteOutcome::Sent) => Self {
                id,
                status: DeviceDeliveryStatus::Completed,
                transport_started: true,
                completed_payload_bytes: u64::try_from(payload_bytes).unwrap_or(u64::MAX),
                transport_latency,
                error: None,
            },
            Ok(DeviceWriteOutcome::SuppressedDuplicate) => {
                Self::suppressed(id, DeviceDeliveryStatus::SuppressedDuplicate)
            }
            Ok(DeviceWriteOutcome::SuppressedCadence) => {
                Self::suppressed(id, DeviceDeliveryStatus::SuppressedCadence)
            }
            Err(error) => Self::failed(id, true, transport_latency, error.to_string()),
        }
    }

    /// Build an acknowledgement for a transport attempt rejected before I/O.
    #[must_use]
    pub fn rejected(id: DeviceDeliveryId, error: impl Into<String>) -> Self {
        Self::failed(id, false, Duration::ZERO, error.into())
    }

    /// Build an acknowledgement for a completed transport attempt.
    #[must_use]
    pub fn completed(
        id: DeviceDeliveryId,
        payload_bytes: usize,
        transport_latency: Duration,
    ) -> Self {
        Self {
            id,
            status: DeviceDeliveryStatus::Completed,
            transport_started: true,
            completed_payload_bytes: u64::try_from(payload_bytes).unwrap_or(u64::MAX),
            transport_latency,
            error: None,
        }
    }

    /// Build an acknowledgement for a failed transport attempt.
    #[must_use]
    pub fn failed(
        id: DeviceDeliveryId,
        transport_started: bool,
        transport_latency: Duration,
        error: impl Into<String>,
    ) -> Self {
        Self {
            id,
            status: DeviceDeliveryStatus::Failed,
            transport_started,
            completed_payload_bytes: 0,
            transport_latency,
            error: Some(error.into()),
        }
    }

    fn suppressed(id: DeviceDeliveryId, status: DeviceDeliveryStatus) -> Self {
        Self {
            id,
            status,
            transport_started: false,
            completed_payload_bytes: 0,
            transport_latency: Duration::ZERO,
            error: None,
        }
    }
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

    /// Deliver a queue-qualified payload and acknowledge its terminal state.
    ///
    /// Drivers with their own output actor override this method so the future
    /// resolves after that actor completes or fails the matching transport I/O.
    async fn deliver_colors_shared(
        &self,
        id: DeviceDeliveryId,
        colors: Arc<Vec<[u8; 3]>>,
    ) -> DeviceDeliveryAck {
        let payload_bytes = colors.len().saturating_mul(3);
        let started_at = std::time::Instant::now();
        let result = self.write_colors_shared_outcome(colors).await;
        DeviceDeliveryAck::from_write_result(id, payload_bytes, started_at.elapsed(), result)
    }

    /// Deliver a queue-qualified payload with live transport-start observation.
    ///
    /// Actor-backed drivers override this method and notify `observer` at the
    /// precise point their transport I/O begins. The default preserves legacy
    /// terminal acknowledgement behavior for synchronous sinks.
    async fn deliver_colors_shared_observed(
        &self,
        id: DeviceDeliveryId,
        colors: Arc<Vec<[u8; 3]>>,
        observer: Arc<dyn DeviceDeliveryObserver>,
    ) -> DeviceDeliveryAck {
        observer.transport_started(id);
        self.deliver_colors_shared(id, colors).await
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

    /// Deliver a queue-qualified payload with live transport-start observation.
    async fn deliver_colors_shared_observed(
        &mut self,
        device_id: &DeviceId,
        delivery_id: DeviceDeliveryId,
        colors: Arc<Vec<[u8; 3]>>,
        observer: Arc<dyn DeviceDeliveryObserver>,
    ) -> DeviceDeliveryAck {
        observer.transport_started(delivery_id);
        let payload_bytes = colors.len().saturating_mul(3);
        let started_at = std::time::Instant::now();
        let result = self.write_colors_shared_outcome(device_id, colors).await;
        DeviceDeliveryAck::from_write_result(
            delivery_id,
            payload_bytes,
            started_at.elapsed(),
            result,
        )
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

    /// Lifecycle policy for a discovered device before connect.
    #[must_use]
    fn lifecycle_policy(&self, info: &DeviceInfo) -> DeviceLifecyclePolicy {
        let _ = info;
        DeviceLifecyclePolicy::default()
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
