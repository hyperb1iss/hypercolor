//! Platform-neutral surface: errors, frame view, and the subsample math.

use std::sync::{Arc, Mutex};
use std::time::Instant;

use thiserror::Error;

/// Screen capture result type.
pub type CaptureResult<T> = Result<T, CaptureError>;

/// Active Windows capture reduction implementation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ReductionPath {
    /// D3D11 compute reduction with pipelined reduced-surface readback.
    Gpu,
    /// Full-quality CPU box reduction used when the GPU path is unavailable.
    #[default]
    CpuFallback,
}

/// Snapshot of capture reduction health and throughput counters.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReductionTelemetry {
    /// Currently active implementation.
    pub path: ReductionPath,
    /// GPU reductions submitted to the immediate context.
    pub gpu_submitted: u64,
    /// GPU reductions whose reduced surface reached the CPU.
    pub gpu_completed: u64,
    /// Frames reduced by the full-quality CPU fallback.
    pub cpu_completed: u64,
    /// Submissions coalesced because every staging slot was still busy.
    pub ring_busy: u64,
    /// Bytes copied from reduced staging surfaces to CPU memory.
    pub readback_bytes: u64,
    /// GPU initialization or execution failures that selected fallback.
    pub gpu_failures: u64,
    /// Degraded-path reason, absent while GPU reduction is healthy.
    pub issue: Option<Arc<str>>,
}

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

    /// The duplicated desktop changed and the capture session must reopen it.
    #[error("desktop duplication access was lost during a display transition")]
    AccessLost,

    /// A Windows capture operation exceeded its wait budget.
    #[error("the Windows capture operation timed out")]
    Timeout,

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

/// Native scanout rectangle selected for capture reduction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CaptureRegion {
    origin_x: u32,
    origin_y: u32,
    width: u32,
    height: u32,
}

impl CaptureRegion {
    /// Construct a non-empty native scanout rectangle.
    #[must_use]
    pub const fn new(origin_x: u32, origin_y: u32, width: u32, height: u32) -> Option<Self> {
        if width == 0 || height == 0 {
            return None;
        }
        Some(Self {
            origin_x,
            origin_y,
            width,
            height,
        })
    }

    pub(crate) const fn full(width: u32, height: u32) -> Self {
        Self {
            origin_x: 0,
            origin_y: 0,
            width,
            height,
        }
    }

    /// Horizontal origin in native scanout coordinates.
    #[must_use]
    pub const fn origin_x(self) -> u32 {
        self.origin_x
    }

    /// Vertical origin in native scanout coordinates.
    #[must_use]
    pub const fn origin_y(self) -> u32 {
        self.origin_y
    }

    /// Selected width in native scanout pixels.
    #[must_use]
    pub const fn width(self) -> u32 {
        self.width
    }

    /// Selected height in native scanout pixels.
    #[must_use]
    pub const fn height(self) -> u32 {
        self.height
    }

    pub(crate) fn fits_within(self, width: u32, height: u32) -> bool {
        self.origin_x
            .checked_add(self.width)
            .is_some_and(|right| right <= width)
            && self
                .origin_y
                .checked_add(self.height)
                .is_some_and(|bottom| bottom <= height)
    }
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
    /// Monotonic capture sequence assigned when Desktop Duplication acquires the state.
    pub sequence: u64,
    /// Time Desktop Duplication acquired the state, before asynchronous reduction.
    pub captured_at: Instant,
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
        sequence: u64,
        captured_at: Instant,
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
            sequence,
            captured_at,
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

    /// Convert a resolved legacy index into its stable persisted form.
    #[must_use]
    pub fn canonical_source(&self, resolved_source_id: &str) -> Option<String> {
        matches!(self, Self::Index(_)).then(|| format!("monitor:{resolved_source_id}"))
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

#[cfg(all(test, target_os = "windows"))]
mod tests {
    use super::*;

    #[test]
    fn native_frame_owns_acquisition_sequence_and_time() {
        let captured_at = Instant::now();
        let pool = Arc::new(Mutex::new(Vec::new()));
        let frame = Frame::new(
            Arc::from("display:test"),
            3,
            41,
            captured_at,
            CursorInfo::default(),
            1,
            1,
            1,
            1,
            0,
            0,
            DisplayRotation::Identity,
            vec![1, 2, 3, 0xFF],
            pool,
        );

        assert_eq!(frame.sequence, 41);
        assert_eq!(frame.captured_at, captured_at);
    }
}
