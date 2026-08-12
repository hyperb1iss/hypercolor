use crate::{MacosCaptureError, MacosCaptureFrame, MacosCapturePixelFormat};

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
