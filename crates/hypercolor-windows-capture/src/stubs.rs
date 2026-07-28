//! Non-Windows stand-in so downstream crates compile unconditionally.

use std::sync::Arc;
use std::time::Duration;

use crate::shared::{
    CaptureError, CaptureExtent, CaptureRegion, CaptureResult, Frame, MonitorSelector,
    ReductionTelemetry,
};

/// Desktop Duplication placeholder for platforms without the API.
pub struct DesktopDuplicator {
    requested_extent: CaptureExtent,
}

impl DesktopDuplicator {
    /// Always fails: Desktop Duplication is Windows-only.
    ///
    /// # Errors
    ///
    /// Always returns [`CaptureError::UnsupportedPlatform`].
    pub const fn new(_monitor: usize, _requested_extent: CaptureExtent) -> CaptureResult<Self> {
        Err(CaptureError::UnsupportedPlatform)
    }

    /// Always fails: Desktop Duplication is Windows-only.
    ///
    /// # Errors
    ///
    /// Always returns [`CaptureError::UnsupportedPlatform`].
    pub const fn open(
        _selector: MonitorSelector,
        _requested_extent: CaptureExtent,
    ) -> CaptureResult<Self> {
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

    /// A stub has no live capture request.
    #[must_use]
    pub const fn requested_extent(&self) -> CaptureExtent {
        self.requested_extent
    }

    /// Ignore the requested extent on unsupported platforms.
    pub const fn set_requested_extent(&mut self, requested_extent: CaptureExtent) {
        self.requested_extent = requested_extent;
    }

    /// Always fails: no native capture extent exists on this platform.
    ///
    /// # Errors
    ///
    /// Always returns [`CaptureError::UnsupportedPlatform`].
    pub const fn set_region(&mut self, _region: Option<CaptureRegion>) -> CaptureResult<()> {
        Err(CaptureError::UnsupportedPlatform)
    }

    /// CPU fallback telemetry for the unsupported platform stub.
    #[must_use]
    pub fn reduction_telemetry(&self) -> ReductionTelemetry {
        ReductionTelemetry {
            issue: Some(Arc::from(
                "desktop screen capture is only available on Windows",
            )),
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
