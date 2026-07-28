//! Allocation-free logical sampling over raw CPU capture planes.

use std::num::NonZeroU128;

use thiserror::Error;

use super::{
    CaptureColorimetry, CaptureCursor, CaptureCursorContent, CaptureFrame, CaptureFrameError,
    CaptureGeometry, CapturePixelFormat, CaptureRotation, CaptureStorage, CpuCaptureStorage,
    PixelExtent, RawCaptureSurface, ResolvedScreenSource, ScreenRational, ScreenResourceApi,
    ScreenSourceReflection,
};

const CHANNELS_PER_PIXEL: i128 = 4;

/// Exact point in the source's final logical edge space.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CpuSamplingPoint {
    x: ScreenRational,
    y: ScreenRational,
}

impl CpuSamplingPoint {
    /// Construct a point from exact source-space coordinates.
    #[must_use]
    pub const fn new(x: ScreenRational, y: ScreenRational) -> Self {
        Self { x, y }
    }

    /// Construct an integer edge-space point.
    #[must_use]
    pub const fn from_u32(x: u32, y: u32) -> Self {
        Self::new(ScreenRational::from_u32(x), ScreenRational::from_u32(y))
    }

    /// Horizontal logical edge coordinate.
    #[must_use]
    pub const fn x(self) -> ScreenRational {
        self.x
    }

    /// Vertical logical edge coordinate.
    #[must_use]
    pub const fn y(self) -> ScreenRational {
        self.y
    }
}

/// Reduced exact coordinate in CPU storage edge space.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CpuStorageCoordinate {
    numerator: u128,
    denominator: NonZeroU128,
}

impl CpuStorageCoordinate {
    /// Reduced numerator.
    #[must_use]
    pub const fn numerator(self) -> u128 {
        self.numerator
    }

    /// Positive reduced denominator.
    #[must_use]
    pub const fn denominator(self) -> NonZeroU128 {
        self.denominator
    }

    fn floor(self) -> u128 {
        self.numerator / self.denominator.get()
    }

    fn ceil(self) -> u128 {
        let denominator = self.denominator.get();
        self.floor() + u128::from(!self.numerator.is_multiple_of(denominator))
    }
}

/// Exact mapped point plus the orientation of each storage axis.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CpuMappedSamplingPoint {
    x: CpuStorageCoordinate,
    y: CpuStorageCoordinate,
    x_reversed: bool,
    y_reversed: bool,
}

impl CpuMappedSamplingPoint {
    /// Horizontal storage edge coordinate.
    #[must_use]
    pub const fn x(self) -> CpuStorageCoordinate {
        self.x
    }

    /// Vertical storage edge coordinate.
    #[must_use]
    pub const fn y(self) -> CpuStorageCoordinate {
        self.y
    }

    /// Whether increasing logical coordinates move backward on the storage X axis.
    #[must_use]
    pub const fn x_reversed(self) -> bool {
        self.x_reversed
    }

    /// Whether increasing logical coordinates move backward on the storage Y axis.
    #[must_use]
    pub const fn y_reversed(self) -> bool {
        self.y_reversed
    }
}

/// Checked zero-copy geometry lens over one raw CPU capture frame.
///
/// The view validates every source fence represented by [`CaptureFrame`]. The
/// frame envelope does not carry backend device or resource generations, so
/// the source worker remains responsible for pairing the frame with the exact
/// [`ResolvedScreenSource`] resource generation.
#[derive(Debug)]
pub struct CpuSamplingView<'frame> {
    source: &'frame ResolvedScreenSource,
    frame: &'frame CaptureFrame<RawCaptureSurface>,
    storage: &'frame CpuCaptureStorage,
    crop_origin: (u32, u32),
    crop_extent: PixelExtent,
    rotated_crop_extent: PixelExtent,
    storage_x_reversed: bool,
    storage_y_reversed: bool,
}

impl<'frame> CpuSamplingView<'frame> {
    /// Validate a raw CPU frame against one immutable resolved source.
    ///
    /// # Errors
    ///
    /// Rejects stale frames, source metadata mismatches, non-CPU storage,
    /// contradictory scale metadata, and cursor storage the source did not
    /// advertise.
    pub fn try_new(
        frame: &'frame CaptureFrame<RawCaptureSurface>,
        source: &'frame ResolvedScreenSource,
    ) -> Result<Self, CpuSamplingError> {
        frame.validate_epoch(source.epoch())?;
        let config = source.config();
        if frame.metadata().geometry != config.geometry() {
            return Err(CpuSamplingError::SourceGeometryMismatch {
                expected: config.geometry(),
                actual: frame.metadata().geometry,
            });
        }
        if frame.metadata().colorimetry != config.colorimetry() {
            return Err(CpuSamplingError::SourceColorimetryMismatch {
                expected: config.colorimetry(),
                actual: frame.metadata().colorimetry,
            });
        }
        if !matches!(config.resources().api(), ScreenResourceApi::Cpu) {
            return Err(CpuSamplingError::UnsupportedSourceResource);
        }
        let CaptureStorage::Cpu(storage) = frame.storage() else {
            return Err(CpuSamplingError::GpuFrameStorage);
        };
        if storage.format() != config.pixel_format() {
            return Err(CpuSamplingError::SourcePixelFormatMismatch {
                expected: config.pixel_format(),
                actual: storage.format(),
            });
        }
        validate_cursor_content(&frame.metadata().cursor.content, source)?;

        let geometry = config.geometry();
        let (crop_origin, crop_extent) = geometry
            .crop()
            .map_or(((0, 0), geometry.native_extent()), |crop| {
                ((crop.x(), crop.y()), crop.extent())
            });
        let rotated_crop_extent = geometry.rotation().apply_to_extent(crop_extent);
        validate_logical_scale(geometry, rotated_crop_extent, config.logical_extent())?;
        let (storage_x_reversed, storage_y_reversed) =
            storage_axis_directions(geometry.rotation(), config.reflection());

        Ok(Self {
            source,
            frame,
            storage,
            crop_origin,
            crop_extent,
            rotated_crop_extent,
            storage_x_reversed,
            storage_y_reversed,
        })
    }

    /// Exact source logical extent after crop, rotation, scale, and reflection.
    #[must_use]
    pub const fn logical_extent(&self) -> PixelExtent {
        self.source.logical_extent()
    }

    /// Extent of the retained CPU pixel plane.
    #[must_use]
    pub const fn storage_extent(&self) -> PixelExtent {
        self.frame.metadata().geometry.storage_extent()
    }

    /// Native acquisition sequence borrowed by this view.
    #[must_use]
    pub const fn source_sequence(&self) -> u64 {
        self.frame.metadata().sequence
    }

    /// Cursor ownership metadata retained without composition.
    #[must_use]
    pub const fn cursor(&self) -> &CaptureCursor {
        &self.frame.metadata().cursor
    }

    /// Map an exact logical edge-space point to CPU storage edge space.
    ///
    /// Mapping follows the canonical crop, clockwise rotation, source scale,
    /// and reflection contract without materializing a normalized raster.
    ///
    /// # Errors
    ///
    /// Rejects points outside the inclusive logical edge bounds or arithmetic
    /// that cannot be represented exactly in the checked `u128` domain.
    pub fn map_logical_edge(
        &self,
        point: CpuSamplingPoint,
    ) -> Result<CpuMappedSamplingPoint, CpuSamplingError> {
        validate_logical_point(point, self.logical_extent())?;
        let reflection = self.source.config().reflection();
        let mut x = WideRational::from_screen(point.x);
        let mut y = WideRational::from_screen(point.y);
        if matches!(
            reflection,
            ScreenSourceReflection::Horizontal | ScreenSourceReflection::Both
        ) {
            x = x.checked_reflect(self.logical_extent().width())?;
        }
        if matches!(
            reflection,
            ScreenSourceReflection::Vertical | ScreenSourceReflection::Both
        ) {
            y = y.checked_reflect(self.logical_extent().height())?;
        }
        x = x.checked_scale(
            self.rotated_crop_extent.width(),
            self.logical_extent().width(),
        )?;
        y = y.checked_scale(
            self.rotated_crop_extent.height(),
            self.logical_extent().height(),
        )?;

        let (native_x, native_y) = match self.frame.metadata().geometry.rotation() {
            CaptureRotation::Identity => (x, y),
            CaptureRotation::Clockwise90 => (y, x.checked_reflect(self.crop_extent.height())?),
            CaptureRotation::Clockwise180 => (
                x.checked_reflect(self.crop_extent.width())?,
                y.checked_reflect(self.crop_extent.height())?,
            ),
            CaptureRotation::Clockwise270 => (y.checked_reflect(self.crop_extent.width())?, x),
        };
        let geometry = self.frame.metadata().geometry;
        let storage_x = native_x
            .checked_add_integer(self.crop_origin.0)?
            .checked_scale(
                geometry.storage_extent().width(),
                geometry.native_extent().width(),
            )?;
        let storage_y = native_y
            .checked_add_integer(self.crop_origin.1)?
            .checked_scale(
                geometry.storage_extent().height(),
                geometry.native_extent().height(),
            )?;

        Ok(CpuMappedSamplingPoint {
            x: storage_x.into_storage_coordinate(),
            y: storage_y.into_storage_coordinate(),
            x_reversed: self.storage_x_reversed,
            y_reversed: self.storage_y_reversed,
        })
    }

    /// Read the nearest stored sample as canonical RGBA bytes.
    ///
    /// Reflected integer-boundary ties select the texel on the descending side,
    /// preserving byte symmetry with the corresponding forward transform.
    ///
    /// # Errors
    ///
    /// Returns a mapping or checked storage-address error.
    pub fn read_logical_nearest(
        &self,
        point: CpuSamplingPoint,
    ) -> Result<[u8; 4], CpuSamplingError> {
        let mapped = self.map_logical_edge(point)?;
        let extent = self.storage_extent();
        let x = nearest_index(mapped.x, extent.width(), mapped.x_reversed)?;
        let y = nearest_index(mapped.y, extent.height(), mapped.y_reversed)?;
        self.read_storage_pixel(x, y)
    }

    fn read_storage_pixel(&self, x: u32, y: u32) -> Result<[u8; 4], CpuSamplingError> {
        let row_offset = i128::try_from(self.storage.row0_offset())
            .map_err(|_| CpuSamplingError::StorageAddressOverflow)?
            .checked_add(
                i128::from(self.storage.row_stride())
                    .checked_mul(i128::from(y))
                    .ok_or(CpuSamplingError::StorageAddressOverflow)?,
            )
            .ok_or(CpuSamplingError::StorageAddressOverflow)?;
        let pixel_offset = row_offset
            .checked_add(i128::from(x) * CHANNELS_PER_PIXEL)
            .ok_or(CpuSamplingError::StorageAddressOverflow)?;
        let pixel_offset =
            usize::try_from(pixel_offset).map_err(|_| CpuSamplingError::StorageAddressOverflow)?;
        let pixel_end = pixel_offset
            .checked_add(4)
            .ok_or(CpuSamplingError::StorageAddressOverflow)?;
        let pixel = self
            .storage
            .bytes()
            .get(pixel_offset..pixel_end)
            .ok_or(CpuSamplingError::StorageAddressOverflow)?;
        Ok(match self.storage.format() {
            CapturePixelFormat::Rgba8 => [pixel[0], pixel[1], pixel[2], pixel[3]],
            CapturePixelFormat::Bgra8 => [pixel[2], pixel[1], pixel[0], pixel[3]],
        })
    }
}

#[derive(Clone, Copy, Debug)]
struct WideRational {
    numerator: u128,
    denominator: NonZeroU128,
}

impl WideRational {
    fn from_screen(value: ScreenRational) -> Self {
        Self {
            numerator: u128::from(value.numerator()),
            denominator: NonZeroU128::new(u128::from(value.denominator().get()))
                .expect("screen rational denominators are non-zero"),
        }
    }

    fn checked_reflect(self, extent: u32) -> Result<Self, CpuSamplingError> {
        let numerator = u128::from(extent)
            .checked_mul(self.denominator.get())
            .and_then(|extent_numerator| extent_numerator.checked_sub(self.numerator))
            .ok_or(CpuSamplingError::GeometryArithmeticOverflow)?;
        Self::new(numerator, self.denominator.get())
    }

    fn checked_add_integer(self, value: u32) -> Result<Self, CpuSamplingError> {
        let numerator = u128::from(value)
            .checked_mul(self.denominator.get())
            .and_then(|offset| self.numerator.checked_add(offset))
            .ok_or(CpuSamplingError::GeometryArithmeticOverflow)?;
        Self::new(numerator, self.denominator.get())
    }

    fn checked_scale(self, numerator: u32, denominator: u32) -> Result<Self, CpuSamplingError> {
        let mut numerator = u128::from(numerator);
        let mut denominator = u128::from(denominator);
        let ratio_divisor = greatest_common_divisor(numerator, denominator);
        numerator /= ratio_divisor;
        denominator /= ratio_divisor;
        let left_divisor = greatest_common_divisor(self.numerator, denominator);
        let right_divisor = greatest_common_divisor(numerator, self.denominator.get());
        let result_numerator = (self.numerator / left_divisor)
            .checked_mul(numerator / right_divisor)
            .ok_or(CpuSamplingError::GeometryArithmeticOverflow)?;
        let result_denominator = (self.denominator.get() / right_divisor)
            .checked_mul(denominator / left_divisor)
            .ok_or(CpuSamplingError::GeometryArithmeticOverflow)?;
        Self::new(result_numerator, result_denominator)
    }

    fn new(numerator: u128, denominator: u128) -> Result<Self, CpuSamplingError> {
        let denominator =
            NonZeroU128::new(denominator).ok_or(CpuSamplingError::GeometryArithmeticOverflow)?;
        let divisor = greatest_common_divisor(numerator, denominator.get());
        Ok(Self {
            numerator: numerator / divisor,
            denominator: NonZeroU128::new(denominator.get() / divisor)
                .ok_or(CpuSamplingError::GeometryArithmeticOverflow)?,
        })
    }

    const fn into_storage_coordinate(self) -> CpuStorageCoordinate {
        CpuStorageCoordinate {
            numerator: self.numerator,
            denominator: self.denominator,
        }
    }
}

fn greatest_common_divisor(mut left: u128, mut right: u128) -> u128 {
    while right != 0 {
        let remainder = left % right;
        left = right;
        right = remainder;
    }
    left
}

fn validate_logical_scale(
    geometry: CaptureGeometry,
    rotated_crop_extent: PixelExtent,
    logical_extent: PixelExtent,
) -> Result<(), CpuSamplingError> {
    let scale = geometry.source_scale();
    let width_matches = u64::from(logical_extent.width()) * u64::from(scale.denominator())
        == u64::from(rotated_crop_extent.width()) * u64::from(scale.numerator());
    let height_matches = u64::from(logical_extent.height()) * u64::from(scale.denominator())
        == u64::from(rotated_crop_extent.height()) * u64::from(scale.numerator());
    if width_matches && height_matches {
        Ok(())
    } else {
        Err(CpuSamplingError::LogicalExtentScaleMismatch {
            logical_extent,
            rotated_crop_extent,
            scale_numerator: scale.numerator(),
            scale_denominator: scale.denominator(),
        })
    }
}

fn validate_logical_point(
    point: CpuSamplingPoint,
    extent: PixelExtent,
) -> Result<(), CpuSamplingError> {
    if rational_within_extent(point.x, extent.width())
        && rational_within_extent(point.y, extent.height())
    {
        Ok(())
    } else {
        Err(CpuSamplingError::LogicalPointOutOfBounds { point, extent })
    }
}

fn rational_within_extent(value: ScreenRational, extent: u32) -> bool {
    u128::from(value.numerator()) <= u128::from(extent) * u128::from(value.denominator().get())
}

fn storage_axis_directions(
    rotation: CaptureRotation,
    reflection: ScreenSourceReflection,
) -> (bool, bool) {
    let logical_x_reversed = matches!(
        reflection,
        ScreenSourceReflection::Horizontal | ScreenSourceReflection::Both
    );
    let logical_y_reversed = matches!(
        reflection,
        ScreenSourceReflection::Vertical | ScreenSourceReflection::Both
    );
    match rotation {
        CaptureRotation::Identity => (logical_x_reversed, logical_y_reversed),
        CaptureRotation::Clockwise90 => (logical_y_reversed, !logical_x_reversed),
        CaptureRotation::Clockwise180 => (!logical_x_reversed, !logical_y_reversed),
        CaptureRotation::Clockwise270 => (!logical_y_reversed, logical_x_reversed),
    }
}

fn nearest_index(
    coordinate: CpuStorageCoordinate,
    extent: u32,
    reversed: bool,
) -> Result<u32, CpuSamplingError> {
    let candidate = if reversed {
        coordinate.ceil().saturating_sub(1)
    } else {
        coordinate.floor()
    };
    let bounded = candidate.min(u128::from(extent - 1));
    u32::try_from(bounded).map_err(|_| CpuSamplingError::StorageAddressOverflow)
}

fn validate_cursor_content(
    content: &CaptureCursorContent,
    source: &ResolvedScreenSource,
) -> Result<(), CpuSamplingError> {
    let capabilities = source.config().cursor_capabilities();
    let supported = match content {
        CaptureCursorContent::Separate(_) => {
            capabilities.has_clean_surface() && capabilities.has_separate_cursor()
        }
        CaptureCursorContent::Composed => capabilities.has_composed_surface(),
        CaptureCursorContent::Absent | CaptureCursorContent::Hidden => true,
    };
    if supported {
        Ok(())
    } else {
        Err(CpuSamplingError::CursorContentUnsupported)
    }
}

/// Invalid source/frame pairing or logical-to-storage mapping.
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum CpuSamplingError {
    /// Frame identity or generation differs from the resolved source.
    #[error(transparent)]
    CaptureFrame(#[from] CaptureFrameError),
    /// The resolved source promises a non-CPU resource API.
    #[error("CPU sampling requires a CPU source resource")]
    UnsupportedSourceResource,
    /// The frame contains an opaque GPU surface.
    #[error("CPU sampling cannot read GPU frame storage")]
    GpuFrameStorage,
    /// Frame geometry differs from the resolved source snapshot.
    #[error("frame geometry {actual:?} differs from resolved geometry {expected:?}")]
    SourceGeometryMismatch {
        expected: CaptureGeometry,
        actual: CaptureGeometry,
    },
    /// Frame color metadata differs from the resolved source snapshot.
    #[error("frame colorimetry {actual:?} differs from resolved colorimetry {expected:?}")]
    SourceColorimetryMismatch {
        expected: CaptureColorimetry,
        actual: CaptureColorimetry,
    },
    /// CPU plane format differs from the resolved source snapshot.
    #[error("frame pixel format {actual:?} differs from resolved format {expected:?}")]
    SourcePixelFormatMismatch {
        expected: CapturePixelFormat,
        actual: CapturePixelFormat,
    },
    /// Logical extent contradicts the exact physical-to-logical scale.
    #[error(
        "logical extent {logical_extent:?} does not equal rotated crop {rotated_crop_extent:?} scaled by {scale_numerator}/{scale_denominator}"
    )]
    LogicalExtentScaleMismatch {
        logical_extent: PixelExtent,
        rotated_crop_extent: PixelExtent,
        scale_numerator: u32,
        scale_denominator: u32,
    },
    /// Cursor storage contradicts the resolved source capabilities.
    #[error("frame cursor storage is not supported by the resolved source")]
    CursorContentUnsupported,
    /// A point escaped the inclusive source edge bounds.
    #[error("logical point {point:?} is outside source extent {extent:?}")]
    LogicalPointOutOfBounds {
        point: CpuSamplingPoint,
        extent: PixelExtent,
    },
    /// Exact geometry arithmetic exceeded the checked `u128` domain.
    #[error("logical-to-storage geometry arithmetic overflowed")]
    GeometryArithmeticOverflow,
    /// A mapped pixel could not be addressed within the CPU plane.
    #[error("mapped CPU storage address overflowed")]
    StorageAddressOverflow,
}
