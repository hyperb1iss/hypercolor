//! ScreenCaptureKit acquisition vocabulary and frame validation.
//!
//! Native framework ownership remains private to this crate. The public frame
//! boundary contains only plain Rust metadata plus an opaque retained surface.

mod clock;
mod cpu;
mod diagnostics;
mod frame;
mod geometry;
mod mailbox;
#[cfg(target_os = "macos")]
mod native;
mod session;
mod worker;

#[cfg(target_os = "macos")]
pub use native::MacosScreenCaptureSession;

pub use clock::{MacosDisplayClock, MacosDisplayClockError};
pub use diagnostics::{MacosCaptureCallbackDiagnostics, MacosFrameDropReason};
#[cfg(target_os = "macos")]
pub use frame::MacosNativeSurfaceLease;
pub use frame::{
    MACOS_STREAM_QUEUE_DEPTH, MacosAttachment, MacosCaptureColorimetry, MacosCaptureError,
    MacosCaptureFrame, MacosCapturePixelFormat, MacosCapturePlane, MacosCaptureSurface,
    MacosChromaLocation, MacosColorPrimaries, MacosColorRange, MacosFrameDecoder, MacosFrameEvent,
    MacosFrameStatus, MacosProtectedSourceState, MacosRawCapturePlane, MacosRawCaptureSample,
    MacosRawCompleteFrame, MacosRawFrameAttachments, MacosTransferFunction, MacosYuvMatrix,
};
pub use geometry::{
    MacosCaptureGeometry, MacosGeometryError, MacosPixelExtent, MacosPixelRect, MacosPointRect,
    MacosScale,
};
pub use mailbox::MacosFrameMailbox;
pub use session::{
    MacosCaptureCadence, MacosCaptureContentStyle, MacosCaptureSelection, MacosCaptureSelector,
    MacosStreamRequest,
};
