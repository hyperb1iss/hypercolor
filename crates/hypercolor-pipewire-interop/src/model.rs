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
    use super::{D4Transform, PackedVideoFormat};

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
}
