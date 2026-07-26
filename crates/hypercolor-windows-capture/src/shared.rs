//! Platform-neutral surface: errors, frame view, and the subsample math.

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

/// A borrowed RGBA frame produced by the capture backend.
///
/// The pixel buffer is owned by the duplicator and reused between frames, so
/// consumers must copy anything they need to keep.
#[derive(Debug)]
pub struct Frame<'a> {
    /// Frame width in pixels, after subsampling.
    pub width: u32,
    /// Frame height in pixels, after subsampling.
    pub height: u32,
    /// Tightly packed RGBA8 pixels, `width * height * 4` bytes.
    pub rgba: &'a [u8],
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
