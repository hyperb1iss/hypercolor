use super::{SectorGrid, accumulate_region, prepare_sector_chunk_plan};
use crate::input::screen::{FrameRegion, LetterboxBars};
use hypercolor_types::canvas::linear_to_srgb_u8;

fn patterned_rgba(width: u32, height: u32) -> Vec<u8> {
    let mut frame = Vec::with_capacity(
        usize::try_from(width * height * 4).expect("small test frame length fits usize"),
    );
    for y in 0..height {
        for x in 0..width {
            frame.extend_from_slice(&[
                u8::try_from((x * 13 + y * 7) % 256).expect("pattern channel fits u8"),
                u8::try_from((x * 3 + y * 19) % 256).expect("pattern channel fits u8"),
                u8::try_from((x * 29 + y * 5) % 256).expect("pattern channel fits u8"),
                255,
            ]);
        }
    }
    frame
}

fn scalar_colors(frame: &[u8], width: u32, height: u32, cols: u32, rows: u32) -> Vec<[u8; 3]> {
    let stride = usize::try_from(width * 4).expect("small test stride fits usize");
    let mut colors = Vec::new();
    for row in 0..rows {
        let y_start = row * height / rows;
        let y_end = ((row + 1) * height / rows).max(y_start + 1).min(height);
        for column in 0..cols {
            let x_start = column * width / cols;
            let x_end = ((column + 1) * width / cols).max(x_start + 1).min(width);
            let (red, green, blue, count) =
                accumulate_region(frame, stride, x_start, x_end, y_start, y_end);
            #[expect(
                clippy::cast_precision_loss,
                clippy::as_conversions,
                reason = "small test pixel counts are exactly representable"
            )]
            let count = count.max(1) as f32;
            colors.push([
                linear_to_srgb_u8((red / count) / 255.0),
                linear_to_srgb_u8((green / count) / 255.0),
                linear_to_srgb_u8((blue / count) / 255.0),
            ]);
        }
    }
    colors
}

#[test]
fn wide_and_tall_grids_schedule_multiple_balanced_chunks() {
    for total_cells in [4_096, 8_192] {
        let plan = prepare_sector_chunk_plan(total_cells, 4).expect("valid grid is scheduled");
        assert!(plan.scheduled_chunks > 1);
        assert!(plan.scheduled_chunks <= 16);
        assert!(plan.cells_per_chunk.abs_diff(total_cells.div_ceil(16)) <= 1);
    }
}

#[test]
fn wide_and_tall_parallel_grids_equal_scalar_reduction() {
    let width = 257;
    let height = 193;
    let frame = patterned_rgba(width, height);
    for (cols, rows) in [(4_096, 1), (1, 4_096)] {
        let parallel = SectorGrid::try_compute(&frame, width, height, cols, rows)
            .expect("parallel grid computes");
        assert_eq!(
            parallel.colors(),
            scalar_colors(&frame, width, height, cols, rows)
        );
    }
}

#[test]
fn grids_larger_than_the_source_sample_a_nonempty_pixel_per_sector() {
    let frame = vec![80, 120, 160, 255, 80, 120, 160, 255];
    let grid = SectorGrid::try_compute(&frame, 2, 1, 8, 3).expect("oversized grid computes");

    assert_eq!(grid.sector_count(), 24);
    assert!(grid.colors().iter().all(|color| *color != [0, 0, 0]));
}

#[test]
fn letterbox_regions_share_proportional_sector_boundaries() {
    let odd = FrameRegion::from_letterbox(
        11,
        7,
        3,
        3,
        LetterboxBars {
            top: 1,
            bottom: 0,
            left: 1,
            right: 0,
        },
    )
    .expect("odd proportional crop is non-empty");
    assert_eq!((odd.x, odd.y, odd.width, odd.height), (3, 2, 8, 5));

    let oversized = FrameRegion::from_letterbox(
        2,
        1,
        8,
        3,
        LetterboxBars {
            top: 1,
            bottom: 0,
            left: 4,
            right: 0,
        },
    )
    .expect("grid larger than source still resolves a physical crop");
    assert_eq!(
        (oversized.x, oversized.y, oversized.width, oversized.height),
        (1, 0, 1, 1)
    );
}
