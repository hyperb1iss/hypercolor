//! Allocation-free logical publications from prepared CPU reduction planes.

use std::mem::size_of;

use thiserror::Error;

use hypercolor_types::canvas::{linear_to_srgb_u8, srgb_u8_to_linear};

use super::{
    CapturePixelFormat, CaptureTransferFunction, PreparedScreenPublication,
    ResolvedScreenPublicationDescriptor, ScreenContentBarsPolicy, ScreenGridPolicy,
    ScreenLetterboxFill, ScreenPhysicalReductionDescriptor, ScreenPublicationKind,
    ScreenSmoothingPolicy, tune::PreparedLinearColorTuning,
};

const BYTES_PER_PIXEL: usize = 4;
const BYTES_PER_PIXEL_U64: u64 = 4;

#[derive(Clone, Debug)]
enum PreparedCpuZoneGrid {
    AreaWeighted {
        horizontal: PreparedAreaAxis,
        vertical: PreparedAreaAxis,
        normalization: f64,
    },
    PointSample {
        horizontal_offsets: Box<[usize]>,
        vertical_offsets: Box<[usize]>,
    },
}

impl PreparedCpuZoneGrid {
    fn byte_len(&self) -> Result<u64, CpuZoneMaterializationError> {
        match self {
            Self::AreaWeighted {
                horizontal,
                vertical,
                ..
            } => horizontal
                .byte_len()?
                .checked_add(vertical.byte_len()?)
                .ok_or(CpuZoneMaterializationError::GeometryOverflow),
            Self::PointSample {
                horizontal_offsets,
                vertical_offsets,
            } => checked_allocation_byte_len::<usize>(horizontal_offsets.len())?
                .checked_add(checked_allocation_byte_len::<usize>(
                    vertical_offsets.len(),
                )?)
                .ok_or(CpuZoneMaterializationError::GeometryOverflow),
        }
    }
}

#[derive(Clone, Debug)]
struct PreparedAreaAxis {
    spans: Box<[AreaSpan]>,
    samples: Box<[WeightedAxisSample]>,
}

impl PreparedAreaAxis {
    fn byte_len(&self) -> Result<u64, CpuZoneMaterializationError> {
        checked_allocation_byte_len::<AreaSpan>(self.spans.len())?
            .checked_add(checked_allocation_byte_len::<WeightedAxisSample>(
                self.samples.len(),
            )?)
            .ok_or(CpuZoneMaterializationError::GeometryOverflow)
    }

    fn samples(&self, span: AreaSpan) -> &[WeightedAxisSample] {
        &self.samples[span.start..span.end]
    }
}

#[derive(Clone, Copy, Debug)]
struct AreaSpan {
    start: usize,
    end: usize,
}

#[derive(Clone, Copy, Debug)]
struct WeightedAxisSample {
    byte_offset: usize,
    weight: u64,
}

/// Descriptor-complete, allocation-free CPU zone materialization.
#[derive(Clone, Debug)]
pub struct PreparedCpuZoneMaterializer {
    descriptor: ResolvedScreenPublicationDescriptor,
    physical_byte_len: usize,
    zone_count: usize,
    pixel_format: CapturePixelFormat,
    transfer: CaptureTransferFunction,
    grid: PreparedCpuZoneGrid,
    tuning: PreparedLinearColorTuning,
    precomputed_byte_len: u64,
}

impl PreparedCpuZoneMaterializer {
    /// Prepare one exact logical Zones branch without allocating frame storage.
    ///
    /// # Errors
    ///
    /// Rejects non-zone branches, policy stages requiring stateful or dynamic
    /// materialization, unsupported transfer functions, and shapes that cannot
    /// be addressed by this process.
    pub fn prepare(
        descriptor: &ResolvedScreenPublicationDescriptor,
    ) -> Result<Self, CpuZoneMaterializationError> {
        let ScreenPublicationKind::Zones { columns, rows } = descriptor.kind() else {
            return Err(CpuZoneMaterializationError::BranchNotZones);
        };
        let profile = descriptor.processing_profile();
        if profile.content_bars() != ScreenContentBarsPolicy::Disabled {
            return Err(CpuZoneMaterializationError::ContentBarsRequireDynamicMaterialization);
        }
        if profile.letterbox_fill() != ScreenLetterboxFill::default() {
            return Err(CpuZoneMaterializationError::LetterboxFillRequiresMaterialization);
        }
        if profile.smoothing() != ScreenSmoothingPolicy::Disabled {
            return Err(CpuZoneMaterializationError::SmoothingRequiresState);
        }
        let physical = descriptor.physical();
        let extent = physical.reduction_extent();
        let physical_byte_len = checked_physical_byte_len(extent.width(), extent.height())?;
        let row_byte_len = checked_physical_byte_len(extent.width(), 1)?;
        let zone_count = checked_zone_count(columns.get(), rows.get())?;
        let transfer = physical.color_pipeline().output().transfer_function();
        if !matches!(
            transfer,
            CaptureTransferFunction::Srgb | CaptureTransferFunction::Linear
        ) {
            return Err(CpuZoneMaterializationError::UnsupportedTransferFunction(
                transfer,
            ));
        }
        let grid = match profile.grid() {
            ScreenGridPolicy::AreaWeighted => PreparedCpuZoneGrid::AreaWeighted {
                horizontal: prepare_area_axis(extent.width(), columns.get(), BYTES_PER_PIXEL)?,
                vertical: prepare_area_axis(extent.height(), rows.get(), row_byte_len)?,
                normalization: 1.0
                    / weight_as_f64(u64::from(extent.width()), u64::from(extent.height())),
            },
            ScreenGridPolicy::PointSample => PreparedCpuZoneGrid::PointSample {
                horizontal_offsets: prepare_point_axis(
                    extent.width(),
                    columns.get(),
                    BYTES_PER_PIXEL,
                )?,
                vertical_offsets: prepare_point_axis(extent.height(), rows.get(), row_byte_len)?,
            },
        };
        let precomputed_byte_len = grid.byte_len()?;
        let tuning = profile.tuning();
        Ok(Self {
            descriptor: descriptor.clone(),
            physical_byte_len,
            zone_count,
            pixel_format: physical.target_pixel_format(),
            transfer,
            grid,
            tuning: PreparedLinearColorTuning::new(
                tuning.saturation(),
                tuning.brightness(),
                tuning.gamma(),
            ),
            precomputed_byte_len,
        })
    }

    /// Exact logical branch produced by this materializer.
    #[must_use]
    pub const fn descriptor(&self) -> &ResolvedScreenPublicationDescriptor {
        &self.descriptor
    }

    /// Complete shared physical key consumed by this materializer.
    #[must_use]
    pub const fn physical_descriptor(&self) -> &ScreenPhysicalReductionDescriptor {
        self.descriptor.physical()
    }

    /// Exact number of row-major zone colors written per publication.
    #[must_use]
    pub const fn zone_count(&self) -> usize {
        self.zone_count
    }

    /// Plan-lifetime bytes retained by precomputed sampling kernels.
    #[must_use]
    pub const fn precomputed_byte_len(&self) -> u64 {
        self.precomputed_byte_len
    }

    /// Materialize one shared physical plane directly into a writable zone slot.
    ///
    /// Validation completes before any destination color is changed. Execution
    /// performs no heap allocation and preserves the reservation for caller-side
    /// finalization.
    ///
    /// # Errors
    ///
    /// Rejects a substituted physical key, wrong physical byte length, another
    /// logical branch, or a reservation without exact writable zone storage.
    pub fn materialize(
        &self,
        physical_descriptor: &ScreenPhysicalReductionDescriptor,
        physical_pixels: &[u8],
        publication: &mut PreparedScreenPublication,
    ) -> Result<(), CpuZoneMaterializationError> {
        if physical_descriptor != self.physical_descriptor() {
            return Err(CpuZoneMaterializationError::PhysicalDescriptorMismatch);
        }
        if physical_pixels.len() != self.physical_byte_len {
            return Err(CpuZoneMaterializationError::PhysicalByteLengthMismatch {
                expected: self.physical_byte_len,
                actual: physical_pixels.len(),
            });
        }
        if publication.descriptor() != &self.descriptor {
            return Err(CpuZoneMaterializationError::PublicationDescriptorMismatch);
        }
        let output = publication
            .zone_colors_mut()
            .map_err(|_| CpuZoneMaterializationError::PublicationReservationUnavailable)?;
        if output.len() != self.zone_count {
            return Err(CpuZoneMaterializationError::ZoneCountMismatch {
                expected: self.zone_count,
                actual: output.len(),
            });
        }
        match &self.grid {
            PreparedCpuZoneGrid::AreaWeighted {
                horizontal,
                vertical,
                normalization,
            } => self.materialize_area(
                physical_pixels,
                output,
                horizontal,
                vertical,
                *normalization,
            ),
            PreparedCpuZoneGrid::PointSample {
                horizontal_offsets,
                vertical_offsets,
            } => self.materialize_points(
                physical_pixels,
                output,
                horizontal_offsets,
                vertical_offsets,
            ),
        }
        Ok(())
    }

    fn materialize_area(
        &self,
        pixels: &[u8],
        output: &mut [[u8; 3]],
        horizontal: &PreparedAreaAxis,
        vertical: &PreparedAreaAxis,
        normalization: f64,
    ) {
        let mut output = output.iter_mut();
        for &vertical_span in &vertical.spans {
            for &horizontal_span in &horizontal.spans {
                let mut sums = [0.0_f64; 3];
                for vertical_sample in vertical.samples(vertical_span) {
                    for horizontal_sample in horizontal.samples(horizontal_span) {
                        let weight =
                            weight_as_f64(horizontal_sample.weight, vertical_sample.weight);
                        let decoded = self.decode(self.read_rgb(
                            pixels,
                            vertical_sample.byte_offset + horizontal_sample.byte_offset,
                        ));
                        for channel in 0..3 {
                            sums[channel] += decoded[channel] * weight;
                        }
                    }
                }
                let mut color = sums.map(|sum| unit_f64_to_f32(sum * normalization));
                self.tuning.apply(&mut color);
                *output
                    .next()
                    .expect("prepared zone count matches row-major iteration") = self.encode(color);
            }
        }
    }

    fn materialize_points(
        &self,
        pixels: &[u8],
        output: &mut [[u8; 3]],
        horizontal_offsets: &[usize],
        vertical_offsets: &[usize],
    ) {
        let mut output = output.iter_mut();
        for &vertical_offset in vertical_offsets {
            for &horizontal_offset in horizontal_offsets {
                let mut color = self
                    .decode(self.read_rgb(pixels, vertical_offset + horizontal_offset))
                    .map(unit_f64_to_f32);
                self.tuning.apply(&mut color);
                *output
                    .next()
                    .expect("prepared zone count matches row-major iteration") = self.encode(color);
            }
        }
    }

    fn read_rgb(&self, pixels: &[u8], offset: usize) -> [u8; 3] {
        match self.pixel_format {
            CapturePixelFormat::Rgba8 => [pixels[offset], pixels[offset + 1], pixels[offset + 2]],
            CapturePixelFormat::Bgra8 => [pixels[offset + 2], pixels[offset + 1], pixels[offset]],
        }
    }

    fn decode(&self, color: [u8; 3]) -> [f64; 3] {
        color.map(|channel| match self.transfer {
            CaptureTransferFunction::Srgb => f64::from(srgb_u8_to_linear(channel)),
            CaptureTransferFunction::Linear => f64::from(channel) / 255.0,
            _ => unreachable!("unsupported transfers are rejected during preparation"),
        })
    }

    fn encode(&self, color: [f32; 3]) -> [u8; 3] {
        color.map(|channel| match self.transfer {
            CaptureTransferFunction::Srgb => linear_to_srgb_u8(channel.clamp(0.0, 1.0)),
            #[expect(
                clippy::cast_possible_truncation,
                clippy::as_conversions,
                reason = "the rounded channel is clamped to the u8 range"
            )]
            CaptureTransferFunction::Linear => (channel.clamp(0.0, 1.0) * 255.0).round() as u8,
            _ => unreachable!("unsupported transfers are rejected during preparation"),
        })
    }
}

fn checked_physical_byte_len(
    width: u32,
    height: u32,
) -> Result<usize, CpuZoneMaterializationError> {
    let byte_len = u64::from(width)
        .checked_mul(u64::from(height))
        .and_then(|pixels| pixels.checked_mul(BYTES_PER_PIXEL_U64))
        .ok_or(CpuZoneMaterializationError::GeometryOverflow)?;
    usize::try_from(byte_len)
        .map_err(|_| CpuZoneMaterializationError::PhysicalByteLengthNotAddressable { byte_len })
}

fn checked_zone_count(columns: u32, rows: u32) -> Result<usize, CpuZoneMaterializationError> {
    let count = u64::from(columns)
        .checked_mul(u64::from(rows))
        .ok_or(CpuZoneMaterializationError::GeometryOverflow)?;
    usize::try_from(count)
        .map_err(|_| CpuZoneMaterializationError::ZoneCountNotAddressable { count })
}

fn prepare_area_axis(
    source: u32,
    divisions: u32,
    byte_stride: usize,
) -> Result<PreparedAreaAxis, CpuZoneMaterializationError> {
    let division_count = usize::try_from(divisions).map_err(|_| {
        CpuZoneMaterializationError::ZoneCountNotAddressable {
            count: u64::from(divisions),
        }
    })?;
    let mut sample_count = 0_u64;
    for index in 0..divisions {
        let (start, end) = sample_range(index, source, divisions);
        sample_count = sample_count
            .checked_add(u64::from(end - start))
            .ok_or(CpuZoneMaterializationError::GeometryOverflow)?;
    }
    let sample_count = usize::try_from(sample_count)
        .map_err(|_| CpuZoneMaterializationError::SamplingKernelNotAddressable { sample_count })?;
    let allocation_byte_len = checked_allocation_byte_len::<AreaSpan>(division_count)?
        .checked_add(checked_allocation_byte_len::<WeightedAxisSample>(
            sample_count,
        )?)
        .ok_or(CpuZoneMaterializationError::GeometryOverflow)?;
    let mut spans = Vec::new();
    spans.try_reserve_exact(division_count).map_err(|_| {
        CpuZoneMaterializationError::SamplingKernelAllocationFailed {
            byte_len: allocation_byte_len,
        }
    })?;
    let mut samples = Vec::new();
    samples.try_reserve_exact(sample_count).map_err(|_| {
        CpuZoneMaterializationError::SamplingKernelAllocationFailed {
            byte_len: allocation_byte_len,
        }
    })?;
    for index in 0..divisions {
        let (start, end) = sample_range(index, source, divisions);
        let span_start = samples.len();
        for coordinate in start..end {
            let coordinate = usize::try_from(coordinate)
                .expect("prepared sample coordinate fits process address space");
            samples.push(WeightedAxisSample {
                byte_offset: coordinate
                    .checked_mul(byte_stride)
                    .ok_or(CpuZoneMaterializationError::GeometryOverflow)?,
                weight: axis_overlap(
                    index,
                    source,
                    divisions,
                    u32::try_from(coordinate)
                        .expect("prepared sample coordinate retains its u32 value"),
                ),
            });
        }
        spans.push(AreaSpan {
            start: span_start,
            end: samples.len(),
        });
    }
    Ok(PreparedAreaAxis {
        spans: spans.into_boxed_slice(),
        samples: samples.into_boxed_slice(),
    })
}

fn prepare_point_axis(
    source: u32,
    divisions: u32,
    byte_stride: usize,
) -> Result<Box<[usize]>, CpuZoneMaterializationError> {
    let division_count = usize::try_from(divisions).map_err(|_| {
        CpuZoneMaterializationError::ZoneCountNotAddressable {
            count: u64::from(divisions),
        }
    })?;
    let allocation_byte_len = checked_allocation_byte_len::<usize>(division_count)?;
    let mut offsets = Vec::new();
    offsets.try_reserve_exact(division_count).map_err(|_| {
        CpuZoneMaterializationError::SamplingKernelAllocationFailed {
            byte_len: allocation_byte_len,
        }
    })?;
    for index in 0..divisions {
        let coordinate = usize::try_from(point_sample(index, source, divisions))
            .expect("prepared point coordinate fits process address space");
        offsets.push(
            coordinate
                .checked_mul(byte_stride)
                .ok_or(CpuZoneMaterializationError::GeometryOverflow)?,
        );
    }
    Ok(offsets.into_boxed_slice())
}

fn checked_allocation_byte_len<T>(count: usize) -> Result<u64, CpuZoneMaterializationError> {
    u64::try_from(count)
        .ok()
        .and_then(|count| {
            u64::try_from(size_of::<T>())
                .ok()
                .and_then(|item_size| count.checked_mul(item_size))
        })
        .ok_or(CpuZoneMaterializationError::GeometryOverflow)
}

fn sample_range(index: u32, source: u32, divisions: u32) -> (u32, u32) {
    let low = u64::from(index) * u64::from(source);
    let high = (u64::from(index) + 1) * u64::from(source);
    let divisions = u64::from(divisions);
    let start = low / divisions;
    let end = high.div_ceil(divisions);
    (
        u32::try_from(start).expect("zone start is bounded by source extent"),
        u32::try_from(end).expect("zone end is bounded by source extent"),
    )
}

fn axis_overlap(zone: u32, source: u32, divisions: u32, pixel: u32) -> u64 {
    let zone_low = u64::from(zone) * u64::from(source);
    let zone_high = (u64::from(zone) + 1) * u64::from(source);
    let pixel_low = u64::from(pixel) * u64::from(divisions);
    let pixel_high = (u64::from(pixel) + 1) * u64::from(divisions);
    zone_high.min(pixel_high) - zone_low.max(pixel_low)
}

#[expect(
    clippy::cast_precision_loss,
    clippy::as_conversions,
    reason = "overlap products are bounded u64 weights normalized in the same domain"
)]
fn weight_as_f64(x_weight: u64, y_weight: u64) -> f64 {
    (x_weight * y_weight) as f64
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::as_conversions,
    reason = "normalized finite color channels are intentionally narrowed for canonical encoding"
)]
fn unit_f64_to_f32(value: f64) -> f32 {
    value as f32
}

fn point_sample(index: u32, source: u32, divisions: u32) -> u32 {
    let numerator = (u128::from(index) * 2 + 1) * u128::from(source);
    let denominator = u128::from(divisions) * 2;
    let sample = (numerator / denominator).min(u128::from(source - 1));
    u32::try_from(sample).expect("point sample is bounded by source extent")
}

/// Preparation or execution failure for exact CPU zone materialization.
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum CpuZoneMaterializationError {
    /// Only logical Zones branches use this materializer.
    #[error("CPU zone materialization requires a Zones descriptor")]
    BranchNotZones,
    /// Content-dependent crop detection requires a stateful branch processor.
    #[error("screen content-bar detection requires dynamic branch materialization")]
    ContentBarsRequireDynamicMaterialization,
    /// A noncanonical fill policy requires a logical surface processor.
    #[error("screen letterbox fill requires logical branch materialization")]
    LetterboxFillRequiresMaterialization,
    /// Temporal smoothing requires transactional per-branch history.
    #[error("screen smoothing requires stateful branch materialization")]
    SmoothingRequiresState,
    /// The zone path currently supports byte-addressable sRGB and linear output.
    #[error("unsupported CPU zone transfer function: {0:?}")]
    UnsupportedTransferFunction(CaptureTransferFunction),
    /// Packed raster or zone arithmetic exceeded the checked u64 ledger.
    #[error("CPU zone geometry exceeds the u64 resource ledger")]
    GeometryOverflow,
    /// The exact physical plane cannot be addressed by this process.
    #[error("physical plane byte length {byte_len} does not fit this process address space")]
    PhysicalByteLengthNotAddressable { byte_len: u64 },
    /// The exact zone grid cannot be addressed by this process.
    #[error("zone count {count} does not fit this process address space")]
    ZoneCountNotAddressable { count: u64 },
    /// A precomputed area kernel cannot be addressed by this process.
    #[error("sampling kernel with {sample_count} entries does not fit this process address space")]
    SamplingKernelNotAddressable { sample_count: u64 },
    /// Plan-lifetime sampling kernels could not reserve their exact storage.
    #[error("could not reserve {byte_len} bytes for the CPU zone sampling kernel")]
    SamplingKernelAllocationFailed { byte_len: u64 },
    /// Caller supplied bytes from another physical key.
    #[error("CPU zone materializer received another physical descriptor")]
    PhysicalDescriptorMismatch,
    /// Caller supplied an incorrectly sized physical plane.
    #[error("physical plane has {actual} bytes; expected exactly {expected}")]
    PhysicalByteLengthMismatch { expected: usize, actual: usize },
    /// Caller paired the materializer with another logical branch.
    #[error("CPU zone materializer received another publication descriptor")]
    PublicationDescriptorMismatch,
    /// A prepared zone slot no longer owns its admitted storage.
    #[error("CPU zone publication lost its writable reservation")]
    PublicationReservationUnavailable,
    /// Hub storage did not match its descriptor-owned zone shape.
    #[error("zone slot has {actual} colors; expected exactly {expected}")]
    ZoneCountMismatch { expected: usize, actual: usize },
}
