use hypercolor_core::input::ScreenData;
use hypercolor_core::types::canvas::Canvas;

pub(crate) fn screen_data_to_canvas(
    screen_data: &ScreenData,
    canvas_width: u32,
    canvas_height: u32,
    sector_grid: &mut Vec<[u8; 3]>,
) -> Option<Canvas> {
    if let Some(surface) = &screen_data.canvas_downscale
        && surface.width() == canvas_width
        && surface.height() == canvas_height
    {
        return Some(Canvas::from_published_surface(surface));
    }

    if canvas_width == 0 || canvas_height == 0 {
        return None;
    }

    let mut max_row = 0_u32;
    let mut max_col = 0_u32;
    let mut saw_sector = false;

    for zone in &screen_data.zone_colors {
        let Some((row, col)) = parse_sector_zone_id(&zone.zone_id) else {
            continue;
        };
        let _color = zone.colors.first().copied().unwrap_or([0, 0, 0]);
        max_row = max_row.max(row);
        max_col = max_col.max(col);
        saw_sector = true;
    }

    if !saw_sector {
        return None;
    }

    let rows = max_row.saturating_add(1);
    let cols = max_col.saturating_add(1);
    let cell_count = usize::try_from(rows).ok().and_then(|row_count| {
        usize::try_from(cols)
            .ok()
            .and_then(|col_count| row_count.checked_mul(col_count))
    })?;

    if sector_grid.len() == cell_count {
        sector_grid.fill([0, 0, 0]);
    } else {
        sector_grid.resize(cell_count, [0, 0, 0]);
    }
    for zone in &screen_data.zone_colors {
        let Some((row, col)) = parse_sector_zone_id(&zone.zone_id) else {
            continue;
        };
        let color = zone.colors.first().copied().unwrap_or([0, 0, 0]);
        let idx_u64 = u64::from(row)
            .checked_mul(u64::from(cols))
            .and_then(|base| base.checked_add(u64::from(col)))?;
        let idx = usize::try_from(idx_u64).ok()?;
        if let Some(cell) = sector_grid.get_mut(idx) {
            *cell = color;
        }
    }

    let mut canvas = Canvas::new(canvas_width, canvas_height);
    let pixels = canvas.as_rgba_bytes_mut();
    let width_u64 = u64::from(canvas_width);
    let height_u64 = u64::from(canvas_height);
    let grid_cols_u64 = u64::from(cols);
    let grid_rows_u64 = u64::from(rows);
    let canvas_width_usize = usize::try_from(canvas_width).ok()?;

    for y in 0..canvas_height {
        let mapped_row_u64 = (u64::from(y) * grid_rows_u64) / height_u64;
        let row = u32::try_from(mapped_row_u64)
            .unwrap_or_default()
            .min(rows.saturating_sub(1));
        let row_offset = usize::try_from(y)
            .ok()?
            .checked_mul(canvas_width_usize)?
            .checked_mul(4)?;

        for x in 0..canvas_width {
            let mapped_col_u64 = (u64::from(x) * grid_cols_u64) / width_u64;
            let col = u32::try_from(mapped_col_u64)
                .unwrap_or_default()
                .min(cols.saturating_sub(1));

            let idx_u64 = u64::from(row)
                .checked_mul(grid_cols_u64)
                .and_then(|base| base.checked_add(u64::from(col)))
                .unwrap_or_default();
            let idx = usize::try_from(idx_u64).unwrap_or_default();
            let [r, g, b] = sector_grid.get(idx).copied().unwrap_or([0, 0, 0]);
            let pixel_offset = row_offset.checked_add(usize::try_from(x).ok()?.checked_mul(4)?)?;
            pixels[pixel_offset] = r;
            pixels[pixel_offset + 1] = g;
            pixels[pixel_offset + 2] = b;
            pixels[pixel_offset + 3] = 255;
        }
    }

    Some(canvas)
}

pub(crate) fn parse_sector_zone_id(zone_id: &str) -> Option<(u32, u32)> {
    let coords = zone_id.strip_prefix("screen:sector_")?;
    let (row_raw, col_raw) = coords.split_once('_')?;
    let row = row_raw.parse().ok()?;
    let col = col_raw.parse().ok()?;
    Some((row, col))
}
