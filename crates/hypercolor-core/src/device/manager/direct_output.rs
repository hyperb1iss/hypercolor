use anyhow::{Result, bail};
use hypercolor_types::device::DeviceId;

use super::BackendManager;

impl BackendManager {
    /// Write one immediate color payload to a specific physical device.
    ///
    /// This bypasses spatial routing and output queues, and is intended for
    /// short, direct control operations like identify/flash actions.
    ///
    /// # Errors
    ///
    /// Returns an error when the backend is missing or the backend write fails.
    pub async fn write_device_colors(
        &mut self,
        backend_id: &str,
        device_id: DeviceId,
        colors: &[[u8; 3]],
    ) -> Result<()> {
        let Some(io) = self.backend_io(backend_id) else {
            bail!("backend '{backend_id}' is not registered");
        };
        io.write_colors(device_id, colors).await
    }

    /// Adjust hardware brightness for a specific physical device.
    ///
    /// This bypasses spatial routing and targets the backend directly.
    ///
    /// # Errors
    ///
    /// Returns an error when the backend is missing or the backend write fails.
    pub async fn set_device_brightness(
        &mut self,
        backend_id: &str,
        device_id: DeviceId,
        brightness: u8,
    ) -> Result<()> {
        let Some(io) = self.backend_io(backend_id) else {
            bail!("backend '{backend_id}' is not registered");
        };
        io.set_brightness(device_id, brightness).await
    }
}
