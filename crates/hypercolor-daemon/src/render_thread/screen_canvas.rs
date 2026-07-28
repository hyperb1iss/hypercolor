use std::sync::Arc;

use anyhow::{Context, Result};
use hypercolor_core::bus::ScreenZonesFrame;
use hypercolor_core::input::ScreenData;
use hypercolor_core::types::canvas::{PublishedSurface, RenderSurfacePool, SurfaceDescriptor};

use super::RenderThreadState;

pub(crate) fn screen_data_to_surface(
    screen_data: &ScreenData,
    canvas_width: u32,
    canvas_height: u32,
    sector_grid: &mut Vec<[u8; 3]>,
    surface_pool: &mut RenderSurfacePool,
) -> Result<Option<PublishedSurface>> {
    if let Some(surface) = &screen_data.canvas_downscale {
        return Ok(Some(surface.clone()));
    }

    if canvas_width == 0 || canvas_height == 0 {
        return Ok(None);
    }

    let rows = screen_data.grid_height;
    let cols = screen_data.grid_width;
    if rows == 0 || cols == 0 {
        return Ok(None);
    }
    let Some(row_count) = usize::try_from(rows).ok() else {
        return Ok(None);
    };
    let Some(col_count) = usize::try_from(cols).ok() else {
        return Ok(None);
    };
    let Some(cell_count) = row_count.checked_mul(col_count) else {
        return Ok(None);
    };

    if screen_data.zone_colors.len() != cell_count {
        return Ok(None);
    }
    sector_grid.clear();
    if sector_grid.capacity() < cell_count {
        sector_grid
            .try_reserve(cell_count - sector_grid.capacity())
            .context("screen sector-grid allocation failed")?;
    }
    for zone in &screen_data.zone_colors {
        let Some(color) = zone.colors.first() else {
            return Ok(None);
        };
        sector_grid.push(*color);
    }

    let descriptor = SurfaceDescriptor::rgba8888(canvas_width, canvas_height);
    descriptor.try_non_empty_byte_len()?;
    let Some(canvas_width_usize) = usize::try_from(canvas_width).ok() else {
        return Ok(None);
    };
    if surface_pool.descriptor() != descriptor {
        *surface_pool = RenderSurfacePool::try_with_lazy_slot_count(descriptor, 2)?;
    }

    let Some(mut lease) = surface_pool.try_dequeue()? else {
        return Ok(None);
    };
    let pixels = lease.canvas_mut().as_rgba_bytes_mut();
    let width_u64 = u64::from(canvas_width);
    let height_u64 = u64::from(canvas_height);
    let grid_cols_u64 = u64::from(cols);
    let grid_rows_u64 = u64::from(rows);

    for y in 0..canvas_height {
        let mapped_row_u64 = (u64::from(y) * grid_rows_u64) / height_u64;
        let row = usize::try_from(mapped_row_u64)
            .expect("grid row originated from u32 dimensions")
            .min(row_count.saturating_sub(1));
        let row_offset = usize::try_from(y).expect("canvas row originated from u32 dimensions")
            * canvas_width_usize
            * 4;

        for x in 0..canvas_width {
            let mapped_col_u64 = (u64::from(x) * grid_cols_u64) / width_u64;
            let col = usize::try_from(mapped_col_u64)
                .expect("grid column originated from u32 dimensions")
                .min(col_count.saturating_sub(1));

            let idx = row * col_count + col;
            let [r, g, b] = sector_grid
                .get(idx)
                .copied()
                .expect("validated sector grid covers every mapped cell");
            let pixel_offset = row_offset
                + usize::try_from(x).expect("canvas column originated from u32 dimensions") * 4;
            pixels[pixel_offset] = r;
            pixels[pixel_offset + 1] = g;
            pixels[pixel_offset + 2] = b;
            pixels[pixel_offset + 3] = 255;
        }
    }

    Ok(Some(lease.submit(0, 0)))
}

/// Publish the latest ambilight zone grid to the bus when anyone is watching.
///
/// Redundant frames (identical zone content) are skipped so relays only wake
/// when colors actually change.
pub(crate) fn publish_screen_zones(
    state: &RenderThreadState,
    screen_data: Option<&ScreenData>,
    frame_number: u32,
    timestamp_ms: u32,
) {
    if state.event_bus.screen_zones_receiver_count() == 0 {
        return;
    }

    let frame = screen_data
        .and_then(|data| screen_zones_frame(data, frame_number, timestamp_ms))
        .unwrap_or_default();

    state
        .event_bus
        .screen_zones_sender()
        .send_if_modified(|current| {
            if current.same_content(&frame) {
                false
            } else {
                *current = frame;
                true
            }
        });
}

fn screen_zones_frame(
    screen_data: &ScreenData,
    frame_number: u32,
    timestamp_ms: u32,
) -> Option<ScreenZonesFrame> {
    let rows = screen_data.grid_height;
    let cols = screen_data.grid_width;
    if rows == 0 || cols == 0 {
        return None;
    }
    let cell_count = usize::try_from(rows)
        .ok()
        .and_then(|row_count| usize::try_from(cols).ok()?.checked_mul(row_count))?;

    if screen_data.zone_colors.len() != cell_count {
        return None;
    }
    let mut colors = Vec::with_capacity(cell_count);
    for zone in &screen_data.zone_colors {
        colors.push(*zone.colors.first()?);
    }

    Some(ScreenZonesFrame {
        frame_number,
        timestamp_ms,
        source_width: screen_data.source_width,
        source_height: screen_data.source_height,
        grid_cols: cols,
        grid_rows: rows,
        letterbox: screen_data.letterbox,
        colors: Arc::new(colors),
    })
}
