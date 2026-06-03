use std::sync::Arc;

use anyhow::{Result, bail};
use hypercolor_types::device::{DeviceId, OwnedDisplayFramePayload};

use super::BackendManager;

impl BackendManager {
    /// Write one immediate JPEG display payload to a specific physical device.
    ///
    /// This bypasses spatial routing and targets display-capable backends
    /// directly for screen/LCD updates.
    ///
    /// # Errors
    ///
    /// Returns an error when the backend is missing or the backend write fails.
    pub async fn write_device_display_frame(
        &mut self,
        backend_id: &str,
        device_id: DeviceId,
        jpeg_data: &[u8],
    ) -> Result<()> {
        let Some(io) = self.backend_io(backend_id) else {
            bail!("backend '{backend_id}' is not registered");
        };
        io.write_display_frame(device_id, jpeg_data).await
    }

    /// Write one owned JPEG display payload to a specific physical device.
    ///
    /// # Errors
    ///
    /// Returns an error when the backend is missing or the backend write fails.
    pub async fn write_device_display_frame_owned(
        &mut self,
        backend_id: &str,
        device_id: DeviceId,
        jpeg_data: Arc<Vec<u8>>,
    ) -> Result<()> {
        let Some(io) = self.backend_io(backend_id) else {
            bail!("backend '{backend_id}' is not registered");
        };
        io.write_display_frame_owned(device_id, jpeg_data).await
    }

    /// Write one owned display payload to a specific physical device.
    ///
    /// # Errors
    ///
    /// Returns an error when the backend is missing or the backend write fails.
    pub async fn write_device_display_payload_owned(
        &mut self,
        backend_id: &str,
        device_id: DeviceId,
        payload: Arc<OwnedDisplayFramePayload>,
    ) -> Result<()> {
        let Some(io) = self.backend_io(backend_id) else {
            bail!("backend '{backend_id}' is not registered");
        };
        io.write_display_payload_owned(device_id, payload).await
    }
}
