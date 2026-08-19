use crate::{
    DeviceOpenStatus, EvdevInputBatch, EvdevInputConfig, EvdevInputError, EvdevInputResult,
    EvdevWorkerState,
};

/// Native evdev session placeholder outside Linux.
pub struct EvdevInputSession {
    _private: (),
}

impl EvdevInputSession {
    /// Always fails because evdev is Linux-only.
    pub fn start(
        _config: EvdevInputConfig,
        _sink: impl FnMut(EvdevInputBatch<'_>) + Send + 'static,
    ) -> EvdevInputResult<Self> {
        Err(EvdevInputError::UnsupportedPlatform)
    }

    #[must_use]
    pub const fn device_count(&self) -> usize {
        0
    }

    #[must_use]
    pub const fn topology_generation(&self) -> u64 {
        0
    }

    #[must_use]
    pub fn device_status(&self) -> Vec<DeviceOpenStatus> {
        Vec::new()
    }

    #[must_use]
    pub fn worker_state(&self) -> EvdevWorkerState {
        EvdevWorkerState::Failed("evdev host input is unavailable".to_owned())
    }

    pub const fn stop(&mut self) {}
}
