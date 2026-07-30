//! Tests for the screen capture input pipeline.
//!
//! All tests use synthetic RGBA pixel buffers — no actual screen capture needed.

use std::time::Duration;

use hypercolor_core::input::screen::sector::{
    LetterboxBars, SectorGrid, proportional_sector_bounds,
};
use hypercolor_core::input::screen::smooth::TemporalSmoother;
use hypercolor_core::input::screen::{
    CaptureConfig, ColorTuning, MAX_REPRESENTABLE_CAPTURE_FPS, PixelExtent,
    ScreenAnalysisResourcePlan, ScreenCaptureInput,
};
use hypercolor_core::input::{InputData, InputSource};
use hypercolor_types::canvas::{DEFAULT_CANVAS_HEIGHT, DEFAULT_CANVAS_WIDTH};

// ── Helpers ───────────────────────────────────────────────────────────────

/// Create a solid-color RGBA frame buffer.
#[allow(clippy::as_conversions)]
fn solid_frame(width: u32, height: u32, r: u8, g: u8, b: u8) -> Vec<u8> {
    let pixel_count = (width * height) as usize;
    let mut buf = Vec::with_capacity(pixel_count * 4);
    for _ in 0..pixel_count {
        buf.push(r);
        buf.push(g);
        buf.push(b);
        buf.push(255);
    }
    buf
}

/// Create a frame where the left half is one color and the right half another.
fn half_split_frame(width: u32, height: u32, left: [u8; 3], right: [u8; 3]) -> Vec<u8> {
    #[allow(clippy::as_conversions)]
    let pixel_count = (width * height) as usize;
    let half_w = width / 2;
    let mut buf = Vec::with_capacity(pixel_count * 4);
    for y in 0..height {
        for x in 0..width {
            let _ = y;
            let color = if x < half_w { left } else { right };
            buf.push(color[0]);
            buf.push(color[1]);
            buf.push(color[2]);
            buf.push(255);
        }
    }
    buf
}

/// Create a horizontal gradient frame from `left_color` to `right_color`.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::as_conversions,
    clippy::cast_precision_loss
)]
fn gradient_frame(width: u32, height: u32, left_color: [u8; 3], right_color: [u8; 3]) -> Vec<u8> {
    let pixel_count = (width * height) as usize;
    let mut buf = Vec::with_capacity(pixel_count * 4);
    for _y in 0..height {
        for x in 0..width {
            let t = if width <= 1 {
                0.0
            } else {
                x as f32 / (width - 1) as f32
            };
            let r = (f32::from(left_color[0]) * (1.0 - t) + f32::from(right_color[0]) * t) as u8;
            let g = (f32::from(left_color[1]) * (1.0 - t) + f32::from(right_color[1]) * t) as u8;
            let b = (f32::from(left_color[2]) * (1.0 - t) + f32::from(right_color[2]) * t) as u8;
            buf.push(r);
            buf.push(g);
            buf.push(b);
            buf.push(255);
        }
    }
    buf
}

/// Create a frame with black bars at top and bottom (letterbox).
#[allow(clippy::as_conversions)]
fn letterbox_frame(width: u32, height: u32, bar_rows: u32, content_color: [u8; 3]) -> Vec<u8> {
    let pixel_count = (width * height) as usize;
    let mut buf = Vec::with_capacity(pixel_count * 4);
    for y in 0..height {
        for _x in 0..width {
            let is_bar = y < bar_rows || y >= height - bar_rows;
            let color = if is_bar { [0, 0, 0] } else { content_color };
            buf.push(color[0]);
            buf.push(color[1]);
            buf.push(color[2]);
            buf.push(255);
        }
    }
    buf
}

// ── SectorGrid: Solid Color ──────────────────────────────────────────────

#[test]
fn sector_grid_solid_red_all_sectors_red() {
    let frame = solid_frame(80, 60, 255, 0, 0);
    let grid = SectorGrid::compute(&frame, 80, 60, 8, 6);

    assert_eq!(grid.cols(), 8);
    assert_eq!(grid.rows(), 6);
    assert_eq!(grid.sector_count(), 48);

    for r in 0..6 {
        for c in 0..8 {
            let color = grid.get(c, r);
            assert_eq!(color, [255, 0, 0], "sector ({c}, {r}) should be pure red");
        }
    }
}

#[test]
fn sector_grid_solid_green() {
    let frame = solid_frame(40, 40, 0, 255, 0);
    let grid = SectorGrid::compute(&frame, 40, 40, 4, 4);

    for r in 0..4 {
        for c in 0..4 {
            assert_eq!(grid.get(c, r), [0, 255, 0]);
        }
    }
}

// ── SectorGrid: Half Split ───────────────────────────────────────────────

#[test]
fn sector_grid_half_red_half_blue() {
    // 80px wide, 2 columns → left 40px red, right 40px blue.
    let frame = half_split_frame(80, 60, [255, 0, 0], [0, 0, 255]);
    let grid = SectorGrid::compute(&frame, 80, 60, 2, 1);

    assert_eq!(grid.get(0, 0), [255, 0, 0], "left sector should be red");
    assert_eq!(grid.get(1, 0), [0, 0, 255], "right sector should be blue");
}

#[test]
fn sector_grid_half_split_multi_row() {
    let frame = half_split_frame(80, 60, [255, 0, 0], [0, 0, 255]);
    let grid = SectorGrid::compute(&frame, 80, 60, 2, 3);

    // Every row should show the same split.
    for r in 0..3 {
        assert_eq!(grid.get(0, r), [255, 0, 0], "row {r} left should be red");
        assert_eq!(grid.get(1, r), [0, 0, 255], "row {r} right should be blue");
    }
}

// ── SectorGrid: Gradient ─────────────────────────────────────────────────

#[test]
fn sector_grid_gradient_approximates_values() {
    let frame = gradient_frame(100, 10, [0, 0, 0], [200, 200, 200]);
    let grid = SectorGrid::compute(&frame, 100, 10, 4, 1);

    // With 4 columns over a 0..200 gradient, approximate expected averages:
    // col 0: pixels 0..24  → ~avg around 24
    // col 1: pixels 25..49 → ~avg around 74
    // col 2: pixels 50..74 → ~avg around 124
    // col 3: pixels 75..99 → ~avg around 170
    let c0 = grid.get(0, 0);
    let c1 = grid.get(1, 0);
    let c2 = grid.get(2, 0);
    let c3 = grid.get(3, 0);

    // Sectors should be monotonically increasing.
    assert!(c0[0] < c1[0], "gradient should increase left-to-right");
    assert!(c1[0] < c2[0], "gradient should increase left-to-right");
    assert!(c2[0] < c3[0], "gradient should increase left-to-right");

    // First sector should be dark, last should be bright.
    assert!(c0[0] < 60, "first sector should be dark, got {}", c0[0]);
    assert!(c3[0] > 140, "last sector should be bright, got {}", c3[0]);
}

// ── SectorGrid: Different Dimensions ─────────────────────────────────────

#[test]
fn sector_grid_2x2() {
    let frame = solid_frame(20, 20, 100, 150, 200);
    let grid = SectorGrid::compute(&frame, 20, 20, 2, 2);
    assert_eq!(grid.sector_count(), 4);
    for r in 0..2 {
        for c in 0..2 {
            assert_eq!(grid.get(c, r), [100, 150, 200]);
        }
    }
}

#[test]
fn sector_grid_16x9() {
    let frame = solid_frame(160, 90, 42, 84, 126);
    let grid = SectorGrid::compute(&frame, 160, 90, 16, 9);
    assert_eq!(grid.sector_count(), 144);
    assert_eq!(grid.get(0, 0), [42, 84, 126]);
    assert_eq!(grid.get(15, 8), [42, 84, 126]);
}

#[test]
fn sector_grid_1x1() {
    let frame = solid_frame(100, 100, 200, 100, 50);
    let grid = SectorGrid::compute(&frame, 100, 100, 1, 1);
    assert_eq!(grid.sector_count(), 1);
    assert_eq!(grid.get(0, 0), [200, 100, 50]);
}

#[test]
fn sector_grid_single_pixel_frame() {
    let frame = vec![42u8, 128, 200, 255];
    let grid = SectorGrid::compute(&frame, 1, 1, 1, 1);
    assert_eq!(grid.sector_count(), 1);
    assert_eq!(grid.get(0, 0), [42, 128, 200]);
}

// ── SectorGrid: All-Black Frame ──────────────────────────────────────────

#[test]
fn sector_grid_all_black_frame() {
    let frame = solid_frame(80, 60, 0, 0, 0);
    let grid = SectorGrid::compute(&frame, 80, 60, 4, 3);
    for r in 0..3 {
        for c in 0..4 {
            assert_eq!(grid.get(c, r), [0, 0, 0]);
        }
    }
}

// ── SectorGrid: Out-of-Bounds Access ─────────────────────────────────────

#[test]
fn sector_grid_out_of_bounds_returns_black() {
    let frame = solid_frame(40, 40, 255, 255, 255);
    let grid = SectorGrid::compute(&frame, 40, 40, 4, 4);

    assert_eq!(grid.get(10, 0), [0, 0, 0]);
    assert_eq!(grid.get(0, 10), [0, 0, 0]);
    assert_eq!(grid.get(100, 100), [0, 0, 0]);
}

// ── Letterbox Detection ──────────────────────────────────────────────────

#[test]
fn letterbox_detection_black_bars_top_bottom() {
    // 80x60 frame, top 10px and bottom 10px are black, middle is white.
    let frame = letterbox_frame(80, 60, 10, [255, 255, 255]);
    let grid = SectorGrid::compute(&frame, 80, 60, 8, 6);

    // With 6 rows, each row is 10px. Top row = black, bottom row = black.
    let bars = grid.detect_letterbox(0.05);
    assert!(bars.top >= 1, "should detect top bar, got {}", bars.top);
    assert!(
        bars.bottom >= 1,
        "should detect bottom bar, got {}",
        bars.bottom
    );
    assert_eq!(bars.left, 0, "no left bar expected");
    assert_eq!(bars.right, 0, "no right bar expected");
    assert!(bars.has_bars());
}

#[test]
fn letterbox_detection_no_bars_on_full_color_frame() {
    let frame = solid_frame(80, 60, 128, 128, 128);
    let grid = SectorGrid::compute(&frame, 80, 60, 8, 6);
    let bars = grid.detect_letterbox(0.05);

    assert_eq!(bars.top, 0);
    assert_eq!(bars.bottom, 0);
    assert_eq!(bars.left, 0);
    assert_eq!(bars.right, 0);
    assert!(!bars.has_bars());
}

#[test]
fn letterbox_crop_removes_black_bars() {
    // Frame: top 2 rows of grid are black, rest is red.
    // 80x60, 8x6 grid → each cell 10x10px.
    // Top 20px black = 2 grid rows, bottom 20px black = 2 grid rows.
    let frame = letterbox_frame(80, 60, 20, [255, 0, 0]);
    let grid = SectorGrid::compute(&frame, 80, 60, 8, 6);

    let bars = grid.detect_letterbox(0.05);
    assert!(bars.top >= 2);
    assert!(bars.bottom >= 2);

    let cropped = grid.crop_letterbox(&bars);
    assert!(cropped.is_some());
    let cropped = cropped.expect("crop should succeed");

    // All remaining sectors should be red (content area).
    for r in 0..cropped.rows() {
        for c in 0..cropped.cols() {
            let color = cropped.get(c, r);
            assert_eq!(
                color,
                [255, 0, 0],
                "cropped sector ({c}, {r}) should be red, got {color:?}"
            );
        }
    }
}

/// An all-black frame is not letterboxed, it is black. Detection used to
/// report bars from every edge here, which cropped the picture out of
/// existence and strobed as content changed; it now reports none.
#[test]
fn letterbox_all_black_frame_reports_no_bars() {
    let frame = solid_frame(80, 60, 0, 0, 0);
    let grid = SectorGrid::compute(&frame, 80, 60, 4, 3);
    let bars = grid.detect_letterbox(0.05);

    assert!(!bars.has_bars(), "an all-black frame has no letterbox bars");
    assert!(
        grid.crop_letterbox(&bars).is_some(),
        "with no bars the grid survives cropping intact"
    );
}

/// `crop_letterbox` still has to refuse bars that would leave nothing behind,
/// since callers can hand it any bars they like.
#[test]
fn letterbox_crop_refuses_bars_that_consume_the_grid() {
    let frame = solid_frame(80, 60, 10, 10, 10);
    let grid = SectorGrid::compute(&frame, 80, 60, 4, 3);

    let devouring = LetterboxBars {
        top: 2,
        bottom: 1,
        left: 0,
        right: 0,
    };
    assert!(
        grid.crop_letterbox(&devouring).is_none(),
        "bars covering every row must yield no croppable grid"
    );
}

// ── Zone Mapping ─────────────────────────────────────────────────────────

#[test]
fn zone_mapping_correct_zone_ids() {
    let frame = solid_frame(40, 30, 100, 200, 50);
    let grid = SectorGrid::compute(&frame, 40, 30, 4, 3);
    let zones = grid.to_zone_colors();

    assert_eq!(zones.len(), 12);
    assert_eq!(zones[0].0, "screen:sector_0_0");
    assert_eq!(zones[1].0, "screen:sector_0_1");
    assert_eq!(zones[4].0, "screen:sector_1_0");
    assert_eq!(zones[11].0, "screen:sector_2_3");

    for (_, color) in &zones {
        assert_eq!(*color, [100, 200, 50]);
    }
}

#[test]
fn zone_mapping_1x1_grid() {
    let frame = solid_frame(10, 10, 42, 42, 42);
    let grid = SectorGrid::compute(&frame, 10, 10, 1, 1);
    let zones = grid.to_zone_colors();

    assert_eq!(zones.len(), 1);
    assert_eq!(zones[0].0, "screen:sector_0_0");
    assert_eq!(zones[0].1, [42, 42, 42]);
}

#[test]
fn oversubscribed_grid_samples_every_sector_from_source_pixels() {
    let frame = vec![255, 0, 0, 255, 0, 0, 255, 255];
    let grid = SectorGrid::try_compute(&frame, 2, 1, 4, 1).expect("grid is admitted");

    assert_eq!(
        grid.colors(),
        &[[255, 0, 0], [255, 0, 0], [0, 0, 255], [0, 0, 255]]
    );
}

#[test]
fn proportional_sector_bounds_handle_u32_max_geometry() {
    assert_eq!(
        proportional_sector_bounds(u32::MAX - 1, u32::MAX, u32::MAX),
        Some((u32::MAX - 1, u32::MAX))
    );
    assert_eq!(proportional_sector_bounds(0, 1, 4), Some((0, 1)));
}

#[test]
fn sector_grid_updates_reuse_prepared_color_storage() {
    let mut grid = SectorGrid::try_with_capacity(8, 6).expect("grid storage is prepared");
    let capacity = grid.color_capacity();
    let frame = solid_frame(8, 6, 10, 20, 30);

    assert!(grid.try_update(&frame, 8, 6, 8, 6));
    assert!(grid.try_update(&frame, 8, 6, 4, 3));
    assert_eq!(grid.color_capacity(), capacity);
}

#[test]
fn analysis_resource_plan_uses_checked_byte_admission_without_axis_caps() {
    let unconstrained = ScreenAnalysisResourcePlan::try_new(256, 2, 30, u64::MAX)
        .expect("wide grid arithmetic is valid");
    let exact = unconstrained.peak_bytes();

    assert_eq!(
        ScreenAnalysisResourcePlan::try_new(256, 2, 30, exact),
        Ok(unconstrained)
    );
    assert!(ScreenAnalysisResourcePlan::try_new(256, 2, 30, exact - 1).is_err());
    assert_eq!(unconstrained.grid_cells(), 512);
}

#[test]
fn analysis_constructor_rejects_grid_above_installed_capacity() {
    let config = CaptureConfig {
        grid_cols: 256,
        grid_rows: 2,
        analysis_memory_bytes: 1,
        ..CaptureConfig::default()
    };
    let extent = PixelExtent::new(1, 1).expect("extent is non-empty");

    assert!(ScreenCaptureInput::with_requested_extent(config, extent).is_err());
}

// ── Temporal Smoothing: Step Change ──────────────────────────────────────

#[test]
fn temporal_smoothing_step_change_converges() {
    // Low threshold so scene-cut doesn't fire for a single zone.
    let mut smoother = TemporalSmoother::new(0.3, 10000.0);

    // Initialize with black.
    let mut colors = vec![[0u8, 0, 0]];
    smoother.apply(&mut colors);
    assert_eq!(colors[0], [0, 0, 0], "first frame passes through");

    // Step to white — should NOT jump to 255 immediately with alpha=0.3.
    let mut colors = vec![[255u8, 255, 255]];
    smoother.apply(&mut colors);
    assert!(
        colors[0][0] < 255,
        "should not reach target immediately, got {}",
        colors[0][0]
    );
    assert!(
        colors[0][0] > 0,
        "should move toward target, got {}",
        colors[0][0]
    );

    // Keep pushing white — should converge.
    for _ in 0..50 {
        let mut c = vec![[255u8, 255, 255]];
        smoother.apply(&mut c);
        colors = c;
    }

    // After 50 iterations with alpha=0.3, should be very close to 255.
    assert!(
        colors[0][0] >= 250,
        "should converge to target after many frames, got {}",
        colors[0][0]
    );
}

// ── Temporal Smoothing: Scene-Cut Detection ──────────────────────────────

#[test]
fn temporal_smoothing_scene_cut_resets_immediately() {
    // Low scene-cut threshold so it fires easily.
    let mut smoother = TemporalSmoother::new(0.1, 50.0);

    // Initialize with black.
    let mut colors = vec![[0u8, 0, 0]; 4];
    smoother.apply(&mut colors);

    // Massive change: all zones from black to bright white.
    // Mean per-zone diff = 255+255+255 = 765, well above threshold of 50.
    let mut colors = vec![[255u8, 255, 255]; 4];
    smoother.apply(&mut colors);

    // Scene cut should snap to new values immediately.
    assert_eq!(
        colors[0],
        [255, 255, 255],
        "scene cut should snap to new colors"
    );
    assert_eq!(colors[3], [255, 255, 255]);
}

#[test]
fn temporal_smoothing_response_is_frame_rate_independent() {
    fn response_after_one_second(fps: u32) -> u8 {
        let mut smoother = TemporalSmoother::new(0.3, 10_000.0);
        let mut colors = vec![[0, 0, 0]];
        smoother.apply_for_elapsed(&mut colors, Duration::ZERO);

        let interval = Duration::from_secs_f64(1.0 / f64::from(fps));
        for _ in 0..fps {
            colors[0] = [255, 255, 255];
            smoother.apply_for_elapsed(&mut colors, interval);
        }
        colors[0][0]
    }

    let at_30_hz = response_after_one_second(30);
    let at_60_hz = response_after_one_second(60);
    let at_120_hz = response_after_one_second(120);

    assert!(at_30_hz.abs_diff(at_60_hz) <= 1);
    assert!(at_60_hz.abs_diff(at_120_hz) <= 1);
}

#[test]
fn temporal_scene_cut_threshold_is_grid_size_independent() {
    const BASELINE: [[u8; 3]; 4] = [[0, 20, 40], [32, 64, 96], [96, 32, 64], [64, 96, 32]];
    const SOFT_CHANGE: [[u8; 3]; 4] = [[16, 36, 56], [48, 80, 112], [112, 48, 80], [80, 112, 48]];
    const SCENE_CUT: [[u8; 3]; 4] = [
        [255, 220, 240],
        [220, 255, 240],
        [240, 220, 255],
        [255, 240, 220],
    ];

    fn tiled(pattern: &[[u8; 3]; 4], repeats: u32) -> Vec<[u8; 3]> {
        let side = repeats * 2;
        (0..side)
            .flat_map(|row| (0..side).map(move |col| pattern[((row % 2) * 2 + col % 2) as usize]))
            .collect()
    }

    fn response(repeats: u32, target: &[[u8; 3]; 4]) -> Vec<[u8; 3]> {
        let mut smoother = TemporalSmoother::new(0.0, 100.0);
        let side = repeats * 2;
        let mut colors = tiled(&BASELINE, repeats);
        smoother.apply_for_elapsed_grid(&mut colors, side, side, Duration::from_millis(16));
        colors = tiled(target, repeats);
        smoother.apply_for_elapsed_grid(&mut colors, side, side, Duration::from_millis(16));
        colors
    }

    assert_eq!(response(1, &SOFT_CHANGE), tiled(&BASELINE, 1));
    assert_eq!(response(8, &SOFT_CHANGE), tiled(&BASELINE, 8));
    assert_eq!(response(1, &SCENE_CUT), tiled(&SCENE_CUT, 1));
    assert_eq!(response(8, &SCENE_CUT), tiled(&SCENE_CUT, 8));
}

// ── Temporal Smoothing: Static Scene ─────────────────────────────────────

#[test]
fn temporal_smoothing_static_scene_stable_output() {
    let mut smoother = TemporalSmoother::new(0.3, 10000.0);

    // Push the same color for many frames.
    let target = [128u8, 64, 192];
    let mut colors = vec![target; 3];
    smoother.apply(&mut colors);

    for _ in 0..30 {
        let mut c = vec![target; 3];
        smoother.apply(&mut c);
        colors = c;
    }

    // After converging on a static scene, output should match input exactly.
    for c in &colors {
        assert_eq!(*c, target, "static scene should stabilize at input color");
    }
}

// ── Temporal Smoothing: Alpha Boundaries ─────────────────────────────────

#[test]
fn temporal_smoothing_alpha_zero_freezes() {
    let mut smoother = TemporalSmoother::new(0.0, 10000.0);

    let mut colors = vec![[100u8, 100, 100]];
    smoother.apply(&mut colors);

    // Change input — alpha=0 should keep previous value.
    let mut colors = vec![[200u8, 200, 200]];
    smoother.apply(&mut colors);
    assert_eq!(
        colors[0],
        [100, 100, 100],
        "alpha=0 should freeze at initial value"
    );
}

#[test]
fn temporal_smoothing_alpha_one_passes_through() {
    let mut smoother = TemporalSmoother::new(1.0, 10000.0);

    let mut colors = vec![[50u8, 50, 50]];
    smoother.apply(&mut colors);

    let mut colors = vec![[200u8, 200, 200]];
    smoother.apply(&mut colors);
    assert_eq!(
        colors[0],
        [200, 200, 200],
        "alpha=1 should pass through immediately"
    );
}

#[test]
fn temporal_smoothing_reset_clears_state() {
    let mut smoother = TemporalSmoother::new(0.3, 10000.0);

    let mut colors = vec![[100u8, 100, 100]];
    smoother.apply(&mut colors);

    smoother.reset();

    // After reset, next apply should initialize fresh (pass through).
    let mut colors = vec![[200u8, 200, 200]];
    smoother.apply(&mut colors);
    assert_eq!(
        colors[0],
        [200, 200, 200],
        "after reset, first frame should pass through"
    );
}

// ── ScreenCaptureInput: Integration ──────────────────────────────────────

#[test]
fn screen_capture_input_lifecycle() {
    let mut input = ScreenCaptureInput::new(CaptureConfig::default());
    assert!(!input.is_running());
    assert_eq!(input.name(), "screen_capture");

    input.start().expect("start should succeed");
    assert!(input.is_running());

    // No frame pushed yet — sample returns None.
    let data = input.sample().expect("sample should succeed");
    assert!(matches!(data, InputData::None));

    input.stop();
    assert!(!input.is_running());
}

#[test]
fn screen_capture_input_produces_screen_data() {
    let config = CaptureConfig {
        grid_cols: 2,
        grid_rows: 2,
        letterbox_enabled: false,
        ..CaptureConfig::default()
    };
    let mut input = ScreenCaptureInput::new(config);
    input.start().expect("start should succeed");

    let frame = solid_frame(40, 40, 200, 100, 50);
    input.push_frame(&frame, 40, 40).expect("frame is admitted");

    let data = input.sample().expect("sample should succeed");
    match data {
        InputData::Screen(screen) => {
            assert_eq!(screen.zone_colors.len(), 4, "2x2 grid = 4 zones");
            assert_eq!(screen.grid_width, 2);
            assert_eq!(screen.grid_height, 2);
            assert_eq!(screen.source_width, 40);
            assert_eq!(screen.source_height, 40);
            let downscale = screen
                .canvas_downscale
                .as_ref()
                .expect("screen data should include downscaled canvas");
            // A square source publishes a square surface: the downscale fits
            // within the canvas bounds rather than stretching to fill them.
            assert_eq!(downscale.width(), DEFAULT_CANVAS_HEIGHT);
            assert_eq!(downscale.height(), DEFAULT_CANVAS_HEIGHT);
            assert!(downscale.width() <= DEFAULT_CANVAS_WIDTH);
            assert_eq!(
                downscale.get_pixel(0, 0),
                hypercolor_core::types::canvas::Rgba::new(200, 100, 50, 255)
            );
            for zc in &screen.zone_colors {
                assert_eq!(zc.colors.len(), 1, "one color per zone");
                assert_eq!(zc.colors[0], [200, 100, 50]);
            }
        }
        other => panic!("expected InputData::Screen, got {other:?}"),
    }
}

#[test]
fn screen_capture_input_reuses_downscale_surface_pool_after_warmup() {
    let config = CaptureConfig {
        grid_cols: 1,
        grid_rows: 1,
        letterbox_enabled: false,
        ..CaptureConfig::default()
    };
    let mut input = ScreenCaptureInput::new(config);
    input.start().expect("start should succeed");

    let frame_a = solid_frame(4, 4, 10, 20, 30);
    let frame_b = solid_frame(4, 4, 40, 50, 60);
    let frame_c = solid_frame(4, 4, 70, 80, 90);

    input
        .push_frame(&frame_a, 4, 4)
        .expect("frame A is admitted");
    let first = match input.sample().expect("first sample should succeed") {
        InputData::Screen(screen) => screen
            .canvas_downscale
            .expect("first sample should include downscale")
            .rgba_bytes()
            .as_ptr()
            .addr(),
        other => panic!("expected InputData::Screen, got {other:?}"),
    };

    input
        .push_frame(&frame_b, 4, 4)
        .expect("frame B is admitted");
    let second = match input.sample().expect("second sample should succeed") {
        InputData::Screen(screen) => screen
            .canvas_downscale
            .expect("second sample should include downscale")
            .rgba_bytes()
            .as_ptr()
            .addr(),
        other => panic!("expected InputData::Screen, got {other:?}"),
    };

    input
        .push_frame(&frame_c, 4, 4)
        .expect("frame C is admitted");
    let third = match input.sample().expect("third sample should succeed") {
        InputData::Screen(screen) => screen
            .canvas_downscale
            .expect("third sample should include downscale")
            .rgba_bytes()
            .as_ptr()
            .addr(),
        other => panic!("expected InputData::Screen, got {other:?}"),
    };

    assert_ne!(first, second);
    assert_eq!(first, third);
}

#[test]
fn screen_zone_snapshots_reuse_three_prepared_slots_without_growth() {
    let mut input = ScreenCaptureInput::new(CaptureConfig::default());
    input.start().expect("screen input starts");
    let frame = solid_frame(8, 6, 30, 60, 90);

    let mut snapshots = Vec::new();
    for _ in 0..3 {
        assert!(input.push_frame(&frame, 8, 6).expect("frame is admitted"));
        let InputData::Screen(mut snapshot) = input.sample().expect("snapshot publishes") else {
            panic!("expected screen data");
        };
        snapshot.canvas_downscale = None;
        snapshots.push(snapshot);
    }

    assert!(
        !input
            .push_frame(&frame, 8, 6)
            .expect("pool exhaustion is a valid latest-value drop")
    );
    let reclaimed_pointer = snapshots[0].zone_colors.as_ptr();
    snapshots.remove(0);
    assert!(
        input
            .push_frame(&frame, 8, 6)
            .expect("released slot is reusable")
    );
    let InputData::Screen(reused) = input.sample().expect("reused snapshot publishes") else {
        panic!("expected screen data");
    };
    assert_eq!(reused.zone_colors.as_ptr(), reclaimed_pointer);
}

#[test]
fn screen_capture_input_zone_ids_in_screen_data() {
    let config = CaptureConfig {
        grid_cols: 2,
        grid_rows: 1,
        letterbox_enabled: false,
        ..CaptureConfig::default()
    };
    let mut input = ScreenCaptureInput::new(config);
    input.start().expect("start should succeed");

    let frame = solid_frame(20, 10, 100, 100, 100);
    input.push_frame(&frame, 20, 10).expect("frame is admitted");

    let data = input.sample().expect("sample should succeed");
    match data {
        InputData::Screen(screen) => {
            assert_eq!(screen.zone_colors.len(), 2);
            assert_eq!(screen.zone_colors[0].zone_id, "screen:sector_0_0");
            assert_eq!(screen.zone_colors[1].zone_id, "screen:sector_0_1");
        }
        other => panic!("expected InputData::Screen, got {other:?}"),
    }
}

#[test]
fn screen_capture_input_stopped_returns_none() {
    let mut input = ScreenCaptureInput::new(CaptureConfig::default());
    input.start().expect("start should succeed");

    let frame = solid_frame(40, 40, 255, 0, 0);
    input.push_frame(&frame, 40, 40).expect("frame is admitted");

    // Confirm data is available.
    let data = input.sample().expect("sample should succeed");
    assert!(matches!(data, InputData::Screen(_)));

    // Stop should clear data.
    input.stop();
    let data = input.sample().expect("sample should succeed");
    assert!(matches!(data, InputData::None));
}

// ── Edge Cases ───────────────────────────────────────────────────────────

#[test]
fn sector_grid_zero_dimensions_treated_as_1x1() {
    let frame = solid_frame(10, 10, 99, 99, 99);
    let grid = SectorGrid::compute(&frame, 10, 10, 0, 0);
    assert_eq!(grid.cols(), 1);
    assert_eq!(grid.rows(), 1);
    assert_eq!(grid.sector_count(), 1);
    assert_eq!(grid.get(0, 0), [99, 99, 99]);
}

#[test]
fn sector_grid_empty_frame_buffer() {
    let grid = SectorGrid::compute(&[], 0, 0, 4, 4);
    assert_eq!(grid.sector_count(), 16);
    // All sectors should be black.
    for r in 0..4 {
        for c in 0..4 {
            assert_eq!(grid.get(c, r), [0, 0, 0]);
        }
    }
}

#[test]
fn temporal_smoother_zone_count_change_reinitializes() {
    let mut smoother = TemporalSmoother::new(0.3, 10000.0);

    let mut two = vec![[100u8, 100, 100]; 2];
    smoother.apply(&mut two);

    // Change zone count — should re-initialize.
    let mut three = vec![[200u8, 200, 200]; 3];
    smoother.apply(&mut three);
    assert_eq!(
        three[0],
        [200, 200, 200],
        "zone count change should pass through"
    );
}

#[test]
fn temporal_smoother_equal_count_shape_change_reinitializes() {
    let mut smoother = TemporalSmoother::new(0.0, 10_000.0);
    let mut horizontal = vec![[10, 20, 30], [40, 50, 60]];
    smoother.apply_for_elapsed_grid(&mut horizontal, 2, 1, Duration::from_millis(16));

    let mut vertical = vec![[200, 180, 160], [140, 120, 100]];
    let expected = vertical.clone();
    smoother.apply_for_elapsed_grid(&mut vertical, 1, 2, Duration::from_millis(16));

    assert_eq!(vertical, expected);
}

#[test]
fn temporal_smoother_adapts_to_checked_portrait_ultrawide_and_odd_shapes() {
    let mut smoother = TemporalSmoother::new(0.0, 10_000.0);

    for (width, height) in [(3, 7), (11, 3), (7, 5)] {
        let len = usize::try_from(width * height).expect("test grid should fit usize");
        let mut colors = (0..len)
            .map(|index| {
                let coordinate = index.to_le_bytes()[0];
                [
                    coordinate,
                    coordinate.wrapping_mul(3),
                    coordinate.wrapping_mul(7),
                ]
            })
            .collect::<Vec<_>>();
        let expected = colors.clone();

        smoother.apply_for_elapsed_grid(&mut colors, width, height, Duration::from_millis(16));

        assert_eq!(colors, expected, "shape {width}x{height} should initialize");

        colors.fill([255, 255, 255]);
        smoother.apply_for_elapsed_grid(&mut colors, width, height, Duration::from_millis(16));
        assert_eq!(
            colors, expected,
            "shape {width}x{height} should retain history"
        );
    }
}

#[test]
fn temporal_smoother_rejects_invalid_grid_math_transactionally() {
    let mut smoother = TemporalSmoother::new(0.0, 10_000.0);
    let mut baseline = [[0, 255, 0]];
    smoother.apply_for_elapsed_grid(&mut baseline, 1, 1, Duration::from_millis(16));

    let mut overflowing = [[255, 0, 255]];
    smoother.apply_for_elapsed_grid(
        &mut overflowing,
        MAX_REPRESENTABLE_CAPTURE_FPS,
        u32::MAX,
        Duration::from_millis(16),
    );
    assert_eq!(overflowing, [[255, 0, 255]]);

    let mut mismatched = [[255, 255, 0], [0, 255, 255]];
    smoother.apply_for_elapsed_grid(&mut mismatched, 1, 1, Duration::from_millis(16));
    assert_eq!(mismatched, [[255, 255, 0], [0, 255, 255]]);

    let mut next = [[255, 0, 255]];
    smoother.apply_for_elapsed_grid(&mut next, 1, 1, Duration::from_millis(16));
    assert_eq!(next, [[0, 255, 0]]);
}

#[test]
fn malformed_push_preserves_the_last_valid_publication() {
    let mut input = ScreenCaptureInput::new(CaptureConfig {
        grid_cols: 2,
        grid_rows: 2,
        smoothing_alpha: 1.0,
        ..CaptureConfig::default()
    });
    input.start().expect("screen input should start");
    let valid = solid_frame(4, 4, 90, 40, 200);
    input
        .push_frame(&valid, 4, 4)
        .expect("valid frame is admitted");
    let InputData::Screen(before) = input.sample().expect("valid sample should publish") else {
        panic!("expected valid screen data");
    };
    let before_surface = before
        .canvas_downscale
        .as_ref()
        .expect("valid sample should publish a policy surface");
    let before_pointer = before_surface.rgba_bytes().as_ptr();
    let before_status = input
        .source_status_handle()
        .expect("screen input should expose status")
        .snapshot();

    input
        .push_frame(&[], u32::MAX, 2)
        .expect("malformed dimensions do not allocate");
    let InputData::Screen(after) = input.sample().expect("malformed push should preserve data")
    else {
        panic!("expected retained screen data");
    };
    let after_surface = after
        .canvas_downscale
        .as_ref()
        .expect("retained sample should keep its policy surface");

    assert_eq!(input.frame_dimensions(), (4, 4));
    assert_eq!(after.zone_colors, before.zone_colors);
    assert_eq!(after_surface.rgba_bytes().as_ptr(), before_pointer);
    assert_eq!(
        input
            .source_status_handle()
            .expect("screen input should expose status")
            .snapshot(),
        before_status
    );
}

#[test]
fn letterbox_bars_default_has_no_bars() {
    let bars = LetterboxBars::default();
    assert!(!bars.has_bars());
}

// ── Color Tuning ──────────────────────────────────────────────────────────

#[test]
fn color_tuning_neutral_is_identity() {
    let tuning = ColorTuning::default();
    assert!(tuning.is_neutral());

    let original = [[200u8, 100, 50], [0, 255, 128]];
    let mut colors = original;
    tuning.apply(&mut colors);
    assert_eq!(colors, original, "neutral tuning must not alter colors");
}

#[test]
fn color_tuning_zero_saturation_produces_gray() {
    let tuning = ColorTuning {
        saturation: 0.0,
        ..ColorTuning::default()
    };

    let mut colors = [[255u8, 0, 0]];
    tuning.apply(&mut colors);
    let [r, g, b] = colors[0];
    assert_eq!(r, g, "desaturated color should be gray");
    assert_eq!(g, b, "desaturated color should be gray");
}

#[test]
fn color_tuning_saturation_boost_increases_chroma() {
    let tuning = ColorTuning {
        saturation: 2.0,
        ..ColorTuning::default()
    };

    // A muted red: some chroma, plenty of headroom.
    let mut colors = [[160u8, 110, 110]];
    tuning.apply(&mut colors);
    let [r, g, b] = colors[0];
    let boosted_spread = i16::from(r) - i16::from(g);
    assert!(
        boosted_spread > 50,
        "saturation boost should widen channel spread, got {boosted_spread}"
    );
    assert_eq!(g, b, "neutral channels should stay matched");
}

#[test]
fn color_tuning_brightness_scales_output() {
    let brighter = ColorTuning {
        brightness: 1.5,
        ..ColorTuning::default()
    };
    let mut colors = [[100u8, 100, 100]];
    brighter.apply(&mut colors);
    assert!(
        colors[0][0] > 100,
        "brightness > 1 should lift output, got {}",
        colors[0][0]
    );

    let dimmer = ColorTuning {
        brightness: 0.5,
        ..ColorTuning::default()
    };
    let mut colors = [[100u8, 100, 100]];
    dimmer.apply(&mut colors);
    assert!(
        colors[0][0] < 100,
        "brightness < 1 should lower output, got {}",
        colors[0][0]
    );
}

#[test]
fn color_tuning_gamma_shapes_midtones() {
    let darker_mids = ColorTuning {
        gamma: 2.0,
        ..ColorTuning::default()
    };
    let mut colors = [[128u8, 128, 128]];
    darker_mids.apply(&mut colors);
    assert!(
        colors[0][0] < 128,
        "gamma > 1 should darken midtones, got {}",
        colors[0][0]
    );

    // Endpoints survive any gamma.
    let mut endpoints = [[0u8, 0, 0], [255, 255, 255]];
    darker_mids.apply(&mut endpoints);
    assert_eq!(endpoints[0], [0, 0, 0]);
    assert_eq!(endpoints[1], [255, 255, 255]);
}

#[test]
fn color_tuning_clamps_out_of_range_parameters() {
    let wild = ColorTuning {
        saturation: 100.0,
        brightness: -5.0,
        gamma: 0.0,
    }
    .clamped();
    assert!((wild.saturation - 4.0).abs() < f32::EPSILON);
    assert!(wild.brightness.abs() < f32::EPSILON);
    assert!((wild.gamma - 0.2).abs() < f32::EPSILON);
}

// ── Live Settings ─────────────────────────────────────────────────────────

#[test]
fn apply_settings_updates_tuning_live() {
    let config = CaptureConfig {
        grid_cols: 2,
        grid_rows: 2,
        smoothing_alpha: 1.0,
        letterbox_enabled: false,
        ..CaptureConfig::default()
    };
    let mut input = ScreenCaptureInput::new(config.clone());
    input.start().expect("start should succeed");

    let frame = solid_frame(40, 40, 160, 110, 110);
    input.push_frame(&frame, 40, 40).expect("frame is admitted");
    let InputData::Screen(before) = input.sample().expect("sample succeeds") else {
        panic!("expected screen data");
    };

    input
        .apply_settings(CaptureConfig {
            tuning: ColorTuning {
                saturation: 0.0,
                ..ColorTuning::default()
            },
            ..config
        })
        .expect("representable live cadence is admitted");
    input.push_frame(&frame, 40, 40).expect("frame is admitted");
    let InputData::Screen(after) = input.sample().expect("sample succeeds") else {
        panic!("expected screen data");
    };

    let pre = before.zone_colors[0].colors[0];
    let post = after.zone_colors[0].colors[0];
    assert_ne!(pre[0], pre[1], "untuned color keeps chroma");
    assert_eq!(post[0], post[1], "live desaturation should apply");
}

#[test]
fn apply_settings_grid_change_takes_effect_next_frame() {
    let config = CaptureConfig {
        grid_cols: 2,
        grid_rows: 2,
        letterbox_enabled: false,
        ..CaptureConfig::default()
    };
    let mut input = ScreenCaptureInput::new(config.clone());
    input.start().expect("start should succeed");

    let frame = solid_frame(64, 64, 50, 100, 150);
    input.push_frame(&frame, 64, 64).expect("frame is admitted");

    input
        .apply_settings(CaptureConfig {
            grid_cols: 4,
            grid_rows: 4,
            ..config
        })
        .expect("representable live cadence is admitted");
    input.push_frame(&frame, 64, 64).expect("frame is admitted");
    let InputData::Screen(data) = input.sample().expect("sample succeeds") else {
        panic!("expected screen data");
    };

    assert_eq!(data.grid_width, 4);
    assert_eq!(data.grid_height, 4);
    assert_eq!(data.zone_colors.len(), 16);
}

#[test]
fn disabling_letterbox_live_clears_stale_bars() {
    let config = CaptureConfig {
        grid_cols: 4,
        grid_rows: 6,
        smoothing_alpha: 1.0,
        letterbox_enabled: true,
        ..CaptureConfig::default()
    };
    let mut input = ScreenCaptureInput::new(config.clone());
    input.start().expect("start should succeed");

    // Heavy letterbox: top and bottom thirds black.
    let frame = letterbox_frame(60, 60, 20, [200, 50, 50]);
    input.push_frame(&frame, 60, 60).expect("frame is admitted");
    assert!(
        input.letterbox_bars().has_bars(),
        "letterbox should be detected while enabled"
    );
    let InputData::Screen(cropped) = input.sample().expect("sample succeeds") else {
        panic!("expected screen data");
    };
    assert_eq!(cropped.grid_width, 4);
    assert_eq!(cropped.grid_height, 2);
    assert_eq!(cropped.zone_colors.len(), 8);
    let cropped_surface = cropped
        .canvas_downscale
        .as_ref()
        .expect("letterbox crop should publish a surface");
    assert_eq!(cropped_surface.width(), 640);
    assert_eq!(cropped_surface.height(), 213);

    input
        .apply_settings(CaptureConfig {
            letterbox_enabled: false,
            ..config
        })
        .expect("representable live cadence is admitted");
    input.push_frame(&frame, 60, 60).expect("frame is admitted");
    assert!(
        !input.letterbox_bars().has_bars(),
        "disabling letterbox must clear stale bars"
    );
    let InputData::Screen(data) = input.sample().expect("sample succeeds") else {
        panic!("expected screen data");
    };
    assert_eq!(
        data.zone_colors.len(),
        24,
        "full uncropped grid should be reported once letterbox is off"
    );
    let full_surface = data
        .canvas_downscale
        .as_ref()
        .expect("full frame should publish a surface");
    assert_eq!(full_surface.width(), 480);
    assert_eq!(full_surface.height(), 480);
}

// ─── Monitor selection ───────────────────────────────────────────────────────

#[test]
fn monitor_source_accepts_prefixed_and_bare_indices() {
    for (source, expected) in [
        ("monitor:0", 0),
        ("monitor:1", 1),
        ("monitor:11", 11),
        ("display:2", 2),
        ("3", 3),
        ("  monitor: 2  ", 2),
    ] {
        assert_eq!(
            hypercolor_core::input::screen::monitor_selector_from_source(source),
            hypercolor_windows_capture::MonitorSelector::Index(expected),
            "source {source:?} should select monitor {expected}"
        );
    }
}

/// Auto means the primary display while non-numeric values remain stable ids.
#[test]
fn monitor_sources_preserve_auto_and_stable_ids() {
    use hypercolor_windows_capture::MonitorSelector;

    assert_eq!(
        hypercolor_core::input::screen::monitor_selector_from_source("auto"),
        MonitorSelector::Auto
    );
    assert_eq!(
        hypercolor_core::input::screen::monitor_selector_from_source(""),
        MonitorSelector::Auto
    );
    assert_eq!(
        hypercolor_core::input::screen::monitor_selector_from_source(
            r"monitor:display:\\?\display#del4098#instance"
        ),
        MonitorSelector::StableId(r"display:\\?\display#del4098#instance".to_owned())
    );
}

// ─── Letterbox degeneracy ────────────────────────────────────────────────────

/// A uniformly dark frame is not letterboxed, it is just dark. Reporting bars
/// from every edge at once made `crop_letterbox` throw the whole picture away,
/// and because the verdict flips with ordinary content changes it strobed.
/// Linear luminance makes this easy to hit: sRGB 30/255 is only 0.013 linear,
/// so a dark-themed desktop reads as black from every edge.
#[test]
fn uniformly_dark_frames_report_no_letterbox_bars() {
    let dark = vec![12_u8; 64 * 48 * 4];
    let grid = SectorGrid::compute(&dark, 64, 48, 8, 6);
    let bars = grid.detect_letterbox(0.02);

    assert_eq!(
        (bars.top, bars.bottom, bars.left, bars.right),
        (0, 0, 0, 0),
        "an all-dark frame must not be treated as entirely letterbox"
    );
    assert!(!bars.has_bars());
    assert!(
        grid.crop_letterbox(&bars).is_some(),
        "a non-degenerate crop must still be possible"
    );
}

/// The guard must not disarm real letterboxing, which always leaves content
/// between the bars.
#[test]
fn genuine_letterbox_bars_are_still_detected() {
    let width = 64_u32;
    let height = 48_u32;
    let mut frame = vec![0_u8; (width * height * 4) as usize];
    // Bright band across the middle third, black bars above and below.
    for y in 16..32 {
        for x in 0..width {
            let px = ((y * width + x) * 4) as usize;
            frame[px] = 220;
            frame[px + 1] = 220;
            frame[px + 2] = 220;
            frame[px + 3] = 255;
        }
    }

    let grid = SectorGrid::compute(&frame, width, height, 8, 6);
    let bars = grid.detect_letterbox(0.02);

    assert!(bars.top > 0, "top bar should be detected");
    assert!(bars.bottom > 0, "bottom bar should be detected");
    assert!(
        bars.top + bars.bottom < 6,
        "real bars must leave content behind"
    );
    assert_eq!((bars.left, bars.right), (0, 0), "no pillarboxing here");
}

// ─── Aspect preservation ─────────────────────────────────────────────────────

/// The published surface must carry the source's aspect ratio. Targeting the
/// canvas bounds directly squashed 16:9 into 4:3, and no downstream fit mode
/// can undo distortion already baked into the pixels.
#[test]
fn downscale_target_preserves_source_aspect() {
    use hypercolor_core::input::screen::fit_within;

    // 16:9 into a 4:3 box fits by width.
    assert_eq!(fit_within(1920, 1080, 640, 480), (640, 360));
    assert_eq!(fit_within(3840, 2160, 640, 480), (640, 360));
    // 4:3 source fills the box exactly.
    assert_eq!(fit_within(1600, 1200, 640, 480), (640, 480));
    // A portrait source fits by height instead.
    assert_eq!(fit_within(1080, 1920, 640, 480), (270, 480));
    // Ultrawide.
    assert_eq!(fit_within(3440, 1440, 640, 480), (640, 267));
}

#[test]
fn downscale_target_never_returns_a_zero_dimension() {
    use hypercolor_core::input::screen::fit_within;

    assert_eq!(fit_within(0, 0, 640, 480), (640, 480));
    assert_eq!(fit_within(1920, 1080, 0, 0), (1, 1));
    // Extreme ratios still round up to a usable pixel.
    let (w, h) = fit_within(100_000, 1, 640, 480);
    assert!(w >= 1 && h >= 1, "got {w}x{h}");
}

#[test]
fn pushed_frames_publish_an_aspect_correct_surface() {
    let mut input = ScreenCaptureInput::new(CaptureConfig::default());
    input.start().expect("start should succeed");

    // A 16:9 frame must not come back as 4:3.
    let frame = solid_frame(320, 180, 90, 40, 200);
    input
        .push_frame(&frame, 320, 180)
        .expect("frame is admitted");

    let InputData::Screen(data) = input.sample().expect("sample succeeds") else {
        panic!("expected screen data");
    };
    let surface = data
        .canvas_downscale
        .as_ref()
        .expect("a downscaled surface should be published");
    let descriptor = surface.descriptor();

    assert_eq!(
        (descriptor.width, descriptor.height),
        (640, 360),
        "16:9 input must publish a 16:9 surface"
    );
}

#[test]
fn arbitrary_wide_requested_extent_is_published_without_a_hidden_cap() {
    let requested_extent = PixelExtent::new(5_001, 1).expect("extent is non-empty");
    let mut input = ScreenCaptureInput::with_requested_extent(
        CaptureConfig {
            grid_cols: 1,
            grid_rows: 1,
            smoothing_alpha: 1.0,
            letterbox_enabled: false,
            ..CaptureConfig::default()
        },
        requested_extent,
    )
    .expect("small ultrawide surface is admitted");
    input.start().expect("screen input starts");
    let frame = solid_frame(5_001, 1, 90, 40, 200);

    assert!(
        input
            .push_frame(&frame, 5_001, 1)
            .expect("ultrawide frame is admitted")
    );
    let InputData::Screen(data) = input.sample().expect("screen sample succeeds") else {
        panic!("expected screen data");
    };
    let surface = data
        .canvas_downscale
        .expect("ultrawide surface is published");
    assert_eq!((surface.width(), surface.height()), (5_001, 1));
}

#[test]
fn failed_extent_admission_preserves_last_good_request_and_publication() {
    let initial_extent = PixelExtent::new(4, 4).expect("extent is non-empty");
    let mut input = ScreenCaptureInput::with_requested_extent(
        CaptureConfig {
            grid_cols: 1,
            grid_rows: 1,
            smoothing_alpha: 1.0,
            letterbox_enabled: false,
            ..CaptureConfig::default()
        },
        initial_extent,
    )
    .expect("small surface is admitted");
    input.start().expect("screen input starts");
    let frame = solid_frame(4, 4, 30, 60, 90);
    input
        .push_frame(&frame, 4, 4)
        .expect("last-good frame is admitted");
    let InputData::Screen(before) = input.sample().expect("last-good sample succeeds") else {
        panic!("expected screen data");
    };
    let before_surface = before
        .canvas_downscale
        .expect("last-good surface is published");
    let before_pointer = before_surface.rgba_bytes().as_ptr();
    let impossible = PixelExtent::new(u32::MAX, u32::MAX).expect("extent is non-empty");

    assert!(input.set_requested_extent(impossible).is_err());
    assert_eq!(input.requested_extent(), initial_extent);
    let InputData::Screen(after) = input.sample().expect("retained sample succeeds") else {
        panic!("expected retained screen data");
    };
    let after_surface = after
        .canvas_downscale
        .expect("retained surface remains published");
    assert_eq!(after_surface.rgba_bytes().as_ptr(), before_pointer);
    assert_eq!((after_surface.width(), after_surface.height()), (4, 4));
    assert!(
        ScreenCaptureInput::with_requested_extent(CaptureConfig::default(), impossible).is_err()
    );
}
