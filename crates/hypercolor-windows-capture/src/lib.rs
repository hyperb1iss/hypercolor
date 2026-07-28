//! Windows desktop screen capture for Hypercolor, via DXGI Desktop Duplication.
//!
//! Desktop Duplication is the right primitive for ambient lighting: it is the
//! compositor's own presented output, it costs nothing when the screen is
//! static, and unlike Windows.Graphics.Capture it draws no yellow border and
//! shows no picker. It also needs no permission grant of any kind — there is
//! no Windows equivalent of the XDG portal handshake or macOS TCC prompt, so a
//! screen-reactive effect can simply work.
//!
//! This crate is deliberately thin: it owns the D3D11 device, the duplication
//! interface, and the staging readback, and hands out RGBA frames. Everything
//! downstream (sector grids, letterbox detection, smoothing) lives in
//! `hypercolor-core` and stays backend-agnostic. It exists as its own crate
//! because the workspace forbids `unsafe_code` and COM interop cannot.

#[cfg(target_os = "windows")]
mod duplication;

#[cfg(target_os = "windows")]
pub use duplication::DesktopDuplicator;

#[cfg(all(target_os = "windows", feature = "capture-bench"))]
#[doc(hidden)]
pub use duplication::gpu_reduction::{
    CaptureCadenceReport, CaptureReductionBenchmark, ReductionBenchmarkSample,
    classify_allocation_pressure_for_test,
};

mod shared;

pub use shared::{
    CaptureError, CaptureExtent, CaptureRegion, CaptureResult, CursorInfo, DisplayRotation, Frame,
    MonitorInfo, MonitorSelector, ReductionPath, ReductionTelemetry, list_monitors, monitor_count,
    subsample_stride, subsample_stride_within, subsampled_extent, width_target_within,
};

#[cfg(not(target_os = "windows"))]
mod stubs;

#[cfg(not(target_os = "windows"))]
pub use stubs::DesktopDuplicator;
