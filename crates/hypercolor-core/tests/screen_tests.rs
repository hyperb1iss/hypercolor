//! Tests for the screen capture input pipeline.
//!
//! All tests use synthetic RGBA pixel buffers — no actual screen capture needed.

use hypercolor_core::input::screen::sector::{LetterboxBars, SectorGrid};
use hypercolor_core::input::screen::smooth::TemporalSmoother;
use hypercolor_core::input::screen::{CaptureConfig, ScreenCaptureInput};
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

#[test]
fn letterbox_all_black_returns_no_bars_on_crop() {
    // Entire frame is black — degenerate case.
    let frame = solid_frame(80, 60, 0, 0, 0);
    let grid = SectorGrid::compute(&frame, 80, 60, 4, 3);
    let bars = grid.detect_letterbox(0.05);

    // All rows/cols are black, so bars consume the entire grid.
    let cropped = grid.crop_letterbox(&bars);
    assert!(
        cropped.is_none(),
        "all-black frame should yield no content after crop"
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
    // Total diff = 4 zones * (255+255+255) = 3060, well above threshold of 50.
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
    input.push_frame(&frame, 40, 40);

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
            assert_eq!(downscale.width(), DEFAULT_CANVAS_WIDTH);
            assert_eq!(downscale.height(), DEFAULT_CANVAS_HEIGHT);
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
    input.push_frame(&frame, 20, 10);

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
    input.push_frame(&frame, 40, 40);

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
fn letterbox_bars_default_has_no_bars() {
    let bars = LetterboxBars::default();
    assert!(!bars.has_bars());
}
