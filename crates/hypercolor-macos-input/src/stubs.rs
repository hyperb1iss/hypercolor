use crate::{
    EffectiveEventMasks, MacosInputBatch, MacosInputConfig, MacosInputDiagnostics, MacosInputError,
    MacosInputResult, MacosVirtualDesktop, MacosWorkerState,
};

/// Native event-tap session placeholder outside macOS.
pub struct MacosInputSession {
    _private: (),
}

impl MacosInputSession {
    /// Always fails because Core Graphics event taps are macOS-only.
    pub fn start(
        _config: MacosInputConfig,
        _sink: impl FnMut(MacosInputBatch<'_>) + Send + 'static,
    ) -> MacosInputResult<Self> {
        Err(MacosInputError::UnsupportedPlatform)
    }

    #[must_use]
    pub const fn effective_masks(&self) -> EffectiveEventMasks {
        EffectiveEventMasks {
            keyboard: 0,
            pointer: 0,
        }
    }

    #[must_use]
    pub fn worker_state(&self) -> MacosWorkerState {
        MacosWorkerState::Failed("macOS host input is unavailable".to_owned())
    }

    #[must_use]
    pub const fn diagnostics(&self) -> MacosInputDiagnostics {
        MacosInputDiagnostics {
            dropped_events: 0,
            tap_disable_count: 0,
            unsupported_system_events: 0,
            invalid_scroll_phases: 0,
            last_point_delta_x: 0,
            last_point_delta_y: 0,
        }
    }

    pub const fn stop(&mut self) {}
}

#[must_use]
pub const fn input_monitoring_granted() -> bool {
    false
}

#[must_use]
pub const fn request_input_monitoring() -> bool {
    false
}

pub fn current_virtual_desktop() -> MacosInputResult<MacosVirtualDesktop> {
    Err(MacosInputError::UnsupportedPlatform)
}
