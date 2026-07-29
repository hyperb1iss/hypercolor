//! Allocation-free logical sampling over raw CPU capture planes.

use std::cmp::Ordering;
use std::num::NonZeroU128;

use thiserror::Error;

use super::{
    CaptureColorimetry, CaptureCursor, CaptureCursorContent, CaptureFrame, CaptureFrameError,
    CaptureGeometry, CapturePixelFormat, CaptureRotation, CaptureStorage, CpuCaptureStorage,
    PixelExtent, RawCaptureSurface, ResolvedScreenSource, ScreenRational, ScreenResourceApi,
    ScreenSourceReflection, ScreenSubpixelRect,
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

#[derive(Clone, Copy, Debug)]
pub(crate) struct CpuSamplingTransform {
    geometry: CaptureGeometry,
    logical_extent: PixelExtent,
    reflection: ScreenSourceReflection,
    crop_origin: (u32, u32),
    crop_extent: PixelExtent,
    rotated_crop_extent: PixelExtent,
    storage_x_reversed: bool,
    storage_y_reversed: bool,
}

impl CpuSamplingTransform {
    pub(crate) fn try_from_source(source: &ResolvedScreenSource) -> Result<Self, CpuSamplingError> {
        let config = source.config();
        if !matches!(config.resources().api(), ScreenResourceApi::Cpu) {
            return Err(CpuSamplingError::UnsupportedSourceResource);
        }
        Self::try_from_cpu_executor(source)
    }

    pub(crate) fn try_from_cpu_executor(
        source: &ResolvedScreenSource,
    ) -> Result<Self, CpuSamplingError> {
        let config = source.config();
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
            geometry,
            logical_extent: config.logical_extent(),
            reflection: config.reflection(),
            crop_origin,
            crop_extent,
            rotated_crop_extent,
            storage_x_reversed,
            storage_y_reversed,
        })
    }

    fn map_logical_edge(
        self,
        point: WideSamplingPoint,
    ) -> Result<CpuMappedSamplingPoint, CpuSamplingError> {
        validate_wide_logical_point(point, self.logical_extent)?;
        let mut x = point.x;
        let mut y = point.y;
        if matches!(
            self.reflection,
            ScreenSourceReflection::Horizontal | ScreenSourceReflection::Both
        ) {
            x = x.checked_reflect(self.logical_extent.width())?;
        }
        if matches!(
            self.reflection,
            ScreenSourceReflection::Vertical | ScreenSourceReflection::Both
        ) {
            y = y.checked_reflect(self.logical_extent.height())?;
        }
        x = x.checked_scale(
            u128::from(self.rotated_crop_extent.width()),
            u128::from(self.logical_extent.width()),
        )?;
        y = y.checked_scale(
            u128::from(self.rotated_crop_extent.height()),
            u128::from(self.logical_extent.height()),
        )?;

        let (native_x, native_y) = match self.geometry.rotation() {
            CaptureRotation::Identity => (x, y),
            CaptureRotation::Clockwise90 => (y, x.checked_reflect(self.crop_extent.height())?),
            CaptureRotation::Clockwise180 => (
                x.checked_reflect(self.crop_extent.width())?,
                y.checked_reflect(self.crop_extent.height())?,
            ),
            CaptureRotation::Clockwise270 => (y.checked_reflect(self.crop_extent.width())?, x),
        };
        let storage_x = native_x
            .checked_add_integer(self.crop_origin.0)?
            .checked_scale(
                u128::from(self.geometry.storage_extent().width()),
                u128::from(self.geometry.native_extent().width()),
            )?;
        let storage_y = native_y
            .checked_add_integer(self.crop_origin.1)?
            .checked_scale(
                u128::from(self.geometry.storage_extent().height()),
                u128::from(self.geometry.native_extent().height()),
            )?;

        Ok(CpuMappedSamplingPoint {
            x: storage_x.into_storage_coordinate(),
            y: storage_y.into_storage_coordinate(),
            x_reversed: self.storage_x_reversed,
            y_reversed: self.storage_y_reversed,
        })
    }

    pub(crate) fn prepare_region(
        self,
        region: ScreenSubpixelRect,
        target_extent: PixelExtent,
    ) -> Result<PreparedCpuSamplingPlan, CpuSamplingError> {
        let origin = WideSamplingPoint {
            x: WideRational::from_screen(region.x()),
            y: WideRational::from_screen(region.y()),
        };
        let width = WideRational::from_screen(region.width());
        let height = WideRational::from_screen(region.height());
        if width.is_zero() || height.is_zero() {
            return Err(CpuSamplingError::EmptySourceRegion);
        }
        let end = WideSamplingPoint {
            x: origin.x.checked_add(width)?,
            y: origin.y.checked_add(height)?,
        };
        if end
            .x
            .cmp_exact(WideRational::from_u32(self.logical_extent.width()))
            == Ordering::Greater
            || end
                .y
                .cmp_exact(WideRational::from_u32(self.logical_extent.height()))
                == Ordering::Greater
        {
            return Err(CpuSamplingError::SourceRegionOutOfBounds);
        }

        let mapped_origin = self.map_logical_edge(origin)?;
        let mapped_x_end = self.map_logical_edge(WideSamplingPoint {
            x: end.x,
            y: origin.y,
        })?;
        let mapped_y_end = self.map_logical_edge(WideSamplingPoint {
            x: origin.x,
            y: end.y,
        })?;
        let logical_x_axis = match self.geometry.rotation() {
            CaptureRotation::Identity | CaptureRotation::Clockwise180 => CpuStorageAxis::X,
            CaptureRotation::Clockwise90 | CaptureRotation::Clockwise270 => CpuStorageAxis::Y,
        };
        let logical_y_axis = logical_x_axis.other();
        let logical_x = ExactAxisGrid::new(
            logical_x_axis,
            mapped_origin.coordinate(logical_x_axis),
            mapped_x_end.coordinate(logical_x_axis),
            target_extent.width(),
            self.storage_axis_extent(logical_x_axis),
        )?;
        let logical_y = ExactAxisGrid::new(
            logical_y_axis,
            mapped_origin.coordinate(logical_y_axis),
            mapped_y_end.coordinate(logical_y_axis),
            target_extent.height(),
            self.storage_axis_extent(logical_y_axis),
        )?;
        Ok(PreparedCpuSamplingPlan {
            logical_x,
            logical_y,
        })
    }

    const fn storage_axis_extent(self, axis: CpuStorageAxis) -> u32 {
        match axis {
            CpuStorageAxis::X => self.geometry.storage_extent().width(),
            CpuStorageAxis::Y => self.geometry.storage_extent().height(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CpuStorageAxis {
    X,
    Y,
}

impl CpuStorageAxis {
    const fn other(self) -> Self {
        match self {
            Self::X => Self::Y,
            Self::Y => Self::X,
        }
    }
}

impl CpuMappedSamplingPoint {
    const fn coordinate(self, axis: CpuStorageAxis) -> CpuStorageCoordinate {
        match axis {
            CpuStorageAxis::X => self.x,
            CpuStorageAxis::Y => self.y,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct PreparedCpuSamplingPlan {
    logical_x: ExactAxisGrid,
    logical_y: ExactAxisGrid,
}

impl PreparedCpuSamplingPlan {
    pub(crate) const fn logical_x_storage_axis(&self) -> CpuStorageAxis {
        self.logical_x.storage_axis
    }

    pub(crate) fn logical_x_nearest(&self, target_x: u32) -> u32 {
        self.logical_x.nearest(target_x)
    }

    pub(crate) fn logical_y_nearest(&self, target_y: u32) -> u32 {
        self.logical_y.nearest(target_y)
    }

    pub(crate) fn logical_x_bilinear(&self, target_x: u32) -> CpuAxisInterpolation {
        self.logical_x.bilinear(target_x)
    }

    pub(crate) fn logical_y_bilinear(&self, target_y: u32) -> CpuAxisInterpolation {
        self.logical_y.bilinear(target_y)
    }

    pub(crate) fn logical_x_area(&self, target_x: u32) -> CpuStorageSpan {
        self.logical_x.cell_span(target_x)
    }

    pub(crate) fn logical_y_area(&self, target_y: u32) -> CpuStorageSpan {
        self.logical_y.cell_span(target_y)
    }
}

#[derive(Clone, Debug)]
struct ExactAxisGrid {
    storage_axis: CpuStorageAxis,
    storage_extent: u32,
    denominator: NonZeroU128,
    start_units: u128,
    half_step_units: u128,
    reversed: bool,
}

impl ExactAxisGrid {
    fn new(
        storage_axis: CpuStorageAxis,
        start: CpuStorageCoordinate,
        end: CpuStorageCoordinate,
        target_len: u32,
        storage_extent: u32,
    ) -> Result<Self, CpuSamplingError> {
        let divisor = greatest_common_divisor(start.denominator.get(), end.denominator.get());
        let start_factor = end.denominator.get() / divisor;
        let end_factor = start.denominator.get() / divisor;
        let common_denominator = start
            .denominator
            .get()
            .checked_mul(start_factor)
            .ok_or(CpuSamplingError::GeometryArithmeticOverflow)?;
        let start_common = start
            .numerator
            .checked_mul(start_factor)
            .ok_or(CpuSamplingError::GeometryArithmeticOverflow)?;
        let end_common = end
            .numerator
            .checked_mul(end_factor)
            .ok_or(CpuSamplingError::GeometryArithmeticOverflow)?;
        let doubled_target = u128::from(target_len)
            .checked_mul(2)
            .ok_or(CpuSamplingError::GeometryArithmeticOverflow)?;
        let denominator = common_denominator
            .checked_mul(doubled_target)
            .and_then(NonZeroU128::new)
            .ok_or(CpuSamplingError::GeometryArithmeticOverflow)?;
        let start_units = start_common
            .checked_mul(doubled_target)
            .ok_or(CpuSamplingError::GeometryArithmeticOverflow)?;
        let end_units = end_common
            .checked_mul(doubled_target)
            .ok_or(CpuSamplingError::GeometryArithmeticOverflow)?;
        let reversed = start_units > end_units;
        let half_step_units = start_common.abs_diff(end_common);
        let complete_span = half_step_units
            .checked_mul(doubled_target)
            .ok_or(CpuSamplingError::GeometryArithmeticOverflow)?;
        let resolved_end = if reversed {
            start_units.checked_sub(complete_span)
        } else {
            start_units.checked_add(complete_span)
        }
        .ok_or(CpuSamplingError::GeometryArithmeticOverflow)?;
        if resolved_end != end_units {
            return Err(CpuSamplingError::GeometryArithmeticOverflow);
        }
        u128::from(storage_extent)
            .checked_mul(denominator.get())
            .ok_or(CpuSamplingError::GeometryArithmeticOverflow)?;
        Ok(Self {
            storage_axis,
            storage_extent,
            denominator,
            start_units,
            half_step_units,
            reversed,
        })
    }

    fn coordinate(&self, factor: u128) -> CpuStorageCoordinate {
        let delta = self
            .half_step_units
            .checked_mul(factor)
            .expect("prepared axis factor remains inside the complete span");
        let numerator = if self.reversed {
            self.start_units
                .checked_sub(delta)
                .expect("prepared descending axis remains non-negative")
        } else {
            self.start_units
                .checked_add(delta)
                .expect("prepared ascending axis remains addressable")
        };
        CpuStorageCoordinate {
            numerator,
            denominator: self.denominator,
        }
    }

    fn center(&self, index: u32) -> CpuStorageCoordinate {
        self.coordinate(u128::from(index) * 2 + 1)
    }

    fn edge(&self, index: u32) -> CpuStorageCoordinate {
        self.coordinate(u128::from(index) * 2)
    }

    fn nearest(&self, index: u32) -> u32 {
        nearest_index(self.center(index), self.storage_extent, self.reversed)
            .expect("prepared nearest coordinate remains inside storage")
    }

    fn bilinear(&self, index: u32) -> CpuAxisInterpolation {
        CpuAxisInterpolation::new(self.center(index), self.storage_extent)
    }

    fn cell_span(&self, index: u32) -> CpuStorageSpan {
        CpuStorageSpan::new(self.edge(index), self.edge(index + 1), self.storage_extent)
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct CpuAxisInterpolation {
    lower: u32,
    upper: u32,
    upper_weight: f64,
}

impl CpuAxisInterpolation {
    fn new(coordinate: CpuStorageCoordinate, extent: u32) -> Self {
        let denominator = coordinate.denominator.get();
        let integer = coordinate.numerator / denominator;
        let remainder = coordinate.numerator % denominator;
        let below_half = remainder < denominator - remainder;
        if integer == 0 && remainder <= denominator - remainder {
            return Self {
                lower: 0,
                upper: 0,
                upper_weight: 0.0,
            };
        }
        let last = u128::from(extent - 1);
        if integer > last || integer == last && !below_half {
            return Self {
                lower: extent - 1,
                upper: extent - 1,
                upper_weight: 0.0,
            };
        }
        let fraction = remainder as f64 / denominator as f64;
        let (lower, upper_weight) = if below_half {
            (integer - 1, fraction + 0.5)
        } else {
            (integer, fraction - 0.5)
        };
        let lower = u32::try_from(lower).expect("prepared interpolation remains inside storage");
        Self {
            lower,
            upper: lower + 1,
            upper_weight,
        }
    }

    pub(crate) const fn lower(self) -> u32 {
        self.lower
    }

    pub(crate) const fn upper(self) -> u32 {
        self.upper
    }

    pub(crate) const fn upper_weight(self) -> f64 {
        self.upper_weight
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct CpuStorageSpan {
    start: u32,
    end: u32,
    low_units: u128,
    high_units: u128,
    denominator: NonZeroU128,
}

impl CpuStorageSpan {
    fn new(first: CpuStorageCoordinate, second: CpuStorageCoordinate, storage_extent: u32) -> Self {
        debug_assert_eq!(first.denominator, second.denominator);
        let (low_units, high_units) = if first.numerator <= second.numerator {
            (first.numerator, second.numerator)
        } else {
            (second.numerator, first.numerator)
        };
        let start = u32::try_from(low_units / first.denominator.get())
            .expect("prepared area start remains inside storage");
        let end = u32::try_from(
            high_units
                .div_ceil(first.denominator.get())
                .min(u128::from(storage_extent)),
        )
        .expect("prepared area end remains inside storage");
        Self {
            start,
            end,
            low_units,
            high_units,
            denominator: first.denominator,
        }
    }

    pub(crate) const fn start(self) -> u32 {
        self.start
    }

    pub(crate) const fn end(self) -> u32 {
        self.end
    }

    pub(crate) fn normalized_weight(self, pixel: u32) -> f64 {
        let pixel_left = u128::from(pixel)
            .checked_mul(self.denominator.get())
            .expect("prepared area pixel boundary remains addressable");
        let pixel_right = u128::from(pixel + 1)
            .checked_mul(self.denominator.get())
            .expect("prepared area pixel boundary remains addressable");
        let overlap = self.high_units.min(pixel_right) - self.low_units.max(pixel_left);
        overlap as f64 / self.high_units.abs_diff(self.low_units) as f64
    }
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
    transform: CpuSamplingTransform,
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
        let transform = CpuSamplingTransform::try_from_source(source)?;
        Self::try_new_prepared(frame, source, transform)
    }

    pub(crate) fn try_new_prepared(
        frame: &'frame CaptureFrame<RawCaptureSurface>,
        source: &'frame ResolvedScreenSource,
        transform: CpuSamplingTransform,
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

        Ok(Self {
            source,
            frame,
            storage,
            transform,
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
        self.transform.map_logical_edge(WideSamplingPoint {
            x: WideRational::from_screen(point.x),
            y: WideRational::from_screen(point.y),
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

    pub(crate) fn read_storage_pixel(&self, x: u32, y: u32) -> Result<[u8; 4], CpuSamplingError> {
        self.storage_row(y)?.read_rgba(x)
    }

    pub(crate) fn storage_row(&self, y: u32) -> Result<CpuSamplingRow<'frame>, CpuSamplingError> {
        if y >= self.storage_extent().height() {
            return Err(CpuSamplingError::StorageAddressOverflow);
        }
        let row_offset = i128::try_from(self.storage.row0_offset())
            .map_err(|_| CpuSamplingError::StorageAddressOverflow)?
            .checked_add(
                i128::from(self.storage.row_stride())
                    .checked_mul(i128::from(y))
                    .ok_or(CpuSamplingError::StorageAddressOverflow)?,
            )
            .ok_or(CpuSamplingError::StorageAddressOverflow)?;
        let row_offset =
            usize::try_from(row_offset).map_err(|_| CpuSamplingError::StorageAddressOverflow)?;
        let row_bytes = usize::try_from(self.storage_extent().width())
            .map_err(|_| CpuSamplingError::StorageAddressOverflow)?
            .checked_mul(
                usize::try_from(CHANNELS_PER_PIXEL)
                    .expect("capture channel count fits the process address space"),
            )
            .ok_or(CpuSamplingError::StorageAddressOverflow)?;
        let row_end = row_offset
            .checked_add(row_bytes)
            .ok_or(CpuSamplingError::StorageAddressOverflow)?;
        let bytes = self
            .storage
            .bytes()
            .get(row_offset..row_end)
            .ok_or(CpuSamplingError::StorageAddressOverflow)?;
        Ok(CpuSamplingRow {
            bytes,
            format: self.storage.format(),
        })
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct CpuSamplingRow<'frame> {
    bytes: &'frame [u8],
    format: CapturePixelFormat,
}

impl CpuSamplingRow<'_> {
    pub(crate) fn read_rgba(self, x: u32) -> Result<[u8; 4], CpuSamplingError> {
        let pixel_offset = usize::try_from(x)
            .map_err(|_| CpuSamplingError::StorageAddressOverflow)?
            .checked_mul(
                usize::try_from(CHANNELS_PER_PIXEL)
                    .expect("capture channel count fits the process address space"),
            )
            .ok_or(CpuSamplingError::StorageAddressOverflow)?;
        let pixel_end = pixel_offset
            .checked_add(4)
            .ok_or(CpuSamplingError::StorageAddressOverflow)?;
        let pixel = self
            .bytes
            .get(pixel_offset..pixel_end)
            .ok_or(CpuSamplingError::StorageAddressOverflow)?;
        Ok(match self.format {
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

    fn checked_add(self, other: Self) -> Result<Self, CpuSamplingError> {
        let denominator_divisor =
            greatest_common_divisor(self.denominator.get(), other.denominator.get());
        let self_factor = other.denominator.get() / denominator_divisor;
        let other_factor = self.denominator.get() / denominator_divisor;
        let numerator = self
            .numerator
            .checked_mul(self_factor)
            .and_then(|left| {
                other
                    .numerator
                    .checked_mul(other_factor)
                    .and_then(|right| left.checked_add(right))
            })
            .ok_or(CpuSamplingError::GeometryArithmeticOverflow)?;
        let denominator = self
            .denominator
            .get()
            .checked_mul(self_factor)
            .ok_or(CpuSamplingError::GeometryArithmeticOverflow)?;
        Self::new(numerator, denominator)
    }

    fn checked_scale(
        self,
        mut numerator: u128,
        mut denominator: u128,
    ) -> Result<Self, CpuSamplingError> {
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

    fn cmp_exact(self, other: Self) -> Ordering {
        compare_non_negative_rationals(
            self.numerator,
            self.denominator.get(),
            other.numerator,
            other.denominator.get(),
        )
    }

    const fn is_zero(self) -> bool {
        self.numerator == 0
    }

    const fn from_u32(value: u32) -> Self {
        Self {
            numerator: value as u128,
            denominator: NonZeroU128::MIN,
        }
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

#[derive(Clone, Copy, Debug)]
struct WideSamplingPoint {
    x: WideRational,
    y: WideRational,
}

fn greatest_common_divisor(mut left: u128, mut right: u128) -> u128 {
    while right != 0 {
        let remainder = left % right;
        left = right;
        right = remainder;
    }
    left
}

fn compare_non_negative_rationals(
    mut left_numerator: u128,
    mut left_denominator: u128,
    mut right_numerator: u128,
    mut right_denominator: u128,
) -> Ordering {
    let mut reversed = false;
    loop {
        let left_integer = left_numerator / left_denominator;
        let right_integer = right_numerator / right_denominator;
        let ordering = left_integer.cmp(&right_integer);
        if ordering != Ordering::Equal {
            return if reversed {
                ordering.reverse()
            } else {
                ordering
            };
        }
        let left_remainder = left_numerator % left_denominator;
        let right_remainder = right_numerator % right_denominator;
        match (left_remainder == 0, right_remainder == 0) {
            (true, true) => return Ordering::Equal,
            (true, false) => {
                return if reversed {
                    Ordering::Greater
                } else {
                    Ordering::Less
                };
            }
            (false, true) => {
                return if reversed {
                    Ordering::Less
                } else {
                    Ordering::Greater
                };
            }
            (false, false) => {
                left_numerator = left_denominator;
                left_denominator = left_remainder;
                right_numerator = right_denominator;
                right_denominator = right_remainder;
                reversed = !reversed;
            }
        }
    }
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

fn validate_wide_logical_point(
    point: WideSamplingPoint,
    extent: PixelExtent,
) -> Result<(), CpuSamplingError> {
    if point.x.cmp_exact(WideRational::from_u32(extent.width())) != Ordering::Greater
        && point.y.cmp_exact(WideRational::from_u32(extent.height())) != Ordering::Greater
    {
        Ok(())
    } else {
        Err(CpuSamplingError::SourceRegionOutOfBounds)
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
    /// A descriptor selected an empty source-space region.
    #[error("CPU sampling source region must be non-empty")]
    EmptySourceRegion,
    /// A descriptor source-space region escaped the final logical extent.
    #[error("CPU sampling source region is outside the final logical extent")]
    SourceRegionOutOfBounds,
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
