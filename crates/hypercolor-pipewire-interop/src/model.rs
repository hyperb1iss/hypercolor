/// Packed raw pixel formats accepted by the capture boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PackedVideoFormat {
    /// Red, green, blue, alpha.
    Rgba,
    /// Blue, green, red, alpha.
    Bgra,
    /// Red, green, blue, ignored.
    Rgbx,
    /// Blue, green, red, ignored.
    Bgrx,
    /// Alpha, red, green, blue.
    Argb,
    /// Alpha, blue, green, red.
    Abgr,
    /// Ignored, red, green, blue.
    Xrgb,
    /// Ignored, blue, green, red.
    Xbgr,
    /// Red, green, blue.
    Rgb,
    /// Blue, green, red.
    Bgr,
}

impl PackedVideoFormat {
    /// Returns the byte width of one packed pixel.
    #[must_use]
    pub const fn bytes_per_pixel(self) -> usize {
        match self {
            Self::Rgb | Self::Bgr => 3,
            Self::Rgba
            | Self::Bgra
            | Self::Rgbx
            | Self::Bgrx
            | Self::Argb
            | Self::Abgr
            | Self::Xrgb
            | Self::Xbgr => 4,
        }
    }
}

/// Rational video cadence reported by the producer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VideoFraction {
    /// Numerator.
    pub numerator: u32,
    /// Denominator.
    pub denominator: u32,
}

/// Exact packed format negotiated by the native stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NegotiatedVideoFormat {
    /// Storage width in pixels.
    pub width: u32,
    /// Storage height in pixels.
    pub height: u32,
    /// Packed pixel format.
    pub format: PackedVideoFormat,
    /// Producer cadence. A zero numerator means variable cadence.
    pub framerate: VideoFraction,
}

/// Exact format requested from the native stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CaptureFormatRequest {
    /// Requested storage width in pixels.
    pub width: u32,
    /// Requested storage height in pixels.
    pub height: u32,
    /// Requested maximum cadence.
    pub target_fps: u32,
}

/// Opaque native format offer derived from an exact capture request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FormatOffer {
    pub(crate) request: CaptureFormatRequest,
}

impl FormatOffer {
    /// Creates one exact native format offer.
    ///
    /// # Errors
    ///
    /// Returns a typed error when the extent or preferred cadence is zero.
    pub fn new(request: CaptureFormatRequest) -> Result<Self, FormatOfferError> {
        if request.width == 0 || request.height == 0 {
            return Err(FormatOfferError::ZeroExtent);
        }
        if request.target_fps == 0 {
            return Err(FormatOfferError::ZeroCadence);
        }
        Ok(Self { request })
    }

    /// Returns the neutral request carried by this offer.
    #[must_use]
    pub const fn request(&self) -> CaptureFormatRequest {
        self.request
    }
}

/// Invalid native format-offer request.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum FormatOfferError {
    /// The requested storage extent has a zero axis.
    #[error("capture format extent must be nonzero")]
    ZeroExtent,
    /// The preferred capture cadence is zero.
    #[error("capture format cadence must be nonzero")]
    ZeroCadence,
}

/// Native format-change event translated without exposing SPA objects.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FormatEvent {
    /// The producer removed its negotiated format.
    Removed,
    /// The producer supplied a malformed or unsupported format.
    Invalid(FormatFault),
    /// The producer fixed one supported packed video format.
    Negotiated(NegotiatedVideoFormat),
}

/// Reason a native format event could not be translated.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FormatFault {
    /// The native format pod could not be decoded.
    Unreadable,
    /// The media type or subtype was not raw video.
    NonRawVideo,
    /// The raw-video body did not contain valid format information.
    InvalidRawVideo,
    /// The negotiated packed pixel format is unsupported.
    UnsupportedPixelFormat,
}

/// Neutral lifecycle state for one native stream.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StreamState {
    /// No native link exists.
    Unconnected,
    /// The native link is being established.
    Connecting,
    /// The native link exists but is not producing buffers.
    Paused,
    /// The native link is producing buffers.
    Streaming,
    /// The native stream entered a terminal error state.
    Error(String),
}

/// One exact native stream-state transition.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StateChange {
    /// State observed before the transition.
    pub previous: StreamState,
    /// State observed after the transition.
    pub current: StreamState,
}

/// Action requested by a synchronous native callback.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CallbackAction {
    /// Keep servicing the native stream.
    Continue,
    /// Quit the native stream loop after this callback returns.
    Quit,
}

/// Failure while constructing or controlling a native capture stream.
#[derive(Debug, thiserror::Error)]
pub enum StreamError {
    /// Native stream support is unavailable on this host.
    #[error("native capture streams are unsupported on this platform")]
    UnsupportedPlatform,
    /// One native operation failed.
    #[error("{operation}: {detail}")]
    Operation {
        /// Stable operation label.
        operation: &'static str,
        /// Native error detail.
        detail: String,
    },
}

/// Crop rectangle in native storage coordinates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PixelCrop {
    /// Horizontal origin.
    pub x: u32,
    /// Vertical origin.
    pub y: u32,
    /// Crop width.
    pub width: u32,
    /// Crop height.
    pub height: u32,
}

/// Full dihedral transform vocabulary carried by native video metadata.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum D4Transform {
    /// Identity transform.
    #[default]
    Identity,
    /// Ninety-degree clockwise rotation.
    Clockwise90,
    /// One-hundred-eighty-degree rotation.
    Clockwise180,
    /// Two-hundred-seventy-degree clockwise rotation.
    Clockwise270,
    /// Horizontal reflection.
    Flipped,
    /// Horizontal reflection followed by a ninety-degree clockwise rotation.
    Flipped90,
    /// Horizontal reflection followed by a one-hundred-eighty-degree rotation.
    Flipped180,
    /// Horizontal reflection followed by a two-hundred-seventy-degree rotation.
    Flipped270,
}

impl D4Transform {
    /// Returns whether the transform swaps the storage axes.
    #[must_use]
    pub const fn swaps_axes(self) -> bool {
        matches!(
            self,
            Self::Clockwise90 | Self::Clockwise270 | Self::Flipped90 | Self::Flipped270
        )
    }
}

/// Validated borrowed frame presented during one native dequeue.
#[derive(Clone, Copy, Debug)]
pub struct FrameView<'a> {
    /// Mapped storage containing the frame chunk.
    pub data: &'a [u8],
    /// Byte offset of the chunk within `data`.
    pub offset: usize,
    /// Byte length of the chunk.
    pub size: usize,
    /// Signed native row stride.
    pub stride: i32,
    /// Negotiated packed format.
    pub format: NegotiatedVideoFormat,
    /// Optional crop metadata. A malformed value is carried as a typed fault.
    pub crop: Option<Result<PixelCrop, MetaFault>>,
    /// Optional transform metadata. A malformed value is carried as a typed fault.
    pub transform: Option<Result<D4Transform, MetaFault>>,
}

/// Native buffer validation fault before the visitor runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BufferFault {
    /// The stream had no buffer ready.
    MissingBuffer,
    /// The dequeued wrapper did not contain a native buffer.
    MissingNativeBuffer,
    /// The native buffer did not contain a pixel plane.
    MissingPlane,
    /// The first plane did not contain a chunk descriptor.
    MissingChunk,
    /// The first plane was not mapped into this process.
    UnmappedPlane,
    /// Native counts or pointers could not form bounded slices.
    InvalidLayout,
    /// Chunk offset or size conversion failed.
    InvalidChunkBounds,
}

/// Fault in optional native frame metadata.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MetaFault {
    /// Metadata storage was smaller than the declared structure.
    Undersized,
    /// Metadata storage was not aligned for the declared structure.
    Misaligned,
    /// Crop coordinates or dimensions were negative or overflowed.
    InvalidCrop,
    /// Transform metadata carried an unknown native identifier.
    InvalidTransform,
}

/// Result of one exact dequeue and visitor boundary.
#[derive(Debug)]
pub enum DequeueOutcome<V> {
    /// No buffer was ready.
    Empty,
    /// Native validation failed before the visitor ran.
    Faulted(BufferFault),
    /// The visitor completed normally.
    Visited(V),
    /// The visitor panicked and the panic was contained inside the boundary.
    VisitorPanicked,
}

#[cfg(test)]
mod tests {
    use super::{CaptureFormatRequest, D4Transform, FormatOffer, PackedVideoFormat};

    #[test]
    fn packed_formats_preserve_exact_pixel_widths() {
        assert_eq!(PackedVideoFormat::Rgb.bytes_per_pixel(), 3);
        assert_eq!(PackedVideoFormat::Bgr.bytes_per_pixel(), 3);
        assert_eq!(PackedVideoFormat::Rgba.bytes_per_pixel(), 4);
        assert_eq!(PackedVideoFormat::Xbgr.bytes_per_pixel(), 4);
    }

    #[test]
    fn d4_axis_swaps_are_exhaustive() {
        assert!(!D4Transform::Identity.swaps_axes());
        assert!(D4Transform::Clockwise90.swaps_axes());
        assert!(!D4Transform::Clockwise180.swaps_axes());
        assert!(D4Transform::Clockwise270.swaps_axes());
        assert!(!D4Transform::Flipped.swaps_axes());
        assert!(D4Transform::Flipped90.swaps_axes());
        assert!(!D4Transform::Flipped180.swaps_axes());
        assert!(D4Transform::Flipped270.swaps_axes());
    }

    #[test]
    fn format_offer_rejects_zero_axes_and_cadence() {
        let request = CaptureFormatRequest {
            width: 0,
            height: 1080,
            target_fps: 60,
        };
        assert!(FormatOffer::new(request).is_err());

        let request = CaptureFormatRequest {
            width: 1920,
            height: 1080,
            target_fps: 0,
        };
        assert!(FormatOffer::new(request).is_err());
    }
}
