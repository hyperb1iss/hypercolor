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
#[cfg(target_os = "macos")]
mod screenshot;
mod session;
mod stream_contract;
mod worker;

#[cfg(target_os = "macos")]
pub use native::MacosScreenCaptureSession;
#[cfg(target_os = "macos")]
pub use screenshot::{
    MAX_MACOS_SCREENSHOT_REFERENCE_BYTES, MacosScreenshotPixelCopy,
    MacosScreenshotPreferredDynamicRange, MacosScreenshotReferenceCapability,
    MacosScreenshotReferenceImage, MacosScreenshotReferenceMetadata, MacosScreenshotReferenceSet,
};

pub use clock::{MacosDisplayClock, MacosDisplayClockError};
pub use cpu::MacosCpuSourceView;
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
pub use stream_contract::{
    MacosCaptureCapabilities, MacosCaptureDynamicRange, MacosConfiguredStream,
    MacosDeliveredFrameMetadata, MacosHostArchitecture, MacosRuntimeCapability,
    MacosStreamDeliveryRejection, MacosStreamDeliveryState, MacosStreamDeliveryValidator,
    MacosStreamPreset, MacosTahoeCapabilities, MacosTahoeRuntimeProbes,
    MacosTahoeSelectionCapabilities, MacosValidatedStreamDelivery,
};
