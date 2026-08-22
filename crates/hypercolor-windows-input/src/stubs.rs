//! Non-Windows stand-ins so `hypercolor-core` compiles unconditionally.
//!
//! Core's Windows host source is `#[cfg]`-gated, but the shared vocabulary in
//! [`crate::shared`] and the arithmetic in [`crate::decode`] are not — that is
//! the point of the split, and it is what lets Linux CI test the decoding.

use crate::shared::{RawInputConfig, RawInputError, RawInputResult, SessionState, WorkerState};
use hypercolor_types::host_input::HostInputBatch;

/// Raw Input session placeholder for platforms without the API.
pub struct RawInputSession {
    _private: (),
}

impl RawInputSession {
    /// Always fails: Raw Input is Windows-only.
    ///
    /// # Errors
    ///
    /// Always returns [`RawInputError::UnsupportedPlatform`].
    pub fn start(
        _config: RawInputConfig,
        _sink: impl FnMut(HostInputBatch<'_>) + Send + 'static,
    ) -> RawInputResult<Self> {
        Err(RawInputError::UnsupportedPlatform)
    }

    /// Devices registered, identified, and streaming.
    #[must_use]
    pub const fn device_count(&self) -> usize {
        0
    }

    /// Liveness of the pump thread.
    #[must_use]
    pub fn worker_state(&self) -> WorkerState {
        WorkerState::Failed("Raw Input is only available on Windows".to_owned())
    }

    /// Idempotent, like the Windows implementation.
    pub const fn stop(&mut self) {}
}

/// Always reports no interactive session off Windows.
#[must_use]
pub const fn interactive_session_state() -> SessionState {
    SessionState::NoInteractiveSession
}
