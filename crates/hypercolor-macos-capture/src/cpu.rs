use crate::{
    MacosCaptureError, MacosCaptureFrame, MacosCapturePixelFormat, MacosColorPrimaries,
    MacosTransferFunction,
};

impl MacosCaptureFrame {
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
        let row_bytes = usize::try_from(self.storage_extent.width)
            .ok()
            .and_then(|width| width.checked_mul(4))
            .ok_or(MacosCaptureError::ArithmeticOverflow)?;
        if destination_stride < row_bytes {
            return Err(MacosCaptureError::InvalidCpuDestinationStride {
                minimum: row_bytes,
                actual: destination_stride,
            });
        }
        let height = usize::try_from(self.storage_extent.height)
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
        let source = self
            .planes
            .first()
            .ok_or(MacosCaptureError::CpuPlaneLayoutMismatch)?;
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

    pub fn convert_bgra8_sdr_to_rgba8(
        &self,
        destination: &mut [u8],
        destination_stride: usize,
    ) -> Result<(), MacosCaptureError> {
        if self.pixel_format != MacosCapturePixelFormat::Bgra8 {
            return Err(MacosCaptureError::UnsupportedCpuPixelFormat(
                self.pixel_format,
            ));
        }
        if matches!(
            self.color.transfer,
            MacosTransferFunction::Pq | MacosTransferFunction::Hlg
        ) {
            return Err(MacosCaptureError::UnsupportedCpuTransferFunction(
                self.color.transfer,
            ));
        }
        let row_bytes = usize::try_from(self.storage_extent.width)
            .ok()
            .and_then(|width| width.checked_mul(4))
            .ok_or(MacosCaptureError::ArithmeticOverflow)?;
        let height = validate_destination(destination, destination_stride, row_bytes, self)?;
        let source = self
            .planes
            .first()
            .ok_or(MacosCaptureError::CpuPlaneLayoutMismatch)?;
        let lengths = [source.length_bytes];
        self.surface.with_plane_bytes(&lengths, |planes| {
            convert_bgra_rows(
                planes[0],
                source.bytes_per_row,
                destination,
                destination_stride,
                row_bytes,
                height,
                self.color.primaries,
                self.color.transfer,
            )
        })?
    }
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

#[allow(clippy::too_many_arguments)]
fn convert_bgra_rows(
    source: &[u8],
    source_stride: usize,
    destination: &mut [u8],
    destination_stride: usize,
    row_bytes: usize,
    height: usize,
    primaries: MacosColorPrimaries,
    transfer: MacosTransferFunction,
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
        let destination_row = destination
            .get_mut(destination_start..destination_end)
            .ok_or(MacosCaptureError::CpuPlaneLayoutMismatch)?;
        for (source_pixel, destination_pixel) in source_row
            .chunks_exact(4)
            .zip(destination_row.chunks_exact_mut(4))
        {
            let linear = [
                decode(source_pixel[2], transfer),
                decode(source_pixel[1], transfer),
                decode(source_pixel[0], transfer),
            ];
            let linear = compress_gamut(convert_primaries(linear, primaries));
            destination_pixel[0] = encode_srgb(linear[0]);
            destination_pixel[1] = encode_srgb(linear[1]);
            destination_pixel[2] = encode_srgb(linear[2]);
            destination_pixel[3] = source_pixel[3];
        }
    }
    Ok(())
}

fn decode(value: u8, transfer: MacosTransferFunction) -> f32 {
    let value = f32::from(value) / 255.0;
    match transfer {
        MacosTransferFunction::Srgb => {
            if value <= 0.040_45 {
                value / 12.92
            } else {
                ((value + 0.055) / 1.055).powf(2.4)
            }
        }
        MacosTransferFunction::Rec709 => decode_bt(value, 1.099, 0.018),
        MacosTransferFunction::Rec2020 => decode_bt(value, 1.099_296_8, 0.018_053_97),
        MacosTransferFunction::Linear => value,
        MacosTransferFunction::Pq | MacosTransferFunction::Hlg => unreachable!(),
    }
}

fn decode_bt(value: f32, alpha: f32, beta: f32) -> f32 {
    let encoded_cut = 4.5 * beta;
    if value < encoded_cut {
        value / 4.5
    } else {
        ((value + alpha - 1.0) / alpha).powf(1.0 / 0.45)
    }
}

fn convert_primaries(rgb: [f32; 3], primaries: MacosColorPrimaries) -> [f32; 3] {
    let matrix: [[f32; 3]; 3] = match primaries {
        MacosColorPrimaries::Srgb => return rgb,
        MacosColorPrimaries::DisplayP3 => [
            [1.224_745, -0.224_904, 0.0],
            [-0.042_058, 1.042_081, 0.0],
            [-0.019_642, -0.078_655, 1.098_537],
        ],
        MacosColorPrimaries::Rec2020 => [
            [1.660_491, -0.587_641, -0.072_85],
            [-0.124_55, 1.132_9, -0.008_349],
            [-0.018_151, -0.100_579, 1.118_73],
        ],
    };
    matrix.map(|row| row[0].mul_add(rgb[0], row[1].mul_add(rgb[1], row[2] * rgb[2])))
}

fn compress_gamut(mut rgb: [f32; 3]) -> [f32; 3] {
    let minimum = rgb.into_iter().reduce(f32::min).unwrap_or(0.0);
    if minimum < 0.0 {
        for channel in &mut rgb {
            *channel -= minimum;
        }
    }
    let maximum = rgb.into_iter().reduce(f32::max).unwrap_or(1.0);
    if maximum > 1.0 {
        for channel in &mut rgb {
            *channel /= maximum;
        }
    }
    rgb
}

fn encode_srgb(value: f32) -> u8 {
    let encoded = if value <= 0.003_130_8 {
        12.92 * value
    } else {
        1.055 * value.powf(1.0 / 2.4) - 0.055
    };
    (encoded.clamp(0.0, 1.0) * 255.0).round() as u8
}
