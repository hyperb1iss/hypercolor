//! ScreenCaptureKit acquisition vocabulary and frame validation.
//!
//! Native framework ownership remains private to this crate. The public frame
//! boundary contains only plain Rust metadata plus an opaque retained surface.

mod frame;
mod geometry;

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
