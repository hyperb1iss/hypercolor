//! Sector grid — divides a captured frame into N x M rectangular sectors.
//!
//! Each sector holds the area-weighted average color of its pixel region.
//! The grid is the intermediate representation between raw RGBA pixels and
//! per-zone LED colors. Works on raw `&[u8]` RGBA buffers — no capture
//! backend dependency.

use crate::types::canvas::{Rgb, linear_to_srgb_u8, srgb_u8_to_linear};

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
        let cols = cols.max(1);
        let rows = rows.max(1);
        let total_sectors = (cols * rows) as usize;
        let stride = width * 4;

        let mut colors = Vec::with_capacity(total_sectors);

        // Guard: empty or undersized buffer → all-black grid.
        let expected_len = (height * stride) as usize;
        if frame.len() < expected_len || width == 0 || height == 0 {
            colors.resize(total_sectors, [0, 0, 0]);
            return Self { cols, rows, colors };
        }

        let sector_w = width / cols;
        let sector_h = height / rows;

        for r in 0..rows {
            let y_start = r * sector_h;
            let y_end = if r == rows - 1 {
                height
            } else {
                (r + 1) * sector_h
            };

            for c in 0..cols {
                let x_start = c * sector_w;
                let x_end = if c == cols - 1 {
                    width
                } else {
                    (c + 1) * sector_w
                };

                let (sum_r, sum_g, sum_b, count) =
                    accumulate_region(frame, stride, x_start, x_end, y_start, y_end);

                let n = count.max(1);
                colors.push([
                    linear_to_srgb_u8((sum_r / n as f32) / 255.0),
                    linear_to_srgb_u8((sum_g / n as f32) / 255.0),
                    linear_to_srgb_u8((sum_b / n as f32) / 255.0),
                ]);
            }
        }

        Self { cols, rows, colors }
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
    #[allow(clippy::as_conversions)]
    pub fn get(&self, col: u32, row: u32) -> [u8; 3] {
        if col >= self.cols || row >= self.rows {
            return [0, 0, 0];
        }
        let idx = (row * self.cols + col) as usize;
        self.colors.get(idx).copied().unwrap_or([0, 0, 0])
    }

    /// Get all sector colors as a slice.
    #[must_use]
    pub fn colors(&self) -> &[[u8; 3]] {
        &self.colors
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
    #[must_use]
    pub fn detect_letterbox(&self, black_threshold: f32) -> LetterboxBars {
        let top = self.count_black_rows_from_top(black_threshold);
        let bottom = self.count_black_rows_from_bottom(black_threshold);
        let left = self.count_black_cols_from_left(black_threshold);
        let right = self.count_black_cols_from_right(black_threshold);
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
    #[allow(clippy::as_conversions)]
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

        let mut colors = Vec::with_capacity((new_rows * new_cols) as usize);
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
    stride: u32,
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
        let row_offset = (y * stride) as usize;
        for x in x_start..x_end {
            let px = row_offset + (x * 4) as usize;
            // Bounds check — skip if pixel would read past the buffer.
            // We need at least 3 bytes (R, G, B) starting at `px`.
            if px + 3 > frame.len() {
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
    let r = srgb_u8_to_linear(c[0]);
    let g = srgb_u8_to_linear(c[1]);
    let b = srgb_u8_to_linear(c[2]);
    0.2126 * r + 0.7152 * g + 0.0722 * b
}

/// Convert `[u8; 3]` to the types crate `Rgb` for interop.
#[must_use]
pub fn to_rgb(c: [u8; 3]) -> Rgb {
    Rgb::new(c[0], c[1], c[2])
}
