//! Backend-neutral screen capture frame contract.

use std::any::Any;
use std::fmt;
use std::marker::PhantomData;
use std::num::{NonZeroU32, NonZeroU64};
use std::ops::{Deref, DerefMut};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Instant;

use thiserror::Error;

use super::admission::{
    ScreenByteAdmissionCoordinator, ScreenByteAdmissionError, ScreenByteLease,
    ScreenByteReservation,
};
use super::plan::ScreenResourceLifetime;

/// Stable logical identity of one capture source.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct CaptureSourceId(Arc<str>);

impl CaptureSourceId {
    /// Create a non-empty source identity.
    ///
    /// # Errors
    ///
    /// Returns [`CaptureFrameError::EmptySourceId`] for empty or whitespace-only ids.
    pub fn new(value: impl Into<Arc<str>>) -> Result<Self, CaptureFrameError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(CaptureFrameError::EmptySourceId);
        }
        Ok(Self(value))
    }

    /// Borrow the stable identifier.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for CaptureSourceId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Positive two-dimensional pixel extent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PixelExtent {
    width: u32,
    height: u32,
}

impl PixelExtent {
    /// Construct a non-empty extent.
    ///
    /// # Errors
    ///
    /// Returns [`CaptureFrameError::EmptyExtent`] when either dimension is zero.
    pub const fn new(width: u32, height: u32) -> Result<Self, CaptureFrameError> {
        if width == 0 || height == 0 {
            return Err(CaptureFrameError::EmptyExtent { width, height });
        }
        Ok(Self { width, height })
    }

    /// Width in pixels.
    #[must_use]
    pub const fn width(self) -> u32 {
        self.width
    }

    /// Height in pixels.
    #[must_use]
    pub const fn height(self) -> u32 {
        self.height
    }

    /// Component-wise maximum used to union independent publication requests.
    #[must_use]
    pub const fn union(self, other: Self) -> Self {
        Self {
            width: if self.width > other.width {
                self.width
            } else {
                other.width
            },
            height: if self.height > other.height {
                self.height
            } else {
                other.height
            },
        }
    }
}

/// Signed origin in the physical desktop coordinate space.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PhysicalOrigin {
    /// Horizontal coordinate. Displays left of the primary output are negative.
    pub x: i32,
    /// Vertical coordinate. Displays above the primary output are negative.
    pub y: i32,
}

/// Rectangle in native scanout pixel coordinates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PixelRect {
    x: u32,
    y: u32,
    extent: PixelExtent,
}

impl PixelRect {
    /// Construct a non-empty pixel rectangle.
    ///
    /// # Errors
    ///
    /// Returns [`CaptureFrameError::EmptyExtent`] for an empty rectangle.
    pub fn new(x: u32, y: u32, width: u32, height: u32) -> Result<Self, CaptureFrameError> {
        let extent = match PixelExtent::new(width, height) {
            Ok(extent) => extent,
            Err(error) => return Err(error),
        };
        Ok(Self { x, y, extent })
    }

    /// Horizontal offset from the scanout origin.
    #[must_use]
    pub const fn x(self) -> u32 {
        self.x
    }

    /// Vertical offset from the scanout origin.
    #[must_use]
    pub const fn y(self) -> u32 {
        self.y
    }

    /// Rectangle extent.
    #[must_use]
    pub const fn extent(self) -> PixelExtent {
        self.extent
    }

    fn fits_within(self, extent: PixelExtent) -> bool {
        self.x
            .checked_add(self.extent.width)
            .is_some_and(|right| right <= extent.width)
            && self
                .y
                .checked_add(self.extent.height)
                .is_some_and(|bottom| bottom <= extent.height)
    }
}

/// Display transform still pending on native scanout pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CaptureRotation {
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

impl CaptureRotation {
    /// Logical extent after applying this transform exactly once.
    #[must_use]
    pub const fn apply_to_extent(self, extent: PixelExtent) -> PixelExtent {
        match self {
            Self::Identity | Self::Clockwise180 => extent,
            Self::Clockwise90 | Self::Clockwise270 => PixelExtent {
                width: extent.height,
                height: extent.width,
            },
        }
    }
}

/// Rational mapping from physical pixels to compositor coordinates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SourceScale {
    numerator: u32,
    denominator: u32,
}

impl SourceScale {
    /// Identity scale.
    pub const ONE: Self = Self {
        numerator: 1,
        denominator: 1,
    };

    /// Construct a positive rational scale.
    ///
    /// # Errors
    ///
    /// Returns [`CaptureFrameError::InvalidSourceScale`] when either term is zero.
    pub const fn new(numerator: u32, denominator: u32) -> Result<Self, CaptureFrameError> {
        if numerator == 0 || denominator == 0 {
            return Err(CaptureFrameError::InvalidSourceScale {
                numerator,
                denominator,
            });
        }
        Ok(Self {
            numerator,
            denominator,
        })
    }

    /// Scale numerator.
    #[must_use]
    pub const fn numerator(self) -> u32 {
        self.numerator
    }

    /// Scale denominator.
    #[must_use]
    pub const fn denominator(self) -> u32 {
        self.denominator
    }
}

/// Physical geometry carried by a capture frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CaptureGeometry {
    origin: PhysicalOrigin,
    native_extent: PixelExtent,
    storage_extent: PixelExtent,
    rotation: CaptureRotation,
    crop: Option<PixelRect>,
    source_scale: SourceScale,
}

impl CaptureGeometry {
    /// Construct validated physical geometry.
    ///
    /// # Errors
    ///
    /// Returns [`CaptureFrameError::CropOutOfBounds`] when the crop escapes the
    /// native scanout extent.
    pub fn new(
        origin: PhysicalOrigin,
        native_extent: PixelExtent,
        storage_extent: PixelExtent,
        rotation: CaptureRotation,
        crop: Option<PixelRect>,
        source_scale: SourceScale,
    ) -> Result<Self, CaptureFrameError> {
        if let Some(crop) = crop
            && !crop.fits_within(native_extent)
        {
            return Err(CaptureFrameError::CropOutOfBounds {
                crop,
                extent: native_extent,
            });
        }
        Ok(Self {
            origin,
            native_extent,
            storage_extent,
            rotation,
            crop,
            source_scale,
        })
    }

    /// Physical desktop origin.
    #[must_use]
    pub const fn origin(&self) -> PhysicalOrigin {
        self.origin
    }

    /// Native scanout extent before rotation, crop, or backend downsampling.
    #[must_use]
    pub const fn native_extent(&self) -> PixelExtent {
        self.native_extent
    }

    /// Extent of the pixel plane retained in frame storage.
    #[must_use]
    pub const fn storage_extent(&self) -> PixelExtent {
        self.storage_extent
    }

    /// Transform still pending on the stored pixels.
    #[must_use]
    pub const fn rotation(&self) -> CaptureRotation {
        self.rotation
    }

    /// Optional crop in native scanout coordinates.
    #[must_use]
    pub const fn crop(&self) -> Option<PixelRect> {
        self.crop
    }

    /// Mapping from physical pixels to compositor coordinates.
    #[must_use]
    pub const fn source_scale(&self) -> SourceScale {
        self.source_scale
    }
}

/// Color primaries and white point of stored pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum CaptureColorSpace {
    /// IEC 61966-2-1 sRGB primaries.
    Srgb,
    /// Display P3 primaries.
    DisplayP3,
    /// ITU-R BT.2020 primaries.
    Rec2020,
    /// Backend did not provide trustworthy color metadata.
    #[default]
    Unknown,
}

/// Transfer function used by stored channel values.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum CaptureTransferFunction {
    /// Standard sRGB electro-optical transfer function.
    Srgb,
    /// Linear light.
    Linear,
    /// ITU-R BT.709 opto-electronic transfer function.
    Rec709,
    /// ITU-R BT.2020 opto-electronic transfer function.
    Rec2020,
    /// SMPTE ST 2084 perceptual quantizer.
    Pq,
    /// Hybrid log-gamma.
    Hlg,
    /// Backend did not provide trustworthy transfer metadata.
    #[default]
    Unknown,
}

/// Canonical positive finite scalar retained by exact color contracts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct CapturePositiveScalar(NonZeroU32);

impl CapturePositiveScalar {
    /// Validate and retain one positive finite scalar without rounding.
    ///
    /// # Errors
    ///
    /// Rejects zero, negative, NaN, and infinite values.
    pub fn try_new(value: f32) -> Result<Self, CaptureColorimetryError> {
        if !value.is_finite() {
            return Err(CaptureColorimetryError::NonFinitePositiveScalar);
        }
        if value <= 0.0 {
            return Err(CaptureColorimetryError::NonPositiveScalar);
        }
        Ok(Self(NonZeroU32::new(value.to_bits()).expect(
            "positive finite floats have non-zero bit patterns",
        )))
    }

    /// Recover the retained scalar.
    #[must_use]
    pub const fn value(self) -> f32 {
        f32::from_bits(self.0.get())
    }
}

/// Absolute luminance context for display-referred color processing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct CaptureLuminanceContext {
    reference_white_nits: CapturePositiveScalar,
    peak_nits: CapturePositiveScalar,
}

impl CaptureLuminanceContext {
    /// Construct an ordered reference-white and peak-luminance contract.
    ///
    /// # Errors
    ///
    /// Rejects a peak luminance below reference white.
    pub fn new(
        reference_white_nits: CapturePositiveScalar,
        peak_nits: CapturePositiveScalar,
    ) -> Result<Self, CaptureColorimetryError> {
        if peak_nits < reference_white_nits {
            return Err(CaptureColorimetryError::PeakBelowReferenceWhite);
        }
        Ok(Self {
            reference_white_nits,
            peak_nits,
        })
    }

    /// Reference white in candelas per square metre.
    #[must_use]
    pub const fn reference_white_nits(self) -> CapturePositiveScalar {
        self.reference_white_nits
    }

    /// Peak luminance in candelas per square metre.
    #[must_use]
    pub const fn peak_nits(self) -> CapturePositiveScalar {
        self.peak_nits
    }
}

/// Declared dynamic range of one encoded surface.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum CaptureDynamicRange {
    /// Standard-dynamic-range content.
    Standard,
    /// High-dynamic-range content.
    High,
}

/// Backend color metadata, including explicitly missing fields.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct CaptureColorimetry {
    color_space: CaptureColorSpace,
    transfer_function: CaptureTransferFunction,
    dynamic_range: Option<CaptureDynamicRange>,
    luminance: Option<CaptureLuminanceContext>,
}

impl CaptureColorimetry {
    /// Canonical fully-known SDR sRGB metadata.
    pub const SRGB: Self = Self::from_known(KnownCaptureColorimetry::SRGB);

    /// Construct backend metadata without manufacturing missing knowledge.
    ///
    /// # Errors
    ///
    /// Rejects known transfer functions paired with a contradictory dynamic
    /// range. Missing fields remain admissible until publication resolution.
    pub fn new(
        color_space: CaptureColorSpace,
        transfer_function: CaptureTransferFunction,
        dynamic_range: Option<CaptureDynamicRange>,
        luminance: Option<CaptureLuminanceContext>,
    ) -> Result<Self, CaptureColorimetryError> {
        validate_transfer_range(transfer_function, dynamic_range)?;
        Ok(Self {
            color_space,
            transfer_function,
            dynamic_range,
            luminance,
        })
    }

    /// Metadata supplied without trustworthy color information.
    #[must_use]
    pub const fn unknown() -> Self {
        Self {
            color_space: CaptureColorSpace::Unknown,
            transfer_function: CaptureTransferFunction::Unknown,
            dynamic_range: None,
            luminance: None,
        }
    }

    /// Convert a fully-known color contract to backend metadata.
    #[must_use]
    pub const fn from_known(known: KnownCaptureColorimetry) -> Self {
        Self {
            color_space: known.color_space,
            transfer_function: known.transfer_function,
            dynamic_range: Some(known.dynamic_range),
            luminance: known.luminance,
        }
    }

    /// Declared primaries and white point.
    #[must_use]
    pub const fn color_space(self) -> CaptureColorSpace {
        self.color_space
    }

    /// Declared channel transfer function.
    #[must_use]
    pub const fn transfer_function(self) -> CaptureTransferFunction {
        self.transfer_function
    }

    /// Declared dynamic range, when known.
    #[must_use]
    pub const fn dynamic_range(self) -> Option<CaptureDynamicRange> {
        self.dynamic_range
    }

    /// Absolute luminance context, when supplied.
    #[must_use]
    pub const fn luminance(self) -> Option<CaptureLuminanceContext> {
        self.luminance
    }

    /// Require every field needed for color-managed processing.
    ///
    /// # Errors
    ///
    /// Rejects unknown primaries, transfer, or dynamic range, and HDR metadata
    /// without an absolute luminance context.
    pub fn try_known(self) -> Result<KnownCaptureColorimetry, CaptureColorimetryError> {
        KnownCaptureColorimetry::try_new(
            self.color_space,
            self.transfer_function,
            self.dynamic_range
                .ok_or(CaptureColorimetryError::UnknownDynamicRange)?,
            self.luminance,
        )
    }
}

impl Default for CaptureColorimetry {
    fn default() -> Self {
        Self::unknown()
    }
}

/// Fully-known source or target colorimetry.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct KnownCaptureColorimetry {
    color_space: CaptureColorSpace,
    transfer_function: CaptureTransferFunction,
    dynamic_range: CaptureDynamicRange,
    luminance: Option<CaptureLuminanceContext>,
}

impl KnownCaptureColorimetry {
    /// Canonical SDR sRGB target without an implied absolute display level.
    pub const SRGB: Self = Self {
        color_space: CaptureColorSpace::Srgb,
        transfer_function: CaptureTransferFunction::Srgb,
        dynamic_range: CaptureDynamicRange::Standard,
        luminance: None,
    };

    /// Construct a complete color-managed processing contract.
    ///
    /// # Errors
    ///
    /// Rejects unknown fields, contradictory transfer/range pairs, and HDR
    /// contracts without absolute luminance context.
    pub fn try_new(
        color_space: CaptureColorSpace,
        transfer_function: CaptureTransferFunction,
        dynamic_range: CaptureDynamicRange,
        luminance: Option<CaptureLuminanceContext>,
    ) -> Result<Self, CaptureColorimetryError> {
        if color_space == CaptureColorSpace::Unknown {
            return Err(CaptureColorimetryError::UnknownColorSpace);
        }
        if transfer_function == CaptureTransferFunction::Unknown {
            return Err(CaptureColorimetryError::UnknownTransferFunction);
        }
        validate_transfer_range(transfer_function, Some(dynamic_range))?;
        if dynamic_range == CaptureDynamicRange::High && luminance.is_none() {
            return Err(CaptureColorimetryError::MissingHdrLuminance);
        }
        Ok(Self {
            color_space,
            transfer_function,
            dynamic_range,
            luminance,
        })
    }

    /// Declared primaries and white point.
    #[must_use]
    pub const fn color_space(self) -> CaptureColorSpace {
        self.color_space
    }

    /// Declared channel transfer function.
    #[must_use]
    pub const fn transfer_function(self) -> CaptureTransferFunction {
        self.transfer_function
    }

    /// Declared dynamic range.
    #[must_use]
    pub const fn dynamic_range(self) -> CaptureDynamicRange {
        self.dynamic_range
    }

    /// Absolute luminance context, when relevant.
    #[must_use]
    pub const fn luminance(self) -> Option<CaptureLuminanceContext> {
        self.luminance
    }

    /// Replace the absolute luminance context while retaining encoding.
    #[must_use]
    pub const fn with_luminance(self, luminance: CaptureLuminanceContext) -> Self {
        Self {
            luminance: Some(luminance),
            ..self
        }
    }
}

fn validate_transfer_range(
    transfer_function: CaptureTransferFunction,
    dynamic_range: Option<CaptureDynamicRange>,
) -> Result<(), CaptureColorimetryError> {
    let contradictory = matches!(
        (transfer_function, dynamic_range),
        (
            CaptureTransferFunction::Srgb
                | CaptureTransferFunction::Rec709
                | CaptureTransferFunction::Rec2020,
            Some(CaptureDynamicRange::High)
        ) | (
            CaptureTransferFunction::Pq | CaptureTransferFunction::Hlg,
            Some(CaptureDynamicRange::Standard)
        )
    );
    if contradictory {
        return Err(CaptureColorimetryError::TransferDynamicRangeMismatch);
    }
    Ok(())
}

/// Invalid color metadata rejected before publication work begins.
#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
pub enum CaptureColorimetryError {
    /// Positive scalars cannot be NaN or infinite.
    #[error("capture color scalar must be finite")]
    NonFinitePositiveScalar,
    /// Positive scalars cannot be zero or negative.
    #[error("capture color scalar must be greater than zero")]
    NonPositiveScalar,
    /// Peak luminance cannot be below reference white.
    #[error("capture peak luminance must be at least reference white")]
    PeakBelowReferenceWhite,
    /// Managed color processing requires known primaries.
    #[error("capture color space is unknown")]
    UnknownColorSpace,
    /// Managed color processing requires a known transfer function.
    #[error("capture transfer function is unknown")]
    UnknownTransferFunction,
    /// Managed color processing requires an explicit dynamic range.
    #[error("capture dynamic range is unknown")]
    UnknownDynamicRange,
    /// The transfer function contradicts the declared dynamic range.
    #[error("capture transfer function contradicts its dynamic range")]
    TransferDynamicRangeMismatch,
    /// HDR processing requires absolute luminance context.
    #[error("HDR capture colorimetry requires luminance context")]
    MissingHdrLuminance,
}

/// Native encoding of one separately-owned cursor shape.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaptureCursorShapeFormat {
    /// BGRA color pixels with backend-defined alpha semantics.
    ColorBgra8,
    /// One bit-per-pixel AND plane followed by one XOR plane.
    MonochromeAndXor,
    /// BGRA pixels whose alpha selects copy or desktop-XOR behavior.
    MaskedColorBgra8,
}

/// Immutable cursor pixels with an exact generation and memory layout.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CaptureCursorShape {
    generation: NonZeroU64,
    extent: PixelExtent,
    format: CaptureCursorShapeFormat,
    row_stride: usize,
    bytes: Arc<[u8]>,
}

impl CaptureCursorShape {
    /// Validate and retain one backend-provided cursor shape without copying.
    ///
    /// Monochrome storage contains `height` AND rows followed by `height` XOR
    /// rows. Color formats contain exactly `height` rows. Row padding is
    /// retained, but trailing bytes are rejected so every shape has one exact
    /// allocation contract.
    ///
    /// # Errors
    ///
    /// Rejects zero generations, undersized strides, and any allocation whose
    /// length does not exactly match its declared layout.
    pub fn new(
        generation: u64,
        extent: PixelExtent,
        format: CaptureCursorShapeFormat,
        row_stride: usize,
        bytes: Arc<[u8]>,
    ) -> Result<Self, CaptureFrameError> {
        let generation =
            NonZeroU64::new(generation).ok_or(CaptureFrameError::ZeroCursorShapeGeneration)?;
        let width = usize::try_from(extent.width)
            .map_err(|_| CaptureFrameError::CursorShapeSizeOverflow)?;
        let height = usize::try_from(extent.height)
            .map_err(|_| CaptureFrameError::CursorShapeSizeOverflow)?;
        let (minimum_stride, rows) = match format {
            CaptureCursorShapeFormat::ColorBgra8 | CaptureCursorShapeFormat::MaskedColorBgra8 => (
                width
                    .checked_mul(4)
                    .ok_or(CaptureFrameError::CursorShapeSizeOverflow)?,
                height,
            ),
            CaptureCursorShapeFormat::MonochromeAndXor => (
                width
                    .checked_add(7)
                    .ok_or(CaptureFrameError::CursorShapeSizeOverflow)?
                    / 8,
                height
                    .checked_mul(2)
                    .ok_or(CaptureFrameError::CursorShapeSizeOverflow)?,
            ),
        };
        if row_stride < minimum_stride {
            return Err(CaptureFrameError::InvalidCursorShapeStride {
                stride: row_stride,
                minimum: minimum_stride,
            });
        }
        let expected = row_stride
            .checked_mul(rows)
            .ok_or(CaptureFrameError::CursorShapeSizeOverflow)?;
        if bytes.len() != expected {
            return Err(CaptureFrameError::CursorShapeLengthMismatch {
                actual: bytes.len(),
                expected,
            });
        }
        Ok(Self {
            generation,
            extent,
            format,
            row_stride,
            bytes,
        })
    }

    /// Stable shape generation within the capture source session.
    #[must_use]
    pub const fn generation(&self) -> NonZeroU64 {
        self.generation
    }

    /// Visible cursor bounds represented by the shape.
    #[must_use]
    pub const fn extent(&self) -> PixelExtent {
        self.extent
    }

    /// Native shape encoding.
    #[must_use]
    pub const fn format(&self) -> CaptureCursorShapeFormat {
        self.format
    }

    /// Byte distance between rows within each plane.
    #[must_use]
    pub const fn row_stride(&self) -> usize {
        self.row_stride
    }

    /// Exact shape bytes, including declared row padding.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}

/// Where cursor pixels for one capture frame reside.
///
/// This records source storage semantics independently of [`CaptureCursor::visible`].
/// A cursor can retain separate or composed source content while falling outside
/// a processed crop.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum CaptureCursorContent {
    /// The backend supplied no cursor pixels for this frame.
    #[default]
    Absent,
    /// The backend explicitly reported that the cursor is hidden.
    Hidden,
    /// Cursor pixels are owned separately from the captured surface.
    Separate(Arc<CaptureCursorShape>),
    /// Cursor pixels are already composed into the captured surface.
    Composed,
}

/// Cursor metadata in native scanout coordinates.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CaptureCursor {
    /// Whether the cursor is visible in this frame.
    pub visible: bool,
    /// Cursor origin relative to the native scanout surface.
    pub position: Option<PhysicalOrigin>,
    /// Cursor hotspot relative to its shape origin.
    pub hotspot: Option<PhysicalOrigin>,
    /// Cursor shape extent when supplied separately from the pixels.
    pub shape_extent: Option<PixelExtent>,
    /// Backend-specific shape generation, without exposing its native handle.
    pub shape_generation: Option<u64>,
    /// Ownership and composition state of the cursor pixels.
    pub content: CaptureCursorContent,
}

/// Pixel encoding of a capture storage plane.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CapturePixelFormat {
    /// Red, green, blue, alpha bytes.
    Rgba8,
    /// Blue, green, red, alpha bytes.
    Bgra8,
    /// Little-endian A2R10G10B10 packed pixels (`l10r`).
    Argb2101010,
    /// Little-endian RGBA binary16 components (`RGhA`).
    Rgba16Float,
    /// Bi-planar 8-bit 4:2:0 video-range YUV (`420v`).
    Yuv420VideoRange,
    /// Bi-planar 8-bit 4:2:0 full-range YUV (`420f`).
    Yuv420FullRange,
    /// Bi-planar MSB-aligned 10-bit 4:4:4 YUV (`xf44`).
    Yuv44410BiPlanar,
}

impl CapturePixelFormat {
    pub(crate) const fn rgba8_bytes_per_pixel(self) -> Option<usize> {
        match self {
            Self::Rgba8 | Self::Bgra8 => Some(4),
            Self::Argb2101010
            | Self::Rgba16Float
            | Self::Yuv420VideoRange
            | Self::Yuv420FullRange
            | Self::Yuv44410BiPlanar => None,
        }
    }
}

trait CaptureBytePlane: Send + Sync {
    fn bytes(&self) -> &[u8];
    fn owner_identity(&self) -> *const ();
}

impl<T> CaptureBytePlane for T
where
    T: AsRef<[u8]> + Send + Sync,
{
    fn bytes(&self) -> &[u8] {
        self.as_ref()
    }

    fn owner_identity(&self) -> *const () {
        std::ptr::from_ref(self).cast()
    }
}

/// Owned CPU pixel plane with an explicit signed row stride.
#[derive(Clone)]
pub struct CpuCaptureStorage {
    plane: Arc<dyn CaptureBytePlane>,
    format: CapturePixelFormat,
    row_stride: i64,
    row0_offset: usize,
}

impl CpuCaptureStorage {
    /// Construct CPU storage. Bounds are validated against frame geometry by
    /// [`CaptureFrame::new`].
    #[must_use]
    pub fn new(
        bytes: Arc<[u8]>,
        format: CapturePixelFormat,
        row_stride: i64,
        row0_offset: usize,
    ) -> Self {
        Self::from_owner(bytes, format, row_stride, row0_offset)
    }

    /// Retain an ownership-transferable byte plane without copying its pixels.
    #[must_use]
    pub fn from_owner<T>(
        owner: T,
        format: CapturePixelFormat,
        row_stride: i64,
        row0_offset: usize,
    ) -> Self
    where
        T: AsRef<[u8]> + Send + Sync + 'static,
    {
        Self {
            plane: Arc::new(owner),
            format,
            row_stride,
            row0_offset,
        }
    }

    /// Retain an already shared owner without allocating an outer `Arc`.
    #[must_use]
    pub fn from_shared_owner<T>(
        owner: Arc<T>,
        format: CapturePixelFormat,
        row_stride: i64,
        row0_offset: usize,
    ) -> Self
    where
        T: AsRef<[u8]> + Send + Sync + 'static,
    {
        Self {
            plane: owner,
            format,
            row_stride,
            row0_offset,
        }
    }

    /// Shared pixel bytes.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        self.plane.bytes()
    }

    /// Pixel encoding.
    #[must_use]
    pub const fn format(&self) -> CapturePixelFormat {
        self.format
    }

    /// Signed byte distance from one row start to the next.
    #[must_use]
    pub const fn row_stride(&self) -> i64 {
        self.row_stride
    }

    /// Byte offset of the first logical row.
    #[must_use]
    pub const fn row0_offset(&self) -> usize {
        self.row0_offset
    }

    /// Identity of the retained owner allocation.
    #[must_use]
    pub fn owner_identity(&self) -> *const () {
        self.plane.owner_identity()
    }

    pub(crate) fn tightly_packed_rgba8(&self, extent: PixelExtent) -> Option<&[u8]> {
        let bytes_per_pixel = self.format.rgba8_bytes_per_pixel()?;
        let row_bytes = usize::try_from(extent.width)
            .ok()?
            .checked_mul(bytes_per_pixel)?;
        let expected = row_bytes.checked_mul(usize::try_from(extent.height).ok()?)?;
        if self.format != CapturePixelFormat::Rgba8
            || self.row_stride != i64::try_from(row_bytes).ok()?
            || self.row0_offset != 0
            || self.bytes().len() < expected
        {
            return None;
        }
        self.bytes().get(..expected)
    }

    fn validate(&self, extent: PixelExtent) -> Result<(), CaptureFrameError> {
        let bytes_per_pixel = self
            .format
            .rgba8_bytes_per_pixel()
            .ok_or(CaptureFrameError::UnsupportedCpuStorageFormat(self.format))?;
        let row_bytes = usize::try_from(extent.width)
            .ok()
            .and_then(|width| width.checked_mul(bytes_per_pixel))
            .ok_or(CaptureFrameError::StorageSizeOverflow)?;
        let row_bytes_i64 =
            i64::try_from(row_bytes).map_err(|_| CaptureFrameError::StorageSizeOverflow)?;
        let stride_magnitude =
            self.row_stride
                .checked_abs()
                .ok_or(CaptureFrameError::InvalidCpuStride {
                    stride: self.row_stride,
                    minimum: row_bytes,
                })?;
        if stride_magnitude < row_bytes_i64 {
            return Err(CaptureFrameError::InvalidCpuStride {
                stride: self.row_stride,
                minimum: row_bytes,
            });
        }

        let row_count = i128::from(extent.height.saturating_sub(1));
        let first =
            i128::try_from(self.row0_offset).map_err(|_| CaptureFrameError::StorageSizeOverflow)?;
        let last = first
            .checked_add(i128::from(self.row_stride).saturating_mul(row_count))
            .ok_or(CaptureFrameError::StorageSizeOverflow)?;
        let lowest = first.min(last);
        let highest = first.max(last);
        let end = highest
            .checked_add(
                i128::try_from(row_bytes).map_err(|_| CaptureFrameError::StorageSizeOverflow)?,
            )
            .ok_or(CaptureFrameError::StorageSizeOverflow)?;
        let buffer_len = i128::try_from(self.bytes().len())
            .map_err(|_| CaptureFrameError::StorageSizeOverflow)?;
        if lowest < 0 || end > buffer_len {
            return Err(CaptureFrameError::CpuBufferOutOfBounds {
                buffer_len: self.bytes().len(),
                row0_offset: self.row0_offset,
                stride: self.row_stride,
                extent,
            });
        }
        Ok(())
    }
}

impl fmt::Debug for CpuCaptureStorage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CpuCaptureStorage")
            .field("len", &self.bytes().len())
            .field("format", &self.format)
            .field("row_stride", &self.row_stride)
            .field("row0_offset", &self.row0_offset)
            .finish_non_exhaustive()
    }
}

#[derive(Debug)]
struct CapturePlanePoolInner {
    available: Mutex<Vec<CapturePlaneBacking>>,
    allocations: AtomicUsize,
    admission_coordinator: Option<ScreenByteAdmissionCoordinator>,
}

#[derive(Debug)]
struct CapturePlaneBacking {
    buffer: Vec<u8>,
    _admission: Option<ScreenByteLease>,
}

/// Reusable owner pool for capture adapters that must materialize CPU pixels.
#[derive(Clone, Debug)]
pub struct CapturePlanePool {
    inner: Arc<CapturePlanePoolInner>,
}

impl Default for CapturePlanePool {
    fn default() -> Self {
        Self {
            inner: Arc::new(CapturePlanePoolInner {
                available: Mutex::new(Vec::new()),
                allocations: AtomicUsize::new(0),
                admission_coordinator: None,
            }),
        }
    }
}

impl CapturePlanePool {
    /// Create a pool whose backing planes share a process-wide byte fence.
    #[must_use]
    pub fn with_admission_coordinator(
        admission_coordinator: ScreenByteAdmissionCoordinator,
    ) -> Self {
        Self {
            inner: Arc::new(CapturePlanePoolInner {
                available: Mutex::new(Vec::new()),
                allocations: AtomicUsize::new(0),
                admission_coordinator: Some(admission_coordinator),
            }),
        }
    }

    /// Acquire exclusive mutable storage with at least `minimum_capacity` bytes.
    ///
    /// # Errors
    ///
    /// Returns a typed allocation failure without discarding a reusable plane.
    pub fn try_acquire(
        &self,
        minimum_capacity: usize,
    ) -> Result<CapturePlaneLease, CaptureFrameError> {
        let mut available = self
            .inner
            .available
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let index = available
            .iter()
            .position(|backing| backing.buffer.capacity() >= minimum_capacity);
        let (backing, created) = if let Some(index) = index {
            (available.swap_remove(index), false)
        } else if let Some(admission_coordinator) = &self.inner.admission_coordinator {
            let requested_bytes = u64::try_from(minimum_capacity)
                .map_err(|_| CaptureFrameError::StorageSizeOverflow)?;
            let reservation = admission_coordinator.try_acquire(requested_bytes).map_err(
                |error| match error {
                    ScreenByteAdmissionError::CapacityExceeded {
                        requested_bytes,
                        available_bytes,
                    } => CaptureFrameError::PlaneCapacityExceeded {
                        requested_bytes,
                        available_bytes,
                    },
                    ScreenByteAdmissionError::CapacityShrinkRejected { .. }
                    | ScreenByteAdmissionError::RevisionExhausted => {
                        CaptureFrameError::PlaneAllocationFailed {
                            byte_len: minimum_capacity,
                        }
                    }
                },
            )?;
            let mut buffer = Vec::new();
            if buffer.try_reserve_exact(minimum_capacity).is_err() {
                return Err(CaptureFrameError::PlaneAllocationFailed {
                    byte_len: minimum_capacity,
                });
            }
            available.retain(|backing| backing.buffer.capacity() >= minimum_capacity);
            (
                CapturePlaneBacking {
                    buffer,
                    _admission: Some(ScreenByteReservation::freeze(reservation)),
                },
                true,
            )
        } else {
            let (mut backing, created) = available.pop().map_or_else(
                || {
                    (
                        CapturePlaneBacking {
                            buffer: Vec::new(),
                            _admission: None,
                        },
                        true,
                    )
                },
                |backing| (backing, false),
            );
            if backing.buffer.capacity() < minimum_capacity
                && backing.buffer.try_reserve_exact(minimum_capacity).is_err()
            {
                available.push(backing);
                return Err(CaptureFrameError::PlaneAllocationFailed {
                    byte_len: minimum_capacity,
                });
            }
            (backing, created)
        };
        if created {
            self.inner.allocations.fetch_add(1, Ordering::Relaxed);
        }
        drop(available);
        Ok(CapturePlaneLease {
            backing: Some(backing),
            pool: Arc::downgrade(&self.inner),
        })
    }

    /// Number of backing allocations created by this pool.
    #[must_use]
    pub fn allocation_count(&self) -> usize {
        self.inner.allocations.load(Ordering::Relaxed)
    }

    /// Number of buffers currently available for immediate reuse.
    #[must_use]
    pub fn available_count(&self) -> usize {
        self.inner
            .available
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len()
    }
}

/// Exclusive mutable lease that freezes into an immutable pooled frame plane.
pub struct CapturePlaneLease {
    backing: Option<CapturePlaneBacking>,
    pool: Weak<CapturePlanePoolInner>,
}

impl CapturePlaneLease {
    /// Freeze the populated buffer for publication.
    #[must_use]
    pub fn freeze(mut self) -> PooledCapturePlane {
        PooledCapturePlane {
            backing: self.backing.take(),
            pool: self.pool.clone(),
        }
    }
}

impl Deref for CapturePlaneLease {
    type Target = Vec<u8>;

    fn deref(&self) -> &Self::Target {
        &self
            .backing
            .as_ref()
            .expect("capture plane lease owns a buffer")
            .buffer
    }
}

impl DerefMut for CapturePlaneLease {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self
            .backing
            .as_mut()
            .expect("capture plane lease owns a buffer")
            .buffer
    }
}

impl Drop for CapturePlaneLease {
    fn drop(&mut self) {
        recycle_plane(self.backing.take(), &self.pool);
    }
}

/// Immutable CPU plane whose allocation returns to its pool after publication.
pub struct PooledCapturePlane {
    backing: Option<CapturePlaneBacking>,
    pool: Weak<CapturePlanePoolInner>,
}

impl AsRef<[u8]> for PooledCapturePlane {
    fn as_ref(&self) -> &[u8] {
        self.backing
            .as_ref()
            .map(|backing| backing.buffer.as_slice())
            .expect("pooled capture plane owns a buffer")
    }
}

impl Drop for PooledCapturePlane {
    fn drop(&mut self) {
        recycle_plane(self.backing.take(), &self.pool);
    }
}

fn recycle_plane(backing: Option<CapturePlaneBacking>, pool: &Weak<CapturePlanePoolInner>) {
    let (Some(mut backing), Some(pool)) = (backing, pool.upgrade()) else {
        return;
    };
    backing.buffer.clear();
    pool.available
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .push(backing);
}

/// Platform family of an opaque GPU surface.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlatformGpuApi {
    /// Direct3D texture shared by a Windows adapter.
    Direct3d11,
    /// Linux DMA-BUF surface.
    DmaBuf,
    /// Vulkan image or exported memory.
    Vulkan,
    /// Metal texture.
    Metal,
    /// Extensible backend name without a native API type.
    Other(Arc<str>),
}

/// Opaque, lifetime-owning GPU surface descriptor.
#[derive(Clone)]
pub struct PlatformGpuSurface {
    api: PlatformGpuApi,
    handle_id: u64,
    extent: PixelExtent,
    format: CapturePixelFormat,
    owner: Arc<dyn Any + Send + Sync>,
    retained_owner: Option<Arc<dyn Any + Send + Sync>>,
    target_resource_lifetime: Option<ScreenResourceLifetime>,
    capture_resource_lifetime: Option<ScreenResourceLifetime>,
}

/// Typed access to one GPU owner paired with every attached resource lifetime.
#[derive(Clone)]
pub struct PlatformGpuSurfaceOwner<T> {
    owner: Arc<T>,
    _target_resource_lifetime: Option<ScreenResourceLifetime>,
    _capture_resource_lifetime: Option<ScreenResourceLifetime>,
}

impl<T> PlatformGpuSurfaceOwner<T> {
    fn new(
        owner: Arc<T>,
        target_resource_lifetime: Option<ScreenResourceLifetime>,
        capture_resource_lifetime: Option<ScreenResourceLifetime>,
    ) -> Self {
        Self {
            owner,
            _target_resource_lifetime: target_resource_lifetime,
            _capture_resource_lifetime: capture_resource_lifetime,
        }
    }

    /// Observe owner retirement without detaching it from admission.
    #[must_use]
    pub fn downgrade(&self) -> Weak<T> {
        Arc::downgrade(&self.owner)
    }
}

impl<T> Deref for PlatformGpuSurfaceOwner<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.owner
    }
}

impl PlatformGpuSurface {
    /// Erase a platform owner behind the neutral capture contract.
    ///
    /// # Errors
    ///
    /// Returns [`CaptureFrameError::InvalidGpuHandle`] for a zero handle id.
    pub fn new<T>(
        api: PlatformGpuApi,
        handle_id: u64,
        extent: PixelExtent,
        format: CapturePixelFormat,
        owner: Arc<T>,
    ) -> Result<Self, CaptureFrameError>
    where
        T: Any + Send + Sync,
    {
        if handle_id == 0 {
            return Err(CaptureFrameError::InvalidGpuHandle);
        }
        let owner: Arc<dyn Any + Send + Sync> = owner;
        Ok(Self {
            api,
            handle_id,
            extent,
            format,
            owner,
            retained_owner: None,
            target_resource_lifetime: None,
            capture_resource_lifetime: None,
        })
    }

    pub(crate) fn with_native_target_owners(
        mut self,
        retained_owner: Arc<dyn Any + Send + Sync>,
        target_resource_lifetime: ScreenResourceLifetime,
        capture_resource_lifetime: Option<ScreenResourceLifetime>,
    ) -> Self {
        self.retained_owner = Some(retained_owner);
        self.target_resource_lifetime = Some(target_resource_lifetime);
        self.capture_resource_lifetime = capture_resource_lifetime;
        self
    }

    /// GPU API family without exposing a native handle type.
    #[must_use]
    pub const fn api(&self) -> &PlatformGpuApi {
        &self.api
    }

    /// Adapter-provided stable handle identity.
    #[must_use]
    pub const fn handle_id(&self) -> u64 {
        self.handle_id
    }

    /// Storage extent.
    #[must_use]
    pub const fn extent(&self) -> PixelExtent {
        self.extent
    }

    /// Pixel encoding.
    #[must_use]
    pub const fn format(&self) -> CapturePixelFormat {
        self.format
    }

    /// Strong references retaining the native surface lifetime.
    #[must_use]
    pub fn owner_strong_count(&self) -> usize {
        Arc::strong_count(&self.owner)
    }

    /// Recover a typed owner in a platform adapter without exposing that type
    /// in the backend-neutral contract.
    #[must_use]
    pub fn owner<T>(&self) -> Option<PlatformGpuSurfaceOwner<T>>
    where
        T: Any + Send + Sync,
    {
        Arc::clone(&self.owner).downcast().ok().map(|owner| {
            PlatformGpuSurfaceOwner::new(
                owner,
                self.target_resource_lifetime.clone(),
                self.capture_resource_lifetime.clone(),
            )
        })
    }

    /// Recover a typed secondary owner retained for renderer-target lifetime.
    #[must_use]
    pub fn retained_owner<T>(&self) -> Option<PlatformGpuSurfaceOwner<T>>
    where
        T: Any + Send + Sync,
    {
        self.retained_owner
            .as_ref()
            .and_then(|owner| Arc::clone(owner).downcast().ok())
            .map(|owner| {
                PlatformGpuSurfaceOwner::new(
                    owner,
                    self.target_resource_lifetime.clone(),
                    self.capture_resource_lifetime.clone(),
                )
            })
    }

    /// Exact renderer-target allocation lifetime retained with this GPU surface.
    #[must_use]
    pub const fn resource_lifetime(&self) -> Option<&ScreenResourceLifetime> {
        self.target_resource_lifetime.as_ref()
    }

    /// Exact capture-plan allocation lifetime retained with this GPU surface.
    #[must_use]
    pub const fn capture_resource_lifetime(&self) -> Option<&ScreenResourceLifetime> {
        self.capture_resource_lifetime.as_ref()
    }
}

impl fmt::Debug for PlatformGpuSurface {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PlatformGpuSurface")
            .field("api", &self.api)
            .field("handle_id", &self.handle_id)
            .field("extent", &self.extent)
            .field("format", &self.format)
            .finish_non_exhaustive()
    }
}

/// Storage backing a capture frame.
#[derive(Clone, Debug)]
pub enum CaptureStorage {
    /// Owned CPU pixel plane.
    Cpu(CpuCaptureStorage),
    /// Opaque GPU surface with an erased lifetime owner.
    Gpu(PlatformGpuSurface),
}

/// Native scanout move operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MoveRegion {
    /// Source rectangle before the move.
    pub source: PixelRect,
    /// Destination origin after the move.
    pub destination: (u32, u32),
}

/// Optional damage metadata for incremental consumers.
#[derive(Clone, Debug, Default)]
pub struct CaptureDamage {
    dirty_regions: Arc<[PixelRect]>,
    move_regions: Arc<[MoveRegion]>,
}

impl CaptureDamage {
    /// Construct damage metadata. Bounds are checked by [`CaptureFrame::new`].
    #[must_use]
    pub fn new(dirty_regions: Vec<PixelRect>, move_regions: Vec<MoveRegion>) -> Self {
        Self {
            dirty_regions: dirty_regions.into(),
            move_regions: move_regions.into(),
        }
    }

    /// Dirty rectangles in native scanout coordinates.
    #[must_use]
    pub fn dirty_regions(&self) -> &[PixelRect] {
        &self.dirty_regions
    }

    /// Move operations in native scanout coordinates.
    #[must_use]
    pub fn move_regions(&self) -> &[MoveRegion] {
        &self.move_regions
    }
}

/// Runtime stage discriminator for diagnostics and serialization adapters.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaptureStageKind {
    /// Native scanout pixels with rotation still pending.
    Raw,
    /// Canonical pixels after crop, rotation, and channel-layout normalization.
    GeometryNormalized,
}

mod stage_sealed {
    pub trait Sealed {}
}

/// Marker implemented by the two legal capture surface stages.
pub trait CaptureSurfaceStage:
    stage_sealed::Sealed + Clone + fmt::Debug + Send + Sync + 'static
{
    /// Runtime diagnostic marker matching the type-level stage.
    const KIND: CaptureStageKind;
}

/// Native scanout surface. Its rotation is pending and must not be normalized by adapters.
#[derive(Clone, Copy, Debug)]
pub struct RawCaptureSurface;

impl stage_sealed::Sealed for RawCaptureSurface {}

impl CaptureSurfaceStage for RawCaptureSurface {
    const KIND: CaptureStageKind = CaptureStageKind::Raw;
}

/// Geometry-normalized surface before screen analysis policy is applied.
#[derive(Clone, Copy, Debug)]
pub struct GeometryNormalizedCaptureSurface;

impl stage_sealed::Sealed for GeometryNormalizedCaptureSurface {}

impl CaptureSurfaceStage for GeometryNormalizedCaptureSurface {
    const KIND: CaptureStageKind = CaptureStageKind::GeometryNormalized;
}

/// Metadata shared by CPU and GPU capture frames.
#[derive(Clone, Debug)]
pub struct CaptureFrameMetadata {
    /// Stable logical source identity.
    pub source_id: CaptureSourceId,
    /// Generation of the physical source topology.
    pub topology_generation: u64,
    /// Generation of the active capture session.
    pub session_generation: u64,
    /// Monotonic frame sequence within the session.
    pub sequence: u64,
    /// Monotonic acquisition timestamp.
    pub captured_at: Instant,
    /// Deadline after which this frame must not be presented as fresh.
    pub fresh_until: Instant,
    /// Physical scanout geometry and pending transform.
    pub geometry: CaptureGeometry,
    /// Source color metadata, including explicitly missing fields.
    pub colorimetry: CaptureColorimetry,
    /// Cursor metadata and composition state.
    pub cursor: CaptureCursor,
}

/// Source and generation tuple used to reject stale worker publications.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CaptureEpoch {
    /// Stable source expected by the adapter.
    pub source_id: CaptureSourceId,
    /// Current topology generation.
    pub topology_generation: u64,
    /// Current capture session generation.
    pub session_generation: u64,
}

/// Validated backend-neutral capture envelope.
#[derive(Clone, Debug)]
pub struct CaptureFrame<S: CaptureSurfaceStage> {
    metadata: CaptureFrameMetadata,
    storage: CaptureStorage,
    damage: CaptureDamage,
    stage: PhantomData<S>,
}

impl CaptureFrame<RawCaptureSurface> {
    /// Validate and construct a raw capture frame.
    ///
    /// # Errors
    ///
    /// Rejects zero generations or sequence, inverted freshness, inconsistent
    /// storage, and out-of-bounds damage.
    pub fn new(
        metadata: CaptureFrameMetadata,
        storage: CaptureStorage,
        damage: CaptureDamage,
    ) -> Result<Self, CaptureFrameError> {
        Self::from_parts(metadata, storage, damage)
    }

    /// Consume a raw frame and publish geometry-normalized pixels.
    ///
    /// The source identity, epochs, sequence, timestamps, and color metadata are
    /// retained from the raw input. Callers supply only the processed geometry,
    /// storage, and damage map, so a processed frame cannot be fabricated without
    /// consuming the raw frame it came from.
    ///
    /// # Errors
    ///
    /// Rejects processed geometry with a pending crop or rotation and validates
    /// the replacement storage against its stored extent.
    pub fn into_geometry_normalized(
        self,
        geometry: CaptureGeometry,
        storage: CaptureStorage,
        damage: CaptureDamage,
    ) -> Result<CaptureFrame<GeometryNormalizedCaptureSurface>, CaptureFrameError> {
        let cursor = self.metadata.cursor.clone();
        self.into_geometry_normalized_with_cursor(geometry, storage, damage, cursor)
    }

    /// Consume a raw frame and publish canonical geometry and cursor metadata.
    ///
    /// This is the geometry processor's transition seam when crop or rotation
    /// changes cursor coordinates alongside the stored pixels.
    ///
    /// # Errors
    ///
    /// Applies the same normalized-stage validation as [`Self::into_geometry_normalized`].
    pub fn into_geometry_normalized_with_cursor(
        self,
        geometry: CaptureGeometry,
        storage: CaptureStorage,
        damage: CaptureDamage,
        cursor: CaptureCursor,
    ) -> Result<CaptureFrame<GeometryNormalizedCaptureSurface>, CaptureFrameError> {
        let mut metadata = self.metadata;
        metadata.geometry = geometry;
        metadata.cursor = cursor;
        CaptureFrame::from_parts(metadata, storage, damage)
    }

    /// Consume a raw frame and stamp all byte-changing output metadata.
    ///
    /// Source identity, epochs, sequence, and acquisition timestamps remain
    /// unchanged. Geometry, color space, transfer function, and cursor describe
    /// the replacement storage produced at this transition.
    ///
    /// # Errors
    ///
    /// Applies normalized-stage validation to the complete output contract.
    pub fn into_geometry_normalized_with_output_metadata(
        self,
        geometry: CaptureGeometry,
        storage: CaptureStorage,
        damage: CaptureDamage,
        colorimetry: CaptureColorimetry,
        cursor: CaptureCursor,
    ) -> Result<CaptureFrame<GeometryNormalizedCaptureSurface>, CaptureFrameError> {
        let mut metadata = self.metadata;
        metadata.geometry = geometry;
        metadata.colorimetry = colorimetry;
        metadata.cursor = cursor;
        CaptureFrame::from_parts(metadata, storage, damage)
    }
}

impl<S: CaptureSurfaceStage> CaptureFrame<S> {
    fn from_parts(
        metadata: CaptureFrameMetadata,
        storage: CaptureStorage,
        damage: CaptureDamage,
    ) -> Result<Self, CaptureFrameError> {
        validate_metadata::<S>(&metadata, &storage, &damage)?;
        Ok(Self {
            metadata,
            storage,
            damage,
            stage: PhantomData,
        })
    }

    /// Envelope metadata.
    #[must_use]
    pub const fn metadata(&self) -> &CaptureFrameMetadata {
        &self.metadata
    }

    /// Backing CPU pixels or opaque GPU surface.
    #[must_use]
    pub const fn storage(&self) -> &CaptureStorage {
        &self.storage
    }

    /// Optional dirty and move metadata.
    #[must_use]
    pub const fn damage(&self) -> &CaptureDamage {
        &self.damage
    }

    /// Runtime stage corresponding to `S`.
    #[must_use]
    pub const fn stage(&self) -> CaptureStageKind {
        S::KIND
    }

    /// Reject a frame published by an obsolete source topology or worker session.
    ///
    /// # Errors
    ///
    /// Returns the precise source or generation mismatch.
    pub fn validate_epoch(&self, expected: &CaptureEpoch) -> Result<(), CaptureFrameError> {
        if self.metadata.source_id != expected.source_id {
            return Err(CaptureFrameError::SourceMismatch {
                expected: expected.source_id.clone(),
                actual: self.metadata.source_id.clone(),
            });
        }
        if self.metadata.topology_generation != expected.topology_generation {
            return Err(CaptureFrameError::StaleTopology {
                expected: expected.topology_generation,
                actual: self.metadata.topology_generation,
            });
        }
        if self.metadata.session_generation != expected.session_generation {
            return Err(CaptureFrameError::StaleSession {
                expected: expected.session_generation,
                actual: self.metadata.session_generation,
            });
        }
        Ok(())
    }
}

fn validate_metadata<S: CaptureSurfaceStage>(
    metadata: &CaptureFrameMetadata,
    storage: &CaptureStorage,
    damage: &CaptureDamage,
) -> Result<(), CaptureFrameError> {
    if metadata.topology_generation == 0 {
        return Err(CaptureFrameError::ZeroGeneration("topology"));
    }
    if metadata.session_generation == 0 {
        return Err(CaptureFrameError::ZeroGeneration("session"));
    }
    if metadata.sequence == 0 {
        return Err(CaptureFrameError::ZeroSequence);
    }
    if metadata.fresh_until < metadata.captured_at {
        return Err(CaptureFrameError::InvalidFreshness);
    }
    match &metadata.cursor.content {
        CaptureCursorContent::Hidden if metadata.cursor.visible => {
            return Err(CaptureFrameError::VisibleCursorMarkedHidden);
        }
        CaptureCursorContent::Separate(shape)
            if metadata.cursor.shape_extent != Some(shape.extent()) =>
        {
            return Err(CaptureFrameError::CursorShapeExtentMismatch {
                metadata: metadata.cursor.shape_extent,
                shape: shape.extent(),
            });
        }
        CaptureCursorContent::Separate(shape)
            if metadata.cursor.shape_generation != Some(shape.generation().get()) =>
        {
            return Err(CaptureFrameError::CursorShapeGenerationMismatch {
                metadata: metadata.cursor.shape_generation,
                shape: shape.generation().get(),
            });
        }
        CaptureCursorContent::Absent
        | CaptureCursorContent::Hidden
        | CaptureCursorContent::Separate(_)
        | CaptureCursorContent::Composed => {}
    }
    if S::KIND == CaptureStageKind::GeometryNormalized
        && metadata.geometry.rotation != CaptureRotation::Identity
    {
        return Err(CaptureFrameError::ProcessedRotationPending(
            metadata.geometry.rotation,
        ));
    }
    if S::KIND == CaptureStageKind::GeometryNormalized
        && let Some(crop) = metadata.geometry.crop
    {
        return Err(CaptureFrameError::ProcessedCropPending(crop));
    }

    match storage {
        CaptureStorage::Cpu(cpu) => cpu.validate(metadata.geometry.storage_extent)?,
        CaptureStorage::Gpu(gpu) if gpu.extent != metadata.geometry.storage_extent => {
            return Err(CaptureFrameError::GpuExtentMismatch {
                storage: gpu.extent,
                geometry: metadata.geometry.storage_extent,
            });
        }
        CaptureStorage::Gpu(_) => {}
    }

    for region in damage.dirty_regions.iter().copied() {
        if !region.fits_within(metadata.geometry.native_extent) {
            return Err(CaptureFrameError::DamageOutOfBounds(region));
        }
    }
    for region in damage.move_regions.iter().copied() {
        let destination = PixelRect {
            x: region.destination.0,
            y: region.destination.1,
            extent: region.source.extent,
        };
        if !region.source.fits_within(metadata.geometry.native_extent)
            || !destination.fits_within(metadata.geometry.native_extent)
        {
            return Err(CaptureFrameError::MoveOutOfBounds(region));
        }
    }
    Ok(())
}

/// Invalid capture metadata rejected at a backend adapter boundary.
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum CaptureFrameError {
    /// Stable source identities cannot be blank.
    #[error("capture source id cannot be empty")]
    EmptySourceId,
    /// Pixel surfaces and regions must be non-empty.
    #[error("capture extent must be non-zero, got {width}x{height}")]
    EmptyExtent { width: u32, height: u32 },
    /// Source scale must be a positive rational number.
    #[error("source scale must be positive, got {numerator}/{denominator}")]
    InvalidSourceScale { numerator: u32, denominator: u32 },
    /// Crop escaped the native scanout extent.
    #[error("capture crop {crop:?} is outside extent {extent:?}")]
    CropOutOfBounds {
        crop: PixelRect,
        extent: PixelExtent,
    },
    /// Topology and session generations start at one.
    #[error("{0} generation must be non-zero")]
    ZeroGeneration(&'static str),
    /// Frame sequence starts at one for each session.
    #[error("capture frame sequence must be non-zero")]
    ZeroSequence,
    /// Freshness deadline preceded acquisition.
    #[error("capture freshness deadline precedes acquisition")]
    InvalidFreshness,
    /// Separately-owned cursor generations start at one.
    #[error("cursor shape generation must be non-zero")]
    ZeroCursorShapeGeneration,
    /// Cursor stride cannot address one complete encoded row.
    #[error("cursor shape stride {stride} is smaller than the {minimum}-byte row")]
    InvalidCursorShapeStride { stride: usize, minimum: usize },
    /// Cursor layout arithmetic overflowed the host address space.
    #[error("cursor shape size overflow")]
    CursorShapeSizeOverflow,
    /// Cursor allocation length disagrees with its exact plane layout.
    #[error("cursor shape has {actual} bytes; expected exactly {expected}")]
    CursorShapeLengthMismatch { actual: usize, expected: usize },
    /// Visible cursor metadata cannot claim an explicitly hidden cursor.
    #[error("visible cursor is marked hidden")]
    VisibleCursorMarkedHidden,
    /// Cursor metadata bounds disagree with separately-owned shape pixels.
    #[error("cursor metadata extent {metadata:?} differs from shape extent {shape:?}")]
    CursorShapeExtentMismatch {
        metadata: Option<PixelExtent>,
        shape: PixelExtent,
    },
    /// Cursor metadata generation disagrees with its separately-owned shape.
    #[error("cursor metadata generation {metadata:?} differs from shape generation {shape}")]
    CursorShapeGenerationMismatch { metadata: Option<u64>, shape: u64 },
    /// Legacy geometry processing cannot rotate separately-owned cursor pixels.
    #[error("separate cursor shape requires shape-aware geometry processing")]
    SeparateCursorGeometryProcessingRequired,
    /// CPU stride cannot address one complete row.
    #[error("CPU stride {stride} is smaller than the {minimum}-byte row")]
    InvalidCpuStride { stride: i64, minimum: usize },
    /// Packed and multi-plane native formats require their scalar decoder.
    #[error("pixel format {0:?} cannot be represented by one RGBA8 CPU plane")]
    UnsupportedCpuStorageFormat(CapturePixelFormat),
    /// CPU row addressing escaped the supplied allocation.
    #[error(
        "CPU storage ({buffer_len} bytes, row0 {row0_offset}, stride {stride}) cannot hold {extent:?}"
    )]
    CpuBufferOutOfBounds {
        buffer_len: usize,
        row0_offset: usize,
        stride: i64,
        extent: PixelExtent,
    },
    /// Storage length arithmetic overflowed the host address space.
    #[error("capture storage size overflow")]
    StorageSizeOverflow,
    /// A demanded CPU capture plane could not be allocated.
    #[error("could not allocate {byte_len} bytes for a capture plane")]
    PlaneAllocationFailed { byte_len: usize },
    /// A demanded CPU capture plane exceeded the shared source byte fence.
    #[error(
        "capture plane needs {requested_bytes} bytes but only {available_bytes} admitted bytes remain"
    )]
    PlaneCapacityExceeded {
        requested_bytes: u64,
        available_bytes: u64,
    },
    /// Opaque GPU handles reserve zero as invalid.
    #[error("GPU surface handle id must be non-zero")]
    InvalidGpuHandle,
    /// GPU descriptor and physical geometry disagree.
    #[error("GPU storage extent {storage:?} differs from geometry {geometry:?}")]
    GpuExtentMismatch {
        storage: PixelExtent,
        geometry: PixelExtent,
    },
    /// A processed surface cannot retain a pending display transform.
    #[error("processed capture surface still has pending rotation {0:?}")]
    ProcessedRotationPending(CaptureRotation),
    /// A processed surface cannot retain a pending native crop.
    #[error("processed capture surface still has pending crop {0:?}")]
    ProcessedCropPending(PixelRect),
    /// GPU geometry needs the owning platform's native interop processor.
    #[error("GPU capture geometry requires platform-native processing")]
    GpuGeometryProcessingRequired,
    /// Legacy pixel averaging only accepts fully-known SDR sRGB samples.
    #[error("legacy screen analysis cannot interpret encoded color metadata {colorimetry:?}")]
    UnsupportedLegacyAnalysisColorimetry { colorimetry: CaptureColorimetry },
    /// Canonical cursor coordinates exceeded the shared signed representation.
    #[error("canonical cursor coordinate exceeds the supported range")]
    CursorCoordinateOverflow,
    /// Canonical capture origin exceeded the shared signed representation.
    #[error("canonical capture origin exceeds the supported range")]
    OriginCoordinateOverflow,
    /// Dirty region escaped the native scanout extent.
    #[error("dirty region {0:?} is outside the capture extent")]
    DamageOutOfBounds(PixelRect),
    /// Move source or destination escaped the native scanout extent.
    #[error("move region {0:?} is outside the capture extent")]
    MoveOutOfBounds(MoveRegion),
    /// Frame identity differs from the active adapter source.
    #[error("capture source mismatch: expected {expected}, got {actual}")]
    SourceMismatch {
        expected: CaptureSourceId,
        actual: CaptureSourceId,
    },
    /// Frame belongs to an obsolete topology.
    #[error("stale topology generation {actual}; expected {expected}")]
    StaleTopology { expected: u64, actual: u64 },
    /// Frame belongs to an obsolete worker session.
    #[error("stale capture session {actual}; expected {expected}")]
    StaleSession { expected: u64, actual: u64 },
}
