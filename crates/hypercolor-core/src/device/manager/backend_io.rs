//! Backend I/O handles that can outlive the manager lock.

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use hypercolor_driver_api::{DeviceError, DiscoveredDevice};
use hypercolor_types::device::{DeviceId, DeviceInfo, OwnedDisplayFramePayload};

use crate::device::traits::{DeviceFrameSink, DeviceLifecyclePolicy, OutputCadence};

use super::BackendHandle;

/// Lightweight handle for backend I/O that can outlive the manager lock.
///
/// Clone this from [`super::BackendManager::backend_io`] while holding the
/// manager briefly, then perform the awaited backend call after releasing the
/// outer manager mutex.
#[derive(Clone)]
pub struct BackendIo {
    backend: BackendHandle,
}

impl BackendIo {
    pub(super) const fn new(backend: BackendHandle) -> Self {
        Self { backend }
    }

    /// Connect a device once using its already-adopted backend inventory.
    ///
    /// Returns the backend's preferred output cadence for the connected device.
    ///
    /// # Errors
    ///
    /// Returns an error if the backend connect call fails.
    pub async fn connect(&self, device_id: DeviceId) -> Result<OutputCadence, DeviceError> {
        self.connect_inner(device_id, None).await
    }

    /// Connect a device, applying timeout only to backend operations after
    /// this handle acquires the backend lock.
    ///
    /// Returns the backend's preferred output cadence for the connected device.
    ///
    /// # Errors
    ///
    /// Returns an error if the backend connect call fails or times out.
    pub async fn connect_with_timeout(
        &self,
        device_id: DeviceId,
        timeout: Duration,
    ) -> Result<OutputCadence, DeviceError> {
        self.connect_inner(device_id, Some(timeout)).await
    }

    async fn connect_inner(
        &self,
        device_id: DeviceId,
        timeout: Option<Duration>,
    ) -> Result<OutputCadence, DeviceError> {
        run_backend_operation(timeout, self.backend.connect(&device_id)).await?;

        Ok(self.backend.output_cadence(&device_id).unwrap_or_default())
    }

    /// Adopt a discovery result into backend-owned inventory.
    ///
    /// # Errors
    ///
    /// Returns an error when the backend cannot reconstruct its connection
    /// descriptor from the canonical discovery payload.
    pub fn adopt_device(&self, discovered: &DiscoveredDevice) -> Result<(), DeviceError> {
        self.backend.adopt_device(discovered)
    }

    /// Return backend lifecycle policy for a discovered device.
    pub fn lifecycle_policy(&self, info: &DeviceInfo) -> DeviceLifecyclePolicy {
        self.backend.lifecycle_policy(info)
    }

    /// Fetch refreshed metadata for a connected device.
    ///
    /// # Errors
    ///
    /// Returns an error if metadata retrieval fails.
    pub async fn connected_device_info(
        &self,
        device_id: DeviceId,
    ) -> Result<Option<DeviceInfo>, DeviceError> {
        self.backend.connected_device_info(&device_id).await
    }

    /// Clone the hot-path frame sink for a connected device, if the backend exposes one.
    pub fn frame_sink(&self, device_id: DeviceId) -> Option<Arc<dyn DeviceFrameSink>> {
        self.backend.frame_sink(&device_id)
    }

    /// Whether this backend can briefly connect an idle device for direct control.
    #[allow(
        clippy::unused_async,
        reason = "control capability lookups share the asynchronous backend I/O facade"
    )]
    pub async fn supports_temporary_direct_control(&self, info: &DeviceInfo) -> bool {
        self.backend.supports_temporary_direct_control(info)
    }

    /// Whether this backend consumes host-managed attachment profiles.
    pub fn supports_host_attachment_profiles(&self, info: &DeviceInfo) -> bool {
        self.backend.supports_host_attachment_profiles(info)
    }

    /// Disconnect a device from the backend.
    ///
    /// # Errors
    ///
    /// Returns an error if the backend disconnect call fails.
    pub async fn disconnect(&self, device_id: DeviceId) -> Result<(), DeviceError> {
        self.backend.disconnect(&device_id).await
    }

    /// Write immediate LED colors directly to the backend.
    ///
    /// # Errors
    ///
    /// Returns an error if the backend write fails.
    pub async fn write_colors(
        &self,
        device_id: DeviceId,
        colors: &[[u8; 3]],
    ) -> Result<(), DeviceError> {
        self.backend.write_colors(&device_id, colors).await
    }

    /// Set hardware brightness directly on the backend.
    ///
    /// # Errors
    ///
    /// Returns an error if the backend brightness write fails.
    pub async fn set_brightness(
        &self,
        device_id: DeviceId,
        brightness: u8,
    ) -> Result<(), DeviceError> {
        self.backend.set_brightness(&device_id, brightness).await
    }

    /// Write immediate display bytes directly to the backend.
    ///
    /// # Errors
    ///
    /// Returns an error if the display write fails.
    pub async fn write_display_frame(
        &self,
        device_id: DeviceId,
        jpeg_data: &[u8],
    ) -> Result<(), DeviceError> {
        self.backend
            .write_display_frame(&device_id, jpeg_data)
            .await
    }

    /// Write an owned display payload directly to the backend.
    ///
    /// # Errors
    ///
    /// Returns an error if the display write fails.
    pub async fn write_display_frame_owned(
        &self,
        device_id: DeviceId,
        jpeg_data: Arc<Vec<u8>>,
    ) -> Result<(), DeviceError> {
        self.backend
            .write_display_frame_owned(&device_id, jpeg_data)
            .await
    }

    /// Write an owned display payload directly to the backend.
    ///
    /// # Errors
    ///
    /// Returns an error if the display write fails.
    pub async fn write_display_payload_owned(
        &self,
        device_id: DeviceId,
        payload: Arc<OwnedDisplayFramePayload>,
    ) -> Result<(), DeviceError> {
        self.backend
            .write_display_payload_owned(&device_id, payload)
            .await
    }
}

async fn run_backend_operation<T, F>(timeout: Option<Duration>, future: F) -> Result<T, DeviceError>
where
    F: Future<Output = Result<T, DeviceError>>,
{
    let Some(timeout) = timeout else {
        return future.await;
    };

    let Ok(result) = tokio::time::timeout(timeout, future).await else {
        return Err(DeviceError::Timeout { after: timeout });
    };

    result
}
