//! Native device backend boundary shared by core and built-in drivers.

use std::sync::Arc;
use std::time::Duration;

use crate::discovery::DiscoveredDevice;
use hypercolor_types::device::{
    DeviceError, DeviceId, DeviceInfo, ErrorRecoverability, OwnedDisplayFramePayload,
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

    /// Decide whether a typed connect failure should enter lifecycle retry.
    #[must_use]
    pub const fn should_retry_connect_failure(self, error: &DeviceError) -> bool {
        match error.recoverability() {
            ErrorRecoverability::Permanent => false,
            ErrorRecoverability::Retry | ErrorRecoverability::Reconnect => {
                !matches!(error, DeviceError::Timeout { .. }) || self.retry_on_connect_timeout
            }
        }
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
    /// Typed error reported by a failed attempt.
    pub error: Option<DeviceError>,
}

/// Observer notified as a queue-qualified delivery crosses transport boundaries.
pub trait DeviceDeliveryObserver: Send + Sync {
    /// Record that the matching transport attempt has started.
    fn transport_started(&self, id: DeviceDeliveryId);

    /// Record the matching terminal transport acknowledgement.
    fn delivery_terminal(&self, ack: &DeviceDeliveryAck) {
        let _ = ack;
    }
}

impl DeviceDeliveryAck {
    /// Build an acknowledgement from a legacy synchronous lane result.
    #[must_use]
    pub fn from_write_result(
        id: DeviceDeliveryId,
        payload_bytes: usize,
        transport_latency: Duration,
        result: Result<DeviceWriteOutcome, DeviceError>,
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
            Err(error) => Self::failed(id, true, transport_latency, error),
        }
    }

    /// Build an acknowledgement for a transport attempt rejected before I/O.
    #[must_use]
    pub fn rejected(id: DeviceDeliveryId, error: DeviceError) -> Self {
        Self::failed(id, false, Duration::ZERO, error)
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
        error: DeviceError,
    ) -> Self {
        Self {
            id,
            status: DeviceDeliveryStatus::Failed,
            transport_started,
            completed_payload_bytes: 0,
            transport_latency,
            error: Some(error),
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
    max_frame_silence: Option<Duration>,
    target_fps: u32,
}

impl OutputCadence {
    /// Build cadence from an integer FPS cap. `0` means unpaced.
    #[must_use]
    pub fn from_fps(target_fps: u32) -> Self {
        if target_fps == 0 {
            return Self {
                min_interval: None,
                max_frame_silence: None,
                target_fps,
            };
        }

        Self {
            min_interval: Some(Duration::from_secs_f64(1.0 / f64::from(target_fps))),
            max_frame_silence: None,
            target_fps,
        }
    }

    /// Build cadence from a concrete minimum interval.
    #[must_use]
    pub const fn from_min_interval(min_interval: Duration, target_fps: u32) -> Self {
        Self {
            min_interval: Some(min_interval),
            max_frame_silence: None,
            target_fps,
        }
    }

    /// Require the output worker to resend its cached payload after this much
    /// transport silence. A zero duration disables cached-payload replay.
    #[must_use]
    pub const fn with_max_frame_silence(mut self, max_frame_silence: Duration) -> Self {
        self.max_frame_silence = if max_frame_silence.is_zero() {
            None
        } else {
            Some(max_frame_silence)
        };
        self
    }

    /// Minimum interval between output attempts, or `None` for unpaced output.
    #[must_use]
    pub const fn min_interval(self) -> Option<Duration> {
        self.min_interval
    }

    /// Maximum time between completed transport writes before the cached
    /// payload must be resent.
    #[must_use]
    pub const fn max_frame_silence(self) -> Option<Duration> {
        self.max_frame_silence
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

    /// Maximum transport silence in milliseconds for telemetry.
    #[must_use]
    pub fn max_frame_silence_ms(self) -> Option<u64> {
        self.max_frame_silence.map(|interval| {
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
    async fn write_colors_shared(&self, colors: Arc<Vec<[u8; 3]>>) -> Result<(), DeviceError>;

    /// Push shared LED color data and report whether the lane actually sent it.
    ///
    /// # Errors
    ///
    /// Returns an error if the device output lane is no longer available or
    /// the driver has observed an asynchronous transport failure.
    async fn write_colors_shared_outcome(
        &self,
        colors: Arc<Vec<[u8; 3]>>,
    ) -> Result<DeviceWriteOutcome, DeviceError> {
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
    ) -> Result<(), DeviceError>;

    /// Deliver a queue-qualified payload and acknowledge its terminal state.
    ///
    /// Drivers with their own output actor override this method so the future
    /// resolves after that actor completes or fails the matching transport I/O.
    async fn deliver_display_payload_owned(
        &self,
        id: DeviceDeliveryId,
        payload: Arc<OwnedDisplayFramePayload>,
    ) -> DeviceDeliveryAck {
        let payload_bytes = payload.data.len();
        let started_at = std::time::Instant::now();
        match self.write_display_payload_owned(payload).await {
            Ok(()) => DeviceDeliveryAck::completed(id, payload_bytes, started_at.elapsed()),
            Err(error) => DeviceDeliveryAck::failed(id, true, started_at.elapsed(), error),
        }
    }

    /// Deliver a queue-qualified payload with actor-owned terminal observation.
    ///
    /// Actor-backed drivers override this method and retain `observer` with the
    /// physical delivery so cancellation of the awaiting caller cannot discard
    /// its terminal acknowledgement.
    async fn deliver_display_payload_owned_observed(
        &self,
        id: DeviceDeliveryId,
        payload: Arc<OwnedDisplayFramePayload>,
        observer: Arc<dyn DeviceDeliveryObserver>,
    ) -> DeviceDeliveryAck {
        observer.transport_started(id);
        let ack = self.deliver_display_payload_owned(id, payload).await;
        observer.delivery_terminal(&ack);
        ack
    }
}

/// Core device communication trait.
#[async_trait::async_trait]
pub trait DeviceBackend: Send + Sync {
    /// Static metadata about this backend.
    fn info(&self) -> BackendInfo;

    /// Adopt one device emitted by this backend's discovery capability.
    ///
    /// # Errors
    ///
    /// Returns an error when the discovery payload does not belong to this
    /// backend or cannot be installed into backend-owned inventory.
    fn adopt_device(&self, discovered: &DiscoveredDevice) -> Result<(), DeviceError>;

    /// Return refreshed metadata for a connected device, if available.
    ///
    /// # Errors
    ///
    /// Returns an error if the device is connected but metadata retrieval
    /// fails. The default implementation reports no refreshed metadata.
    async fn connected_device_info(
        &self,
        id: &DeviceId,
    ) -> Result<Option<DeviceInfo>, DeviceError> {
        let _ = id;
        Ok(None)
    }

    /// Establish a connection to a specific device.
    ///
    /// # Errors
    ///
    /// Returns an error if the device is not found, permissions are denied,
    /// or the transport-level connection fails.
    async fn connect(&self, id: &DeviceId) -> Result<(), DeviceError>;

    /// Cleanly disconnect from a device.
    ///
    /// # Errors
    ///
    /// Returns an error if the disconnect operation fails.
    async fn disconnect(&self, id: &DeviceId) -> Result<(), DeviceError>;

    /// Push LED color data to a connected device.
    ///
    /// # Errors
    ///
    /// Returns an error if the device is disconnected or the write fails.
    async fn write_colors(&self, id: &DeviceId, colors: &[[u8; 3]]) -> Result<(), DeviceError>;

    /// Push shared LED color data to a connected device.
    ///
    /// # Errors
    ///
    /// Returns an error if the device is disconnected or the write fails.
    async fn write_colors_shared(
        &self,
        id: &DeviceId,
        colors: Arc<Vec<[u8; 3]>>,
    ) -> Result<(), DeviceError> {
        self.write_colors(id, colors.as_slice()).await
    }

    /// Push shared LED color data and report whether the backend actually sent it.
    ///
    /// # Errors
    ///
    /// Returns an error if the device is disconnected or the write fails.
    async fn write_colors_shared_outcome(
        &self,
        id: &DeviceId,
        colors: Arc<Vec<[u8; 3]>>,
    ) -> Result<DeviceWriteOutcome, DeviceError> {
        self.write_colors_shared(id, colors)
            .await
            .map(|()| DeviceWriteOutcome::Sent)
    }

    /// Deliver a queue-qualified payload with live transport-start observation.
    async fn deliver_colors_shared_observed(
        &self,
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

    /// Deliver a queue-qualified display payload with terminal observation.
    async fn deliver_display_payload_owned_observed(
        &self,
        device_id: &DeviceId,
        delivery_id: DeviceDeliveryId,
        payload: Arc<OwnedDisplayFramePayload>,
        observer: Arc<dyn DeviceDeliveryObserver>,
    ) -> DeviceDeliveryAck {
        observer.transport_started(delivery_id);
        let payload_bytes = payload.data.len();
        let started_at = std::time::Instant::now();
        let result = self.write_display_payload_owned(device_id, payload).await;
        let ack = match result {
            Ok(()) => {
                DeviceDeliveryAck::completed(delivery_id, payload_bytes, started_at.elapsed())
            }
            Err(error) => DeviceDeliveryAck::failed(delivery_id, true, started_at.elapsed(), error),
        };
        observer.delivery_terminal(&ack);
        ack
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

    /// Push an owned display payload to a connected device.
    ///
    /// The one display write on the backend; a backend that drives displays
    /// overrides it and decides per payload format what it can take.
    ///
    /// # Errors
    ///
    /// Returns an error if display output is unsupported or the write fails.
    async fn write_display_payload_owned(
        &self,
        id: &DeviceId,
        payload: Arc<OwnedDisplayFramePayload>,
    ) -> Result<(), DeviceError> {
        let _ = (id, payload);
        Err(DeviceError::Unsupported {
            backend: self.info().id,
            operation: "device display output",
        })
    }

    /// Adjust hardware brightness for a connected device, if supported.
    ///
    /// # Errors
    ///
    /// Returns an error if device-level brightness is unsupported or the write
    /// fails.
    async fn set_brightness(&self, id: &DeviceId, brightness: u8) -> Result<(), DeviceError> {
        let _ = (id, brightness);
        Err(DeviceError::Unsupported {
            backend: self.info().id,
            operation: "device brightness control",
        })
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
}
