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

    /// No attached output has the requested stable source id.
    #[error("display source {requested:?} is no longer attached")]
    SourceNotFound {
        /// Stable source id that was requested.
        requested: String,
    },

    /// Windows has no free Desktop Duplication client slot for this session.
    ///
    /// The operating system caps concurrent duplication clients per session.
    #[error("desktop duplication concurrency limit reached")]
    AlreadyDuplicating,

    /// The active desktop cannot currently be accessed, such as during UAC.
    #[error("the active Windows desktop is not accessible")]
    AccessDenied,

    /// The interactive Windows session disconnected or switched away.
    #[error("the interactive Windows session is disconnected")]
    SessionUnavailable,

    /// The graphics device was removed or reset.
    #[error("the capture graphics device was removed or reset")]
    DeviceLost,

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

/// Cursor metadata associated with an already-composited frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CursorInfo {
    /// Whether the backend reported a separately visible pointer.
    pub visible: bool,
    /// Pointer shape origin in native scanout coordinates.
    pub position_x: i32,
    /// Pointer shape origin in native scanout coordinates.
    pub position_y: i32,
    /// Hotspot offset transformed into the native scanout bounding box.
    pub hotspot_x: i32,
    /// Hotspot offset transformed into the native scanout bounding box.
    pub hotspot_y: i32,
    /// Visible pointer-shape width.
    pub width: u32,
    /// Visible pointer-shape height.
    pub height: u32,
    /// Monotonic shape generation within this duplication session.
    pub shape_generation: u64,
    /// Whether the returned RGBA plane already contains the pointer pixels.
    pub composed: bool,
}

type FramePool = Arc<Mutex<Vec<Vec<u8>>>>;

/// Owned RGBA frame produced by the capture backend.
///
/// The pixel allocation returns to the duplicator's pool when the final frame
/// owner drops it, so downstream adapters can retain the plane without copying.
#[derive(Debug)]
pub struct Frame {
    /// Stable id of the display that produced this frame.
    pub source_id: Arc<str>,
    /// Attached-output topology generation at acquisition.
    pub topology_generation: u64,
    /// Cursor state represented by this frame.
    pub cursor: CursorInfo,
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
        source_id: Arc<str>,
        topology_generation: u64,
        cursor: CursorInfo,
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
            source_id,
            topology_generation,
            cursor,
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
/// New callers persist `id`; `index` remains for ordering and legacy configs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonitorInfo {
    /// Legacy zero-based enumeration index for display ordering.
    pub index: usize,
    /// Stable source id used for persisted selection and capture epochs.
    pub id: String,
    /// OS device name, e.g. `\\.\DISPLAY1`.
    pub name: String,
    /// Desktop width in pixels.
    pub width: u32,
    /// Desktop height in pixels.
    pub height: u32,
    /// Horizontal origin in virtual-desktop coordinates.
    pub origin_x: i32,
    /// Vertical origin in virtual-desktop coordinates.
    pub origin_y: i32,
    /// Whether this output hosts the origin of the virtual desktop.
    pub primary: bool,
    /// Transform still pending on duplicated scanout pixels.
    pub rotation: DisplayRotation,
    /// Monotonic generation of the attached-output topology.
    pub topology_generation: u64,
}

/// A persisted display selection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MonitorSelector {
    /// Follow whichever attached output Windows marks primary.
    Auto,
    /// Follow one output across enumeration reorder by its stable id.
    StableId(String),
    /// Legacy adapter/output enumeration index.
    Index(usize),
}

impl MonitorSelector {
    /// Parse a configured capture source.
    #[must_use]
    pub fn parse(source: &str) -> Self {
        let source = source.trim();
        if source.is_empty() || source.eq_ignore_ascii_case("auto") {
            return Self::Auto;
        }

        if let Some(value) = source.strip_prefix("monitor:") {
            let value = value.trim();
            return value
                .parse::<usize>()
                .map_or_else(|_| Self::StableId(value.to_owned()), Self::Index);
        }
        if let Some(value) = source.strip_prefix("display:")
            && let Ok(index) = value.trim().parse::<usize>()
        {
            return Self::Index(index);
        }
        source
            .parse::<usize>()
            .map_or_else(|_| Self::StableId(source.to_owned()), Self::Index)
    }

    /// Resolve this selection against one topology snapshot.
    ///
    /// # Errors
    ///
    /// Returns [`CaptureError::MonitorNotFound`] for a legacy index outside
    /// the snapshot, or [`CaptureError::SourceNotFound`] for an absent stable
    /// id. `Auto` resolves the primary output even when it is not index zero.
    pub fn resolve<'a>(&self, monitors: &'a [MonitorInfo]) -> CaptureResult<&'a MonitorInfo> {
        match self {
            Self::Auto => monitors
                .iter()
                .find(|monitor| monitor.primary)
                .or_else(|| monitors.first())
                .ok_or(CaptureError::MonitorNotFound {
                    requested: 0,
                    available: 0,
                }),
            Self::StableId(requested) => monitors
                .iter()
                .find(|monitor| monitor.id == *requested)
                .ok_or_else(|| CaptureError::SourceNotFound {
                    requested: requested.clone(),
                }),
            Self::Index(requested) => {
                monitors
                    .get(*requested)
                    .ok_or(CaptureError::MonitorNotFound {
                        requested: *requested,
                        available: monitors.len(),
                    })
            }
        }
    }
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
/// Capture reduction averages each `stride` square into one output pixel. The
/// stride keeps the intermediate surface bounded without aliasing thin desktop
/// content before the later sector-grid reduction.
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
