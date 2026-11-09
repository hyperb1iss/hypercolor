use hypercolor_driver_api::OutputPolicy;
use hypercolor_types::device::DeviceId;

use super::{BackendDeviceKey, BackendManager, DeviceFrameSinkHandle, DirectControlGuard};

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

    /// Cache a backend-provided output policy for a physical device.
    pub fn set_cached_output_policy(
        &mut self,
        backend_id: &str,
        device_id: DeviceId,
        output_policy: OutputPolicy,
    ) {
        self.output
            .set_output_policy(backend_id, device_id, output_policy);
    }

    /// Return the display frame rate of a connected physical device, if cached.
    #[must_use]
    pub fn cached_display_fps(&self, backend_id: &str, device_id: DeviceId) -> Option<u32> {
        self.output.display_fps(backend_id, device_id)
    }

    /// Return the cached output policy for a connected physical device, if present.
    #[must_use]
    pub fn cached_output_policy(
        &self,
        backend_id: &str,
        device_id: DeviceId,
    ) -> Option<OutputPolicy> {
        self.output.output_policy(backend_id, device_id)
    }

    /// Suppress queued frame writes for a specific physical device.
    ///
    /// The returned guard releases suppression when dropped, including during
    /// task cancellation and panic unwinding.
    pub fn begin_direct_control(
        &self,
        backend_id: &str,
        device_id: DeviceId,
    ) -> DirectControlGuard {
        self.output.begin_direct_control(backend_id, device_id)
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
