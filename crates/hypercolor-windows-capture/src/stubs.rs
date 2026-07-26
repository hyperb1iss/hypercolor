//! Non-Windows stand-in so downstream crates compile unconditionally.

use std::time::Duration;

use crate::shared::{CaptureError, CaptureResult, Frame};

/// Desktop Duplication placeholder for platforms without the API.
pub struct DesktopDuplicator {
    _private: (),
}

impl DesktopDuplicator {
    /// Always fails: Desktop Duplication is Windows-only.
    ///
    /// # Errors
    ///
    /// Always returns [`CaptureError::UnsupportedPlatform`].
    pub const fn new(_monitor: usize, _max_width: u32) -> CaptureResult<Self> {
        Err(CaptureError::UnsupportedPlatform)
    }

    /// Monitor index this duplicator would be bound to.
    #[must_use]
    pub const fn monitor(&self) -> usize {
        0
    }

    /// Native desktop dimensions.
    #[must_use]
    pub const fn native_extent(&self) -> (u32, u32) {
        (0, 0)
    }

    /// Change the subsample target for subsequent frames.
    pub const fn set_max_width(&mut self, _max_width: u32) {}

    /// Always fails: Desktop Duplication is Windows-only.
    ///
    /// # Errors
    ///
    /// Always returns [`CaptureError::UnsupportedPlatform`].
    pub const fn next_frame(&mut self, _timeout: Duration) -> CaptureResult<Option<Frame<'_>>> {
        Err(CaptureError::UnsupportedPlatform)
    }
}
