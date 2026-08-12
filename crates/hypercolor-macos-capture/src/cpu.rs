use crate::{
    MacosCaptureColorimetry, MacosCaptureError, MacosCaptureFrame, MacosCapturePixelFormat,
    MacosCapturePlane, MacosChromaLocation, MacosColorRange, MacosPixelExtent, MacosYuvMatrix,
};

const RGBA_CHANNELS: usize = 4;

/// Borrowed, validated CPU view over one retained native capture frame.
///
/// RGB samples remain in the source transfer domain. `RGhA` therefore retains
/// extended-linear values above one, while YUV samples are matrix-converted to
/// transfer-encoded RGB before the shared color pipeline decodes them.
#[derive(Clone, Copy, Debug)]
pub struct MacosCpuSourceView<'frame> {
    extent: MacosPixelExtent,
    format: MacosCapturePixelFormat,
    color: MacosCaptureColorimetry,
    descriptors: &'frame [MacosCapturePlane],
    planes: &'frame [&'frame [u8]],
}

impl<'frame> MacosCpuSourceView<'frame> {
    /// Exact storage extent represented by this view.
    #[must_use]
    pub const fn extent(self) -> MacosPixelExtent {
        self.extent
    }

    /// Exact native format decoded by this view.
    #[must_use]
    pub const fn pixel_format(self) -> MacosCapturePixelFormat {
        self.format
    }

    /// Source color metadata whose transfer domain the returned RGB retains.
    #[must_use]
    pub const fn colorimetry(self) -> MacosCaptureColorimetry {
        self.color
    }

    /// Decode one native pixel into source-domain RGBA32Float.
    ///
    /// # Errors
    ///
    /// Rejects coordinates outside the validated storage extent and any
    /// addressing arithmetic that escapes a retained plane.
    pub fn sample_rgba32f(self, x: u32, y: u32) -> Result<[f32; 4], MacosCaptureError> {
        if x >= self.extent.width || y >= self.extent.height {
            return Err(MacosCaptureError::CpuPixelOutsideStorage {
                x,
                y,
                extent: self.extent,
            });
        }
        match self.format {
            MacosCapturePixelFormat::Bgra8 => self.sample_bgra8(x, y),
            MacosCapturePixelFormat::Argb2101010 => self.sample_argb2101010(x, y),
            MacosCapturePixelFormat::Rgba16Float => self.sample_rgba16_float(x, y),
            MacosCapturePixelFormat::Yuv420VideoRange
            | MacosCapturePixelFormat::Yuv420FullRange => self.sample_yuv420(x, y),
            MacosCapturePixelFormat::Yuv44410BiPlanar => self.sample_yuv44410(x, y),
        }
    }

    fn sample_bgra8(self, x: u32, y: u32) -> Result<[f32; 4], MacosCaptureError> {
        let pixel = self.packed_pixel(0, x, y, 4)?;
        Ok([
            normalize_u8(pixel[2]),
            normalize_u8(pixel[1]),
            normalize_u8(pixel[0]),
            normalize_u8(pixel[3]),
        ])
    }

    fn sample_argb2101010(self, x: u32, y: u32) -> Result<[f32; 4], MacosCaptureError> {
        let pixel = self.packed_pixel(0, x, y, 4)?;
        let packed = u32::from_le_bytes(
            pixel
                .try_into()
                .expect("validated packed pixel has exactly four bytes"),
        );
        Ok([
            ((packed >> 20) & 0x03ff) as f32 / 1_023.0,
            ((packed >> 10) & 0x03ff) as f32 / 1_023.0,
            (packed & 0x03ff) as f32 / 1_023.0,
            ((packed >> 30) & 0x0003) as f32 / 3.0,
        ])
    }

    fn sample_rgba16_float(self, x: u32, y: u32) -> Result<[f32; 4], MacosCaptureError> {
        let pixel = self.packed_pixel(0, x, y, 8)?;
        Ok([
            decode_f16(u16::from_le_bytes([pixel[0], pixel[1]])),
            decode_f16(u16::from_le_bytes([pixel[2], pixel[3]])),
            decode_f16(u16::from_le_bytes([pixel[4], pixel[5]])),
            decode_f16(u16::from_le_bytes([pixel[6], pixel[7]])),
        ])
    }

    fn sample_yuv420(self, x: u32, y: u32) -> Result<[f32; 4], MacosCaptureError> {
        let luma = f32::from(self.packed_pixel(0, x, y, 1)?[0]);
        let location = self
            .color
            .chroma_location
            .ok_or(MacosCaptureError::MissingYuvColorMetadata)?;
        let [cb, cr] = self.sample_chroma_420(x, y, location)?;
        let [luma, cb, cr] = match self.color.range {
            MacosColorRange::Video => [
                (luma - 16.0) / 219.0,
                (cb - 128.0) / 224.0,
                (cr - 128.0) / 224.0,
            ],
            MacosColorRange::Full => [luma / 255.0, (cb - 128.0) / 255.0, (cr - 128.0) / 255.0],
        };
        Ok(yuv_to_rgb(
            luma,
            cb,
            cr,
            self.color
                .matrix
                .ok_or(MacosCaptureError::MissingYuvColorMetadata)?,
        ))
    }

    fn sample_yuv44410(self, x: u32, y: u32) -> Result<[f32; 4], MacosCaptureError> {
        let luma = f32::from(read_msb_10(self.packed_pixel(0, x, y, 2)?));
        let chroma = self.packed_pixel(1, x, y, 4)?;
        let cb = f32::from(read_msb_10(&chroma[..2]));
        let cr = f32::from(read_msb_10(&chroma[2..]));
        let [luma, cb, cr] = match self.color.range {
            MacosColorRange::Video => [
                (luma - 64.0) / 876.0,
                (cb - 512.0) / 896.0,
                (cr - 512.0) / 896.0,
            ],
            MacosColorRange::Full => [
                luma / 1_023.0,
                (cb - 512.0) / 1_023.0,
                (cr - 512.0) / 1_023.0,
            ],
        };
        Ok(yuv_to_rgb(
            luma,
            cb,
            cr,
            self.color
                .matrix
                .ok_or(MacosCaptureError::MissingYuvColorMetadata)?,
        ))
    }

    fn sample_chroma_420(
        self,
        x: u32,
        y: u32,
        location: MacosChromaLocation,
    ) -> Result<[f32; 2], MacosCaptureError> {
        let (horizontal_offset, vertical_offset) = match location {
            MacosChromaLocation::Center => (1.0, 1.0),
            MacosChromaLocation::Left => (0.5, 1.0),
            MacosChromaLocation::TopLeft => (0.5, 0.5),
        };
        let chroma_x = (x as f32 + 0.5 - horizontal_offset) * 0.5;
        let chroma_y = (y as f32 + 0.5 - vertical_offset) * 0.5;
        self.bilinear_chroma_8(chroma_x, chroma_y)
    }

    fn bilinear_chroma_8(self, x: f32, y: f32) -> Result<[f32; 2], MacosCaptureError> {
        let extent = self.descriptors[1].extent;
        let maximum_x = extent.width.saturating_sub(1) as f32;
        let maximum_y = extent.height.saturating_sub(1) as f32;
        let x = x.clamp(0.0, maximum_x);
        let y = y.clamp(0.0, maximum_y);
        let x0 = x.floor() as u32;
        let y0 = y.floor() as u32;
        let x1 = (x0 + 1).min(extent.width - 1);
        let y1 = (y0 + 1).min(extent.height - 1);
        let x_weight = x - x0 as f32;
        let y_weight = y - y0 as f32;
        let top_left = self.chroma_8(x0, y0)?;
        let top_right = self.chroma_8(x1, y0)?;
        let bottom_left = self.chroma_8(x0, y1)?;
        let bottom_right = self.chroma_8(x1, y1)?;
        Ok(std::array::from_fn(|channel| {
            let top = top_left[channel] + (top_right[channel] - top_left[channel]) * x_weight;
            let bottom =
                bottom_left[channel] + (bottom_right[channel] - bottom_left[channel]) * x_weight;
            top + (bottom - top) * y_weight
        }))
    }

    fn chroma_8(self, x: u32, y: u32) -> Result<[f32; 2], MacosCaptureError> {
        let pixel = self.packed_pixel(1, x, y, 2)?;
        Ok([f32::from(pixel[0]), f32::from(pixel[1])])
    }

    fn packed_pixel(
        self,
        plane: usize,
        x: u32,
        y: u32,
        bytes_per_pixel: usize,
    ) -> Result<&'frame [u8], MacosCaptureError> {
        let descriptor = &self.descriptors[plane];
        let row = usize::try_from(y)
            .ok()
            .and_then(|y| y.checked_mul(descriptor.bytes_per_row))
            .ok_or(MacosCaptureError::ArithmeticOverflow)?;
        let offset = usize::try_from(x)
            .ok()
            .and_then(|x| x.checked_mul(bytes_per_pixel))
            .and_then(|x| row.checked_add(x))
            .ok_or(MacosCaptureError::ArithmeticOverflow)?;
        let end = offset
            .checked_add(bytes_per_pixel)
            .ok_or(MacosCaptureError::ArithmeticOverflow)?;
        self.planes[plane]
            .get(offset..end)
            .ok_or(MacosCaptureError::CpuPlaneLayoutMismatch)
    }
}

impl MacosCaptureFrame {
    /// Borrow a validated scalar decoding oracle while the retained pixel
    /// buffer remains CPU-locked.
    ///
    /// # Errors
    ///
    /// Rejects a descriptor whose plane extents, strides, lengths, allocation,
    /// or color metadata no longer match the delivered native frame.
    pub fn with_cpu_source<R>(
        &self,
        operation: impl for<'plane> FnOnce(MacosCpuSourceView<'plane>) -> R,
    ) -> Result<R, MacosCaptureError> {
        validate_cpu_source(self)?;
        let lengths = self
            .planes
            .iter()
            .map(|plane| plane.length_bytes)
            .collect::<Vec<_>>();
        self.surface.with_plane_bytes(&lengths, |planes| {
            operation(MacosCpuSourceView {
                extent: self.storage_extent,
                format: self.pixel_format,
                color: self.color,
                descriptors: &self.planes,
                planes,
            })
        })
    }

    /// Decode the retained native frame into tightly typed RGBA32Float bytes.
    ///
    /// Each component is written in little-endian IEEE-754 form. RGB remains
    /// in the source transfer domain for the shared color pipeline; alpha is
    /// normalized. Destination row padding is left untouched.
    pub fn copy_source_rgba32f_to(
        &self,
        destination: &mut [u8],
        destination_stride: usize,
    ) -> Result<(), MacosCaptureError> {
        let row_bytes = usize::try_from(self.storage_extent.width)
            .ok()
            .and_then(|width| width.checked_mul(RGBA_CHANNELS * size_of::<f32>()))
            .ok_or(MacosCaptureError::ArithmeticOverflow)?;
        let height = validate_destination(destination, destination_stride, row_bytes, self)?;
        self.with_cpu_source(|source| {
            for y in 0..height {
                let destination_start = y
                    .checked_mul(destination_stride)
                    .ok_or(MacosCaptureError::ArithmeticOverflow)?;
                let destination_end = destination_start
                    .checked_add(row_bytes)
                    .ok_or(MacosCaptureError::ArithmeticOverflow)?;
                let destination_length = destination.len();
                let row = destination
                    .get_mut(destination_start..destination_end)
                    .ok_or(MacosCaptureError::CpuDestinationTooSmall {
                        required: destination_end,
                        actual: destination_length,
                    })?;
                for (x, pixel) in row.chunks_exact_mut(16).enumerate() {
                    let rgba = source.sample_rgba32f(
                        u32::try_from(x).map_err(|_| MacosCaptureError::ArithmeticOverflow)?,
                        u32::try_from(y).map_err(|_| MacosCaptureError::ArithmeticOverflow)?,
                    )?;
                    for (channel, bytes) in rgba.into_iter().zip(pixel.chunks_exact_mut(4)) {
                        bytes.copy_from_slice(&channel.to_le_bytes());
                    }
                }
            }
            Ok(())
        })?
    }

    pub fn copy_bgra8_to(
        &self,
        destination: &mut [u8],
        destination_stride: usize,
    ) -> Result<(), MacosCaptureError> {
        if self.pixel_format != MacosCapturePixelFormat::Bgra8 {
            return Err(MacosCaptureError::UnsupportedCpuPixelFormat(
                self.pixel_format,
            ));
        }
        validate_cpu_source(self)?;
        let row_bytes = usize::try_from(self.storage_extent.width)
            .ok()
            .and_then(|width| width.checked_mul(4))
            .ok_or(MacosCaptureError::ArithmeticOverflow)?;
        let height = validate_destination(destination, destination_stride, row_bytes, self)?;
        let source = &self.planes[0];
        let lengths = [source.length_bytes];
        self.surface.with_plane_bytes(&lengths, |planes| {
            copy_rows(
                planes[0],
                source.bytes_per_row,
                destination,
                destination_stride,
                row_bytes,
                height,
            )
        })?
    }
}

fn validate_cpu_source(frame: &MacosCaptureFrame) -> Result<(), MacosCaptureError> {
    frame.color.validate_for(frame.pixel_format)?;
    let expected = frame.pixel_format.plane_layout(frame.storage_extent);
    if frame.planes.len() != expected.len() {
        return Err(MacosCaptureError::CpuPlaneLayoutMismatch);
    }
    let mut allocation = 0_u64;
    for (position, (plane, (extent, bytes_per_pixel))) in
        frame.planes.iter().zip(expected).enumerate()
    {
        if usize::try_from(plane.index).ok() != Some(position) || plane.extent != extent {
            return Err(MacosCaptureError::CpuPlaneLayoutMismatch);
        }
        let minimum_stride = u64::from(extent.width)
            .checked_mul(bytes_per_pixel)
            .ok_or(MacosCaptureError::ArithmeticOverflow)?;
        let stride = u64::try_from(plane.bytes_per_row)
            .map_err(|_| MacosCaptureError::ArithmeticOverflow)?;
        let minimum_length = stride
            .checked_mul(u64::from(extent.height))
            .ok_or(MacosCaptureError::ArithmeticOverflow)?;
        if stride < minimum_stride || plane.length_bytes < minimum_length {
            return Err(MacosCaptureError::CpuPlaneLayoutMismatch);
        }
        allocation = allocation
            .checked_add(plane.length_bytes)
            .ok_or(MacosCaptureError::ArithmeticOverflow)?;
    }
    if allocation > frame.surface.allocation_bytes {
        return Err(MacosCaptureError::AllocationTooSmall {
            required: allocation,
            actual: frame.surface.allocation_bytes,
        });
    }
    Ok(())
}

fn validate_destination(
    destination: &[u8],
    destination_stride: usize,
    row_bytes: usize,
    frame: &MacosCaptureFrame,
) -> Result<usize, MacosCaptureError> {
    if destination_stride < row_bytes {
        return Err(MacosCaptureError::InvalidCpuDestinationStride {
            minimum: row_bytes,
            actual: destination_stride,
        });
    }
    let height = usize::try_from(frame.storage_extent.height)
        .map_err(|_| MacosCaptureError::ArithmeticOverflow)?;
    let required = destination_stride
        .checked_mul(height)
        .ok_or(MacosCaptureError::ArithmeticOverflow)?;
    if destination.len() < required {
        return Err(MacosCaptureError::CpuDestinationTooSmall {
            required,
            actual: destination.len(),
        });
    }
    Ok(height)
}

fn copy_rows(
    source: &[u8],
    source_stride: usize,
    destination: &mut [u8],
    destination_stride: usize,
    row_bytes: usize,
    height: usize,
) -> Result<(), MacosCaptureError> {
    for row in 0..height {
        let source_start = row
            .checked_mul(source_stride)
            .ok_or(MacosCaptureError::ArithmeticOverflow)?;
        let source_end = source_start
            .checked_add(row_bytes)
            .ok_or(MacosCaptureError::ArithmeticOverflow)?;
        let destination_start = row
            .checked_mul(destination_stride)
            .ok_or(MacosCaptureError::ArithmeticOverflow)?;
        let destination_end = destination_start
            .checked_add(row_bytes)
            .ok_or(MacosCaptureError::ArithmeticOverflow)?;
        let source_row = source
            .get(source_start..source_end)
            .ok_or(MacosCaptureError::CpuPlaneLayoutMismatch)?;
        let destination_length = destination.len();
        let destination_row = destination
            .get_mut(destination_start..destination_end)
            .ok_or(MacosCaptureError::CpuDestinationTooSmall {
                required: destination_end,
                actual: destination_length,
            })?;
        destination_row.copy_from_slice(source_row);
    }
    Ok(())
}

fn normalize_u8(value: u8) -> f32 {
    f32::from(value) / 255.0
}

fn read_msb_10(bytes: &[u8]) -> u16 {
    u16::from_le_bytes([bytes[0], bytes[1]]) >> 6
}

fn yuv_to_rgb(luma: f32, cb: f32, cr: f32, matrix: MacosYuvMatrix) -> [f32; 4] {
    let (red_luma, blue_luma) = match matrix {
        MacosYuvMatrix::Bt601 => (0.299, 0.114),
        MacosYuvMatrix::Bt709 => (0.2126, 0.0722),
        MacosYuvMatrix::Bt2020 => (0.2627, 0.0593),
    };
    let green_luma = 1.0 - red_luma - blue_luma;
    [
        luma + 2.0 * (1.0 - red_luma) * cr,
        luma - 2.0 * blue_luma * (1.0 - blue_luma) / green_luma * cb
            - 2.0 * red_luma * (1.0 - red_luma) / green_luma * cr,
        luma + 2.0 * (1.0 - blue_luma) * cb,
        1.0,
    ]
}

fn decode_f16(bits: u16) -> f32 {
    let sign = u32::from(bits & 0x8000) << 16;
    let exponent = u32::from((bits >> 10) & 0x001f);
    let fraction = u32::from(bits & 0x03ff);
    if exponent == 0 {
        if fraction == 0 {
            return f32::from_bits(sign);
        }
        let magnitude = fraction as f32 * 2.0_f32.powi(-24);
        return if sign == 0 { magnitude } else { -magnitude };
    }
    let decoded = match exponent {
        0x1f => sign | 0x7f80_0000 | (fraction << 13),
        _ => sign | ((exponent + 112) << 23) | (fraction << 13),
    };
    f32::from_bits(decoded)
}
