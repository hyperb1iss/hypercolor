//! Sector grid — divides a captured frame into N x M rectangular sectors.
//!
//! Each sector holds the area-weighted average color of its pixel region.
//! The grid is the intermediate representation between raw RGBA pixels and
//! per-zone LED colors. Works on raw `&[u8]` RGBA buffers — no capture
//! backend dependency.

use hypercolor_color::LinearRgba;

use crate::types::canvas::{Rgb, linear_to_srgb_u8, srgb_u8_to_linear};
use rayon::prelude::*;
use thiserror::Error;

use super::CaptureTransferFunction;

#[cfg(test)]
mod tests;

const SECTOR_TASKS_PER_WORKER: usize = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SectorChunkPlan {
    cells_per_chunk: usize,
    scheduled_chunks: usize,
}

fn prepare_sector_chunk_plan(total_cells: usize, worker_count: usize) -> Option<SectorChunkPlan> {
    if total_cells == 0 || worker_count == 0 {
        return None;
    }
    let target_chunks = worker_count
        .checked_mul(SECTOR_TASKS_PER_WORKER)?
        .min(total_cells);
    let cells_per_chunk = total_cells.div_ceil(target_chunks);
    Some(SectorChunkPlan {
        cells_per_chunk,
        scheduled_chunks: total_cells.div_ceil(cells_per_chunk),
    })
}

/// Preallocated row/column luminance scratch for dynamic content-bar detection.
#[derive(Clone, Debug)]
pub struct PreparedLetterboxDetector {
    columns: u32,
    rows: u32,
    row_luminance: Vec<f32>,
    column_luminance: Vec<f32>,
    retained_byte_len: u64,
}

impl PreparedLetterboxDetector {
    /// Reserve the exact scratch required for one maximum grid shape.
    ///
    /// # Errors
    ///
    /// Returns a typed failure for empty or unaddressable geometry and failed
    /// plan-lifetime allocation. No fixed resolution or zone-count cap applies.
    pub fn try_new(columns: u32, rows: u32) -> Result<Self, LetterboxDetectionError> {
        if columns == 0 || rows == 0 {
            return Err(LetterboxDetectionError::EmptyGrid { columns, rows });
        }
        let column_count = usize::try_from(columns)
            .map_err(|_| LetterboxDetectionError::GeometryOverflow { columns, rows })?;
        let row_count = usize::try_from(rows)
            .map_err(|_| LetterboxDetectionError::GeometryOverflow { columns, rows })?;
        let scratch_count = column_count
            .checked_add(row_count)
            .ok_or(LetterboxDetectionError::GeometryOverflow { columns, rows })?;
        let requested_byte_len = u64::try_from(scratch_count)
            .ok()
            .and_then(|count| {
                u64::try_from(std::mem::size_of::<f32>())
                    .ok()
                    .and_then(|item_size| count.checked_mul(item_size))
            })
            .ok_or(LetterboxDetectionError::GeometryOverflow { columns, rows })?;
        let requested_byte_len_usize = usize::try_from(requested_byte_len)
            .map_err(|_| LetterboxDetectionError::GeometryOverflow { columns, rows })?;
        let mut row_luminance = Vec::new();
        row_luminance.try_reserve_exact(row_count).map_err(|_| {
            LetterboxDetectionError::AllocationFailed {
                columns,
                rows,
                byte_len: requested_byte_len_usize,
            }
        })?;
        row_luminance.resize(row_count, 0.0);
        let mut column_luminance = Vec::new();
        column_luminance
            .try_reserve_exact(column_count)
            .map_err(|_| LetterboxDetectionError::AllocationFailed {
                columns,
                rows,
                byte_len: requested_byte_len_usize,
            })?;
        column_luminance.resize(column_count, 0.0);
        let retained_byte_len = u64::try_from(
            row_luminance
                .capacity()
                .checked_add(column_luminance.capacity())
                .ok_or(LetterboxDetectionError::GeometryOverflow { columns, rows })?,
        )
        .ok()
        .and_then(|count| {
            u64::try_from(std::mem::size_of::<f32>())
                .ok()
                .and_then(|item_size| count.checked_mul(item_size))
        })
        .ok_or(LetterboxDetectionError::GeometryOverflow { columns, rows })?;
        Ok(Self {
            columns,
            rows,
            row_luminance,
            column_luminance,
            retained_byte_len,
        })
    }

    /// Exact shape accepted by this detector.
    #[must_use]
    pub const fn shape(&self) -> (u32, u32) {
        (self.columns, self.rows)
    }

    /// Exact retained heap-byte ledger for row and column scratch.
    #[must_use]
    pub const fn retained_byte_len(&self) -> u64 {
        self.retained_byte_len
    }

    /// Reserved element capacities, useful for proving frame-time reuse.
    #[must_use]
    pub fn capacities(&self) -> (usize, usize) {
        (
            self.row_luminance.capacity(),
            self.column_luminance.capacity(),
        )
    }

    /// Detect edge bars from one exact encoded-color grid.
    ///
    /// Scratch is cleared and reused in place. Luminance is evaluated in linear
    /// light for both sRGB and linear encoded publications.
    ///
    /// # Errors
    ///
    /// Rejects a mismatched grid or unsupported transfer function.
    pub fn detect(
        &mut self,
        colors: &[[u8; 3]],
        transfer: CaptureTransferFunction,
        black_threshold: f32,
    ) -> Result<LetterboxBars, LetterboxDetectionError> {
        let expected = usize::try_from(self.columns)
            .ok()
            .and_then(|columns| {
                usize::try_from(self.rows)
                    .ok()
                    .and_then(|rows| columns.checked_mul(rows))
            })
            .ok_or(LetterboxDetectionError::GeometryOverflow {
                columns: self.columns,
                rows: self.rows,
            })?;
        if colors.len() != expected {
            return Err(LetterboxDetectionError::ColorCountMismatch {
                expected,
                actual: colors.len(),
            });
        }
        if !matches!(
            transfer,
            CaptureTransferFunction::Srgb | CaptureTransferFunction::Linear
        ) {
            return Err(LetterboxDetectionError::UnsupportedTransferFunction(
                transfer,
            ));
        }

        self.row_luminance.fill(0.0);
        self.column_luminance.fill(0.0);
        let columns = usize::try_from(self.columns)
            .expect("prepared column count fits the process address space");
        for (index, color) in colors.iter().copied().enumerate() {
            let row = index / columns;
            let column = index % columns;
            let luminance = linear_luminance(color, transfer);
            self.row_luminance[row] += luminance;
            self.column_luminance[column] += luminance;
        }
        let row_normalization = 1.0 / self.columns as f32;
        let column_normalization = 1.0 / self.rows as f32;
        for luminance in &mut self.row_luminance {
            *luminance *= row_normalization;
        }
        for luminance in &mut self.column_luminance {
            *luminance *= column_normalization;
        }

        let threshold = black_threshold.clamp(0.0, 1.0);
        let mut top = count_dark_prefix(&self.row_luminance, threshold);
        let mut bottom = count_dark_suffix(&self.row_luminance, threshold);
        let mut left = count_dark_prefix(&self.column_luminance, threshold);
        let mut right = count_dark_suffix(&self.column_luminance, threshold);
        if top.saturating_add(bottom) >= self.rows {
            top = 0;
            bottom = 0;
        }
        if left.saturating_add(right) >= self.columns {
            left = 0;
            right = 0;
        }
        Ok(LetterboxBars {
            top,
            bottom,
            left,
            right,
        })
    }
}

/// Preparation or frame validation failure for dynamic content-bar detection.
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum LetterboxDetectionError {
    /// Both grid axes must be non-zero.
    #[error("letterbox detector requires a non-empty grid, got {columns}x{rows}")]
    EmptyGrid { columns: u32, rows: u32 },
    /// Checked scratch or grid arithmetic overflowed.
    #[error("letterbox detector geometry overflows for {columns}x{rows}")]
    GeometryOverflow { columns: u32, rows: u32 },
    /// Exact scratch storage could not be reserved.
    #[error("could not reserve {byte_len} bytes for a {columns}x{rows} detector")]
    AllocationFailed {
        columns: u32,
        rows: u32,
        byte_len: usize,
    },
    /// Caller supplied a slice different from the prepared shape.
    #[error("letterbox grid has {actual} colors; expected exactly {expected}")]
    ColorCountMismatch { expected: usize, actual: usize },
    /// Only byte-addressable sRGB and linear output are supported.
    #[error("unsupported letterbox detection transfer function: {0:?}")]
    UnsupportedTransferFunction(CaptureTransferFunction),
}

fn count_dark_prefix(values: &[f32], threshold: f32) -> u32 {
    u32::try_from(
        values
            .iter()
            .take_while(|value| **value < threshold)
            .count(),
    )
    .expect("prepared scratch length originates from u32 geometry")
}

fn count_dark_suffix(values: &[f32], threshold: f32) -> u32 {
    u32::try_from(
        values
            .iter()
            .rev()
            .take_while(|value| **value < threshold)
            .count(),
    )
    .expect("prepared scratch length originates from u32 geometry")
}

fn linear_luminance(color: [u8; 3], transfer: CaptureTransferFunction) -> f32 {
    let decode = |channel| match transfer {
        CaptureTransferFunction::Srgb => srgb_u8_to_linear(channel),
        CaptureTransferFunction::Linear => f32::from(channel) / 255.0,
        _ => unreachable!("unsupported transfers are rejected before decoding"),
    };
    LinearRgba::new(decode(color[0]), decode(color[1]), decode(color[2]), 1.0).luma()
}

// ── SectorGrid ────────────────────────────────────────────────────────────

/// A grid of sectors overlaid on a captured frame.
///
/// Each sector's color is the average of all pixels in its rectangular region.
/// Stored row-major: index = `row * cols + col`.
#[derive(Debug, Clone)]
pub struct SectorGrid {
    /// Number of columns (horizontal divisions).
    cols: u32,
    /// Number of rows (vertical divisions).
    rows: u32,
    /// Flat array of sector colors, row-major. Length: `cols * rows`.
    colors: Vec<[u8; 3]>,
}

impl SectorGrid {
    /// Compute sector colors from an RGBA8 frame buffer.
    ///
    /// The buffer must contain `width * height * 4` bytes in row-major RGBA order.
    /// Each sector covers a rectangular block of pixels; the last column and row
    /// absorb any remainder pixels when dimensions aren't evenly divisible.
    ///
    /// # Arguments
    ///
    /// * `frame` — Raw RGBA8 pixel data, row-major, 4 bytes per pixel.
    /// * `width` — Frame width in pixels.
    /// * `height` — Frame height in pixels.
    /// * `cols` — Number of horizontal grid divisions.
    /// * `rows` — Number of vertical grid divisions.
    ///
    /// # Panics
    ///
    /// Does not panic. Returns a 1x1 grid if `cols` or `rows` is zero.
    #[must_use]
    #[allow(clippy::cast_possible_truncation, clippy::as_conversions)]
    pub fn compute(frame: &[u8], width: u32, height: u32, cols: u32, rows: u32) -> Self {
        Self::try_compute(frame, width, height, cols, rows).unwrap_or_else(|| {
            let cols = cols.max(1);
            let rows = rows.max(1);
            let Some(total_sectors) = usize::try_from(cols)
                .ok()
                .and_then(|cols| usize::try_from(rows).ok()?.checked_mul(cols))
            else {
                return Self::black(1, 1, 1);
            };
            Self::black(cols, rows, total_sectors)
        })
    }

    /// Compute a sector grid only when all frame and grid arithmetic is valid.
    #[must_use]
    pub fn try_compute(
        frame: &[u8],
        width: u32,
        height: u32,
        cols: u32,
        rows: u32,
    ) -> Option<Self> {
        let cols = cols.max(1);
        let rows = rows.max(1);
        let mut grid = Self::try_with_capacity(cols, rows)?;
        grid.try_update(frame, width, height, cols, rows)
            .then_some(grid)
    }

    /// Reserve reusable storage for at most `cols * rows` sectors.
    #[must_use]
    pub fn try_with_capacity(cols: u32, rows: u32) -> Option<Self> {
        let cols = cols.max(1);
        let rows = rows.max(1);
        let total_sectors = checked_sector_count(cols, rows)?;
        let mut colors = Vec::new();
        colors.try_reserve_exact(total_sectors).ok()?;
        Some(Self { cols, rows, colors })
    }

    /// Recompute a grid in place without growing beyond prepared capacity.
    pub fn try_update(
        &mut self,
        frame: &[u8],
        width: u32,
        height: u32,
        cols: u32,
        rows: u32,
    ) -> bool {
        let cols = cols.max(1);
        let rows = rows.max(1);
        let Some(total_sectors) = checked_sector_count(cols, rows) else {
            return false;
        };
        let Some(stride) = usize::try_from(width)
            .ok()
            .and_then(|width| width.checked_mul(4))
        else {
            return false;
        };
        let Some(expected_len) = usize::try_from(height)
            .ok()
            .and_then(|height| height.checked_mul(stride))
        else {
            return false;
        };
        if width == 0
            || height == 0
            || frame.len() < expected_len
            || self.colors.capacity() < total_sectors
        {
            return false;
        }

        self.cols = cols;
        self.rows = rows;
        self.colors.resize(total_sectors, [0, 0, 0]);
        let Some(chunk_plan) =
            prepare_sector_chunk_plan(total_sectors, rayon::current_num_threads())
        else {
            return false;
        };
        let cols_usize = usize::try_from(cols).expect("validated sector columns fit usize");

        self.colors
            .par_chunks_mut(chunk_plan.cells_per_chunk)
            .enumerate()
            .for_each(|(chunk_index, chunk)| {
                let first_cell = chunk_index * chunk_plan.cells_per_chunk;
                for (chunk_offset, color) in chunk.iter_mut().enumerate() {
                    let cell_index = first_cell + chunk_offset;
                    let r = u32::try_from(cell_index / cols_usize)
                        .expect("sector row is bounded by a u32 grid extent");
                    let c = u32::try_from(cell_index % cols_usize)
                        .expect("sector column is bounded by a u32 grid extent");
                    let (y_start, y_end) = proportional_bounds(r, rows, height);
                    let (x_start, x_end) = proportional_bounds(c, cols, width);
                    let (sum_r, sum_g, sum_b, count) =
                        accumulate_region(frame, stride, x_start, x_end, y_start, y_end);

                    #[expect(
                        clippy::cast_precision_loss,
                        clippy::as_conversions,
                        reason = "pixel count is always safely representable as f32"
                    )]
                    let n_f = count.max(1) as f32;
                    *color = [
                        linear_to_srgb_u8((sum_r / n_f) / 255.0),
                        linear_to_srgb_u8((sum_g / n_f) / 255.0),
                        linear_to_srgb_u8((sum_b / n_f) / 255.0),
                    ];
                }
            });

        true
    }

    /// Number of columns in the grid.
    #[must_use]
    pub const fn cols(&self) -> u32 {
        self.cols
    }

    /// Number of rows in the grid.
    #[must_use]
    pub const fn rows(&self) -> u32 {
        self.rows
    }

    /// Total number of sectors (`cols * rows`).
    #[must_use]
    pub fn sector_count(&self) -> usize {
        self.colors.len()
    }

    /// Look up the color of sector `(col, row)`.
    ///
    /// Returns black if coordinates are out of bounds.
    #[must_use]
    pub fn get(&self, col: u32, row: u32) -> [u8; 3] {
        if col >= self.cols || row >= self.rows {
            return [0, 0, 0];
        }
        let Some(idx) = usize::try_from(row)
            .ok()
            .and_then(|row| {
                usize::try_from(self.cols)
                    .ok()
                    .and_then(|cols| row.checked_mul(cols))
            })
            .and_then(|offset| {
                usize::try_from(col)
                    .ok()
                    .and_then(|col| offset.checked_add(col))
            })
        else {
            return [0, 0, 0];
        };
        self.colors.get(idx).copied().unwrap_or([0, 0, 0])
    }

    /// Get all sector colors as a slice.
    #[must_use]
    pub fn colors(&self) -> &[[u8; 3]] {
        &self.colors
    }

    #[must_use]
    pub fn color_capacity(&self) -> usize {
        self.colors.capacity()
    }

    /// Map sector grid to zone IDs, producing one `(zone_id, [r, g, b])` per sector.
    ///
    /// Zone IDs follow the pattern `"screen:sector_{row}_{col}"`.
    #[must_use]
    pub fn to_zone_colors(&self) -> Vec<(String, [u8; 3])> {
        let mut result = Vec::with_capacity(self.colors.len());
        for r in 0..self.rows {
            for c in 0..self.cols {
                let color = self.get(c, r);
                result.push((format!("screen:sector_{r}_{c}"), color));
            }
        }
        result
    }

    /// Detect letterbox bars by scanning for rows/columns where the average
    /// luminance falls below `black_threshold` (0.0 - 1.0).
    ///
    /// Returns `(top_rows, bottom_rows, left_cols, right_cols)` — the number
    /// of consecutive black rows/columns from each edge.
    ///
    /// An axis whose bars would swallow the whole grid reports no bars at
    /// all. `pixel_luminance` is *linear*, where dark content sits far lower
    /// than its sRGB value suggests — sRGB 30/255 is only 0.013 linear — so a
    /// dark desktop or a night-time scene can read as black from every edge
    /// at once. Cropping on that removes the entire picture, and because the
    /// verdict flips with ordinary content changes it strobes frame to frame.
    /// Real letterboxing always leaves something in the middle.
    #[must_use]
    pub fn detect_letterbox(&self, black_threshold: f32) -> LetterboxBars {
        let mut top = self.count_black_rows_from_top(black_threshold);
        let mut bottom = self.count_black_rows_from_bottom(black_threshold);
        let mut left = self.count_black_cols_from_left(black_threshold);
        let mut right = self.count_black_cols_from_right(black_threshold);

        if top.saturating_add(bottom) >= self.rows {
            top = 0;
            bottom = 0;
        }
        if left.saturating_add(right) >= self.cols {
            left = 0;
            right = 0;
        }

        LetterboxBars {
            top,
            bottom,
            left,
            right,
        }
    }

    /// Build a new grid excluding the letterbox bars.
    ///
    /// Returns `None` if bars consume the entire grid (degenerate case).
    #[must_use]
    pub fn crop_letterbox(&self, bars: &LetterboxBars) -> Option<Self> {
        let top = bars.top.min(self.rows);
        let bottom = bars.bottom.min(self.rows.saturating_sub(top));
        let left = bars.left.min(self.cols);
        let right = bars.right.min(self.cols.saturating_sub(left));

        let new_rows = self.rows.saturating_sub(top + bottom);
        let new_cols = self.cols.saturating_sub(left + right);

        if new_rows == 0 || new_cols == 0 {
            return None;
        }

        let capacity = usize::try_from(new_rows)
            .ok()?
            .checked_mul(usize::try_from(new_cols).ok()?)?;
        let mut colors = Vec::new();
        colors.try_reserve_exact(capacity).ok()?;
        for r in top..(self.rows - bottom) {
            for c in left..(self.cols - right) {
                colors.push(self.get(c, r));
            }
        }

        Some(Self {
            cols: new_cols,
            rows: new_rows,
            colors,
        })
    }

    // ── Internal helpers ──────────────────────────────────────────────────

    #[allow(clippy::cast_precision_loss, clippy::as_conversions)]
    fn row_avg_luminance(&self, row: u32) -> f32 {
        if self.cols == 0 {
            return 0.0;
        }
        let sum: f32 = (0..self.cols)
            .map(|col| {
                let c = self.get(col, row);
                pixel_luminance(c)
            })
            .sum();
        sum / self.cols as f32
    }

    #[allow(clippy::cast_precision_loss, clippy::as_conversions)]
    fn col_avg_luminance(&self, col: u32) -> f32 {
        if self.rows == 0 {
            return 0.0;
        }
        let sum: f32 = (0..self.rows)
            .map(|row| {
                let c = self.get(col, row);
                pixel_luminance(c)
            })
            .sum();
        sum / self.rows as f32
    }

    fn count_black_rows_from_top(&self, threshold: f32) -> u32 {
        let mut count = 0;
        for row in 0..self.rows {
            if self.row_avg_luminance(row) < threshold {
                count += 1;
            } else {
                break;
            }
        }
        count
    }

    fn count_black_rows_from_bottom(&self, threshold: f32) -> u32 {
        let mut count = 0;
        for row in (0..self.rows).rev() {
            if self.row_avg_luminance(row) < threshold {
                count += 1;
            } else {
                break;
            }
        }
        count
    }

    fn count_black_cols_from_left(&self, threshold: f32) -> u32 {
        let mut count = 0;
        for col in 0..self.cols {
            if self.col_avg_luminance(col) < threshold {
                count += 1;
            } else {
                break;
            }
        }
        count
    }

    fn count_black_cols_from_right(&self, threshold: f32) -> u32 {
        let mut count = 0;
        for col in (0..self.cols).rev() {
            if self.col_avg_luminance(col) < threshold {
                count += 1;
            } else {
                break;
            }
        }
        count
    }

    fn black(cols: u32, rows: u32, total_sectors: usize) -> Self {
        let mut colors = Vec::new();
        if colors.try_reserve_exact(total_sectors).is_err() {
            return Self {
                cols: 1,
                rows: 1,
                colors: vec![[0, 0, 0]],
            };
        }
        colors.resize(total_sectors, [0, 0, 0]);
        Self { cols, rows, colors }
    }
}

fn checked_sector_count(cols: u32, rows: u32) -> Option<usize> {
    usize::try_from(cols)
        .ok()?
        .checked_mul(usize::try_from(rows).ok()?)
}

#[must_use]
pub fn proportional_sector_bounds(index: u32, extent: u32, divisions: u32) -> Option<(u32, u32)> {
    if extent == 0 || divisions == 0 || index >= divisions {
        return None;
    }
    Some(proportional_bounds(index, divisions, extent))
}

pub(super) fn proportional_bounds(index: u32, divisions: u32, extent: u32) -> (u32, u32) {
    let start = u64::from(index) * u64::from(extent) / u64::from(divisions);
    let proportional_end = (u64::from(index) + 1) * u64::from(extent) / u64::from(divisions);
    let end = proportional_end.max(start + 1).min(u64::from(extent));
    (
        u32::try_from(start).expect("proportional sector start fits the source extent"),
        u32::try_from(end).expect("proportional sector end fits the source extent"),
    )
}

// ── LetterboxBars ─────────────────────────────────────────────────────────

/// Detected black bars at each edge, measured in grid rows/columns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LetterboxBars {
    /// Consecutive black rows from the top edge.
    pub top: u32,
    /// Consecutive black rows from the bottom edge.
    pub bottom: u32,
    /// Consecutive black columns from the left edge.
    pub left: u32,
    /// Consecutive black columns from the right edge.
    pub right: u32,
}

impl LetterboxBars {
    /// Whether any bars were detected.
    #[must_use]
    pub fn has_bars(&self) -> bool {
        self.top > 0 || self.bottom > 0 || self.left > 0 || self.right > 0
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────

/// Accumulate R, G, B sums and pixel count for a rectangular region.
#[allow(clippy::as_conversions)]
fn accumulate_region(
    frame: &[u8],
    stride: usize,
    x_start: u32,
    x_end: u32,
    y_start: u32,
    y_end: u32,
) -> (f32, f32, f32, u64) {
    let mut sum_r = 0.0_f32;
    let mut sum_g = 0.0_f32;
    let mut sum_b = 0.0_f32;
    let mut count: u64 = 0;

    for y in y_start..y_end {
        let Some(row_offset) = usize::try_from(y).ok().and_then(|y| y.checked_mul(stride)) else {
            continue;
        };
        for x in x_start..x_end {
            let Some(px) = usize::try_from(x)
                .ok()
                .and_then(|x| x.checked_mul(4))
                .and_then(|x| row_offset.checked_add(x))
            else {
                continue;
            };
            // Bounds check — skip if pixel would read past the buffer.
            // We need at least 3 bytes (R, G, B) starting at `px`.
            if px.checked_add(3).is_none_or(|end| end > frame.len()) {
                continue;
            }
            sum_r += srgb_u8_to_linear(frame[px]) * 255.0;
            sum_g += srgb_u8_to_linear(frame[px + 1]) * 255.0;
            sum_b += srgb_u8_to_linear(frame[px + 2]) * 255.0;
            count += 1;
        }
    }

    (sum_r, sum_g, sum_b, count)
}

/// Relative luminance of an RGB pixel (BT.709 coefficients), 0.0 - 1.0.
fn pixel_luminance(c: [u8; 3]) -> f32 {
    LinearRgba::new(
        srgb_u8_to_linear(c[0]),
        srgb_u8_to_linear(c[1]),
        srgb_u8_to_linear(c[2]),
        1.0,
    )
    .luma()
}

/// Convert `[u8; 3]` to the types crate `Rgb` for interop.
#[must_use]
pub fn to_rgb(c: [u8; 3]) -> Rgb {
    Rgb::new(c[0], c[1], c[2])
}
