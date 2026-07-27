//! Platform-neutral surface: errors, frame view, and the subsample math.

use std::sync::{Arc, Mutex};

use thiserror::Error;

/// Screen capture result type.
pub type CaptureResult<T> = Result<T, CaptureError>;

/// Screen capture failures.
#[derive(Debug, Error)]
pub enum CaptureError {
    /// Desktop Duplication is a Windows-only API.
    #[error("desktop screen capture is only available on Windows")]
    UnsupportedPlatform,

    /// No display output matched the requested monitor index.
    #[error("monitor {requested} not found ({available} attached)")]
    MonitorNotFound {
        /// Zero-based monitor index that was requested.
        requested: usize,
        /// How many outputs enumeration actually found.
        available: usize,
    },

    /// Another process already holds the duplication interface for this output.
    ///
    /// Windows permits exactly one duplication per output per process, and a
    /// handful of tools (some capture utilities, other ambient-lighting apps)
    /// hold theirs for their whole lifetime.
    #[error("another application is already duplicating this display")]
    AlreadyDuplicating,

    /// A Windows API call failed.
    ///
    /// Carries a rendered message rather than the `windows` error type: with
    /// `default-features = false` that type does not implement
    /// `std::error::Error`, and the HRESULT text is the part worth keeping.
    #[error("{context}: {message}")]
    Windows {
        /// What we were attempting.
        context: &'static str,
        /// Rendered HRESULT description.
        message: String,
    },
}

// Only the cfg(windows) duplication module builds this variant, so the
// constructor must be gated with it: on Linux it would have no callers and
// the workspace's -D warnings turns dead code into a build failure.
#[cfg(target_os = "windows")]
impl CaptureError {
    /// Build a [`CaptureError::Windows`] from anything printable.
    pub(crate) fn windows(context: &'static str, message: impl std::fmt::Display) -> Self {
        Self::Windows {
            context,
            message: message.to_string(),
        }
    }
}

/// Pending display rotation reported by Desktop Duplication.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DisplayRotation {
    /// Pixels already share the logical display orientation.
    #[default]
    Identity,
    /// Rotate 90 degrees clockwise.
    Clockwise90,
    /// Rotate 180 degrees.
    Clockwise180,
    /// Rotate 270 degrees clockwise.
    Clockwise270,
}

type FramePool = Arc<Mutex<Vec<Vec<u8>>>>;

/// Owned RGBA frame produced by the capture backend.
///
/// The pixel allocation returns to the duplicator's pool when the final frame
/// owner drops it, so downstream adapters can retain the plane without copying.
#[derive(Debug)]
pub struct Frame {
    /// Frame width in pixels, after subsampling.
    pub width: u32,
    /// Frame height in pixels, after subsampling.
    pub height: u32,
    /// Native scanout width before subsampling.
    pub native_width: u32,
    /// Native scanout height before subsampling.
    pub native_height: u32,
    /// Horizontal origin in virtual-desktop coordinates.
    pub origin_x: i32,
    /// Vertical origin in virtual-desktop coordinates.
    pub origin_y: i32,
    /// Display transform still pending on the stored pixels.
    pub rotation: DisplayRotation,
    /// Tightly packed RGBA8 pixels, `width * height * 4` bytes.
    pub rgba: Vec<u8>,
    pool: FramePool,
}

#[cfg(target_os = "windows")]
impl Frame {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        width: u32,
        height: u32,
        native_width: u32,
        native_height: u32,
        origin_x: i32,
        origin_y: i32,
        rotation: DisplayRotation,
        rgba: Vec<u8>,
        pool: FramePool,
    ) -> Self {
        Self {
            width,
            height,
            native_width,
            native_height,
            origin_x,
            origin_y,
            rotation,
            rgba,
            pool,
        }
    }
}

impl AsRef<[u8]> for Frame {
    fn as_ref(&self) -> &[u8] {
        &self.rgba
    }
}

impl Drop for Frame {
    fn drop(&mut self) {
        let mut rgba = std::mem::take(&mut self.rgba);
        rgba.clear();
        self.pool
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(rgba);
    }
}

/// Number of attached display outputs, or zero when capture is unavailable.
#[must_use]
pub fn monitor_count() -> usize {
    #[cfg(target_os = "windows")]
    {
        crate::duplication::output_count().unwrap_or(0)
    }
    #[cfg(not(target_os = "windows"))]
    {
        0
    }
}

/// One attached display output, in capture index order.
///
/// `index` is the value a [`crate::DesktopDuplicator`] accepts as its
/// `monitor` argument, so a UI can enumerate here and open what it showed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonitorInfo {
    /// Zero-based capture index (adapter-then-output enumeration order).
    pub index: usize,
    /// OS device name, e.g. `\\.\DISPLAY1`.
    pub name: String,
    /// Desktop width in pixels.
    pub width: u32,
    /// Desktop height in pixels.
    pub height: u32,
    /// Whether this output hosts the origin of the virtual desktop.
    pub primary: bool,
}

/// Describe every attached display output.
///
/// Empty when capture is unavailable (non-Windows, headless, RDP), so
/// callers can use emptiness itself as "this platform has no monitor
/// picker" rather than needing a separate capability probe.
#[must_use]
pub fn list_monitors() -> Vec<MonitorInfo> {
    #[cfg(target_os = "windows")]
    {
        crate::duplication::describe_outputs().unwrap_or_default()
    }
    #[cfg(not(target_os = "windows"))]
    {
        Vec::new()
    }
}

/// Integer subsample stride that brings `source` at or under `target`.
///
/// Ambient lighting reduces the frame to a coarse sector grid, so point
/// sampling every Nth pixel is indistinguishable from a box filter in the
/// output while letting readback skip most of the mapped staging rows. That
/// matters: a 4K desktop is 33 MB per frame, and mapped staging memory is
/// write-combined, so untouched rows are the cheapest possible rows.
#[must_use]
pub fn subsample_stride(source: u32, target: u32) -> u32 {
    if target == 0 || source <= target {
        return 1;
    }
    source.div_ceil(target).max(1)
}

/// Dimension after applying `stride` to `source`.
#[must_use]
pub const fn subsampled_extent(source: u32, stride: u32) -> u32 {
    if stride <= 1 {
        return source;
    }
    source.div_ceil(stride)
}
