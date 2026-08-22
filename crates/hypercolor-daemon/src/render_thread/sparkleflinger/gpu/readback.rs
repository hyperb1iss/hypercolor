use anyhow::Result;
use hypercolor_core::types::canvas::{BYTES_PER_PIXEL, PublishedSurface, RenderSurfacePool};

pub(super) use super::source::CachedReadbackKey;
#[allow(unused_imports)]
pub(super) use super::source::{
    CachedReadbackAdjust, CachedReadbackLayer, CachedReadbackTransform,
};

#[derive(Debug, Clone)]
pub(super) struct CachedReadbackSurface {
    pub(super) key: Option<CachedReadbackKey>,
    pub(super) surface: PublishedSurface,
}

pub(super) fn copy_mapped_readback_buffer_into_surface(
    buffer: &wgpu::Buffer,
    used_bytes: u64,
    width: u32,
    height: u32,
    padded_bytes_per_row: u32,
    surfaces: &mut RenderSurfacePool,
    #[cfg(test)] last_readback_bytes: &mut u64,
) -> Result<PublishedSurface> {
    #[cfg(test)]
    {
        *last_readback_bytes = used_bytes;
    }
    let slice = buffer.slice(..used_bytes);
    let mapped = slice.get_mapped_range();
    let unpadded_bytes_per_row = width * BYTES_PER_PIXEL as u32;
    let Some(mut lease) = surfaces.dequeue() else {
        drop(mapped);
        buffer.unmap();
        anyhow::bail!("GPU readback surface pool should provide a reusable slot");
    };
    let target = lease.canvas_mut().as_rgba_bytes_mut();
    if padded_bytes_per_row == unpadded_bytes_per_row {
        target.copy_from_slice(
            &mapped[..usize::try_from(unpadded_bytes_per_row)
                .expect("row width should fit in usize")
                .saturating_mul(height as usize)],
        );
    } else {
        let row_width = usize::try_from(unpadded_bytes_per_row).expect("row width should fit");
        let padded_row_width =
            usize::try_from(padded_bytes_per_row).expect("row pitch should fit in usize");
        for (target_row, row) in target.chunks_exact_mut(row_width).zip(
            mapped
                .chunks(
                    usize::try_from(padded_bytes_per_row).expect("row pitch should fit in usize"),
                )
                .take(height as usize),
        ) {
            debug_assert_eq!(row.len(), padded_row_width);
            target_row.copy_from_slice(&row[..row_width]);
        }
    }
    drop(mapped);
    buffer.unmap();

    Ok(lease.submit(0, 0))
}
