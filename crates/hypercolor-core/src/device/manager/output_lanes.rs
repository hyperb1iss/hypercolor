use hypercolor_types::device::DeviceId;

use super::{BackendDeviceKey, BackendManager, DeviceFrameSinkHandle};

impl BackendManager {
    /// Cache a backend-provided hot-path frame sink for a physical device.
    pub fn set_device_frame_sink(
        &mut self,
        backend_id: &str,
        device_id: DeviceId,
        frame_sink: Option<DeviceFrameSinkHandle>,
    ) {
        self.output
            .set_frame_sink(backend_id, device_id, frame_sink);
    }

    /// Cache a backend-provided output FPS for a physical device.
    pub fn set_cached_target_fps(
        &mut self,
        backend_id: &str,
        device_id: DeviceId,
        target_fps: u32,
    ) {
        self.output
            .set_target_fps(backend_id, device_id, target_fps);
    }

    /// Return the cached target FPS for a connected physical device, if present.
    #[must_use]
    pub fn cached_target_fps(&self, backend_id: &str, device_id: DeviceId) -> Option<u32> {
        self.output.target_fps(backend_id, device_id)
    }

    /// Suppress queued frame writes for a specific physical device.
    ///
    /// Returns the active direct-control lock count after incrementing.
    pub fn begin_direct_control(&mut self, backend_id: &str, device_id: DeviceId) -> usize {
        self.output.begin_direct_control(backend_id, device_id)
    }

    /// Release one direct-control lock for a specific physical device.
    ///
    /// Returns the remaining lock count after decrementing.
    pub fn end_direct_control(&mut self, backend_id: &str, device_id: DeviceId) -> usize {
        self.output.end_direct_control(backend_id, device_id)
    }

    /// Whether queued frame writes are currently suppressed for a device.
    #[must_use]
    pub fn is_direct_control_active(&self, backend_id: &str, device_id: DeviceId) -> bool {
        self.is_direct_control_active_key(&(backend_id.to_owned(), device_id))
    }

    pub(super) fn is_direct_control_active_key(&self, key: &BackendDeviceKey) -> bool {
        self.output.is_direct_control_active_key(key)
    }
}
