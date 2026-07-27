//! Non-Windows stand-in so downstream crates compile unconditionally.

use std::time::Duration;

use crate::shared::{CaptureError, CaptureResult, Frame, MonitorSelector, ReductionTelemetry};

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

    /// Always fails: Desktop Duplication is Windows-only.
    ///
    /// # Errors
    ///
    /// Always returns [`CaptureError::UnsupportedPlatform`].
    pub fn open(_selector: MonitorSelector, _max_width: u32) -> CaptureResult<Self> {
        Err(CaptureError::UnsupportedPlatform)
    }

    /// Monitor index this duplicator would be bound to.
    #[must_use]
    pub const fn monitor(&self) -> usize {
        0
    }

    /// Empty because no platform source can be opened.
    #[must_use]
    pub const fn source_id(&self) -> &str {
        ""
    }

    /// Zero because no platform topology exists.
    #[must_use]
    pub const fn topology_generation(&self) -> u64 {
        0
    }

    /// Native desktop dimensions.
    #[must_use]
    pub const fn native_extent(&self) -> (u32, u32) {
        (0, 0)
    }

    /// Logical desktop dimensions.
    #[must_use]
    pub const fn logical_extent(&self) -> (u32, u32) {
        (0, 0)
    }

    /// Zero because no duplication interface exists.
    #[must_use]
    pub const fn duplication_generation(&self) -> u64 {
        0
    }

    /// Change the subsample target for subsequent frames.
    pub const fn set_max_width(&mut self, _max_width: u32) {}

    /// CPU fallback telemetry for the unsupported platform stub.
    #[must_use]
    pub fn reduction_telemetry(&self) -> ReductionTelemetry {
        ReductionTelemetry {
            issue: Some("desktop screen capture is only available on Windows".to_owned()),
            ..ReductionTelemetry::default()
        }
    }

    /// Always fails: Desktop Duplication is Windows-only.
    ///
    /// # Errors
    ///
    /// Always returns [`CaptureError::UnsupportedPlatform`].
    pub const fn next_frame(&mut self, _timeout: Duration) -> CaptureResult<Option<Frame>> {
        Err(CaptureError::UnsupportedPlatform)
    }
}
