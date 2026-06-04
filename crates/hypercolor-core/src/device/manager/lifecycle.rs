use anyhow::{Result, bail};
use hypercolor_types::device::{DeviceId, DeviceInfo};

use super::BackendManager;

impl BackendManager {
    /// Connect a physical device and map it to a layout device identifier.
    ///
    /// This keeps connect + map as a single operation so discovery/lifecycle
    /// code can avoid split-brain states.
    ///
    /// # Errors
    ///
    /// Returns an error if the backend is missing or the backend connect call
    /// fails.
    pub async fn connect_device(
        &mut self,
        backend_id: &str,
        device_id: DeviceId,
        layout_device_id: &str,
    ) -> Result<()> {
        let Some(io) = self.backend_io(backend_id) else {
            bail!("backend '{backend_id}' is not registered");
        };
        let target_fps = io.connect_with_refresh(device_id).await?;
        let frame_sink = io.frame_sink(device_id).await;
        self.set_cached_target_fps(backend_id, device_id, target_fps);
        self.set_device_frame_sink(backend_id, device_id, frame_sink);

        self.map_device(
            layout_device_id.to_owned(),
            backend_id.to_owned(),
            device_id,
        );
        Ok(())
    }

    /// Query refreshed metadata for a connected physical device.
    ///
    /// Backends can use this to expose connect-time topology discovery back
    /// to the daemon after a successful handshake.
    ///
    /// # Errors
    ///
    /// Returns an error if the backend is missing or metadata retrieval fails.
    pub async fn connected_device_info(
        &self,
        backend_id: &str,
        device_id: DeviceId,
    ) -> Result<Option<DeviceInfo>> {
        let Some(io) = self.backend_io(backend_id) else {
            bail!("backend '{backend_id}' is not registered");
        };
        io.connected_device_info(device_id).await
    }

    /// Disconnect a physical device and unmap its layout device identifier.
    ///
    /// # Errors
    ///
    /// Returns an error if the backend is missing or disconnect fails.
    pub async fn disconnect_device(
        &mut self,
        backend_id: &str,
        device_id: DeviceId,
        _layout_device_id: &str,
    ) -> Result<()> {
        let Some(io) = self.backend_io(backend_id) else {
            bail!("backend '{backend_id}' is not registered");
        };

        let disconnect_result = io.disconnect(device_id).await;
        let _ = self.remove_device_mappings_for_physical(backend_id, device_id);
        disconnect_result
    }
}
