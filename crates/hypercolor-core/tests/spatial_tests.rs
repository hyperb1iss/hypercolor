//! Tests for the spatial sampling engine.
//!
//! Uses hand-crafted canvases with known pixel data to verify that
//! topology position generation, coordinate transforms, and sampling
//! algorithms produce the expected LED colors.

use hypercolor_core::spatial::{
    PreparedZoneSamples, SpatialEngine, generate_positions, sample_led,
};
use hypercolor_types::canvas::{Canvas, Rgba, linear_to_srgb, srgb_to_linear};
use hypercolor_types::event::ZoneColors;
use hypercolor_types::spatial::{
    Corner, DeviceZone, EdgeBehavior, LedTopology, NormalizedPosition, SamplingMode,
    StripDirection, Winding,
};

// ── Helpers ─────────────────────────────────────────────────────────────────

/// Build a minimal `SpatialLayout` with the given zones.
fn test_layout(
    zones: Vec<DeviceZone>,
    canvas_width: u32,
    canvas_height: u32,
) -> hypercolor_types::spatial::SpatialLayout {
    hypercolor_types::spatial::SpatialLayout {
        id: "test".into(),
        name: "Test Layout".into(),
        description: None,
        canvas_width,
        canvas_height,
        zones,
        default_sampling_mode: SamplingMode::Bilinear,
        default_edge_behavior: hypercolor_types::spatial::EdgeBehavior::Clamp,
        spaces: None,
        version: 1,
    }
}

/// Build a test zone centered on the full canvas with the given topology.
fn full_canvas_zone(id: &str, topology: LedTopology) -> DeviceZone {
    DeviceZone {
        id: id.into(),
        name: id.into(),
        device_id: format!("test:{id}"),
        zone_name: None,
        position: NormalizedPosition::new(0.5, 0.5),
        size: NormalizedPosition::new(1.0, 1.0),
        rotation: 0.0,
        scale: 1.0,
        orientation: None,
        topology,
        led_positions: Vec::new(),
        led_mapping: None,
        sampling_mode: None,
        edge_behavior: None,
        shape: None,
        shape_preset: None,
        display_order: 0,
        attachment: None,
        brightness: None,
    }
}

/// Build a zone with explicit position, size, and sampling mode.
fn custom_zone(
    id: &str,
    topology: LedTopology,
    position: NormalizedPosition,
    size: NormalizedPosition,
    sampling_mode: Option<SamplingMode>,
) -> DeviceZone {
    DeviceZone {
        id: id.into(),
        name: id.into(),
        device_id: format!("test:{id}"),
        zone_name: None,
        position,
        size,
        rotation: 0.0,
        scale: 1.0,
        orientation: None,
        topology,
        led_positions: Vec::new(),
        led_mapping: None,
        sampling_mode,
        edge_behavior: None,
        shape: None,
        shape_preset: None,
        display_order: 0,
        attachment: None,
        brightness: None,
    }
}

/// Create a canvas filled with a single solid color.
fn solid_canvas(width: u32, height: u32, color: Rgba) -> Canvas {
    let mut canvas = Canvas::new(width, height);
    canvas.fill(color);
    canvas
}

/// Create a horizontal gradient from `left` to `right`.
#[allow(clippy::as_conversions, clippy::cast_precision_loss)]
fn horizontal_gradient(width: u32, height: u32, left: Rgba, right: Rgba) -> Canvas {
    let mut canvas = Canvas::new(width, height);
    for y in 0..height {
        for x in 0..width {
            let t = if width <= 1 {
                0.5
            } else {
                x as f32 / (width - 1) as f32
            };
            let r = f32::from(left.r) + (f32::from(right.r) - f32::from(left.r)) * t;
            let g = f32::from(left.g) + (f32::from(right.g) - f32::from(left.g)) * t;
            let b = f32::from(left.b) + (f32::from(right.b) - f32::from(left.b)) * t;
            #[allow(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                clippy::as_conversions
            )]
            canvas.set_pixel(
                x,
                y,
                Rgba::new(r.round() as u8, g.round() as u8, b.round() as u8, 255),
            );
        }
    }
    canvas
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::as_conversions
)]
fn linear_byte_to_srgb(value: u8) -> u8 {
    (linear_to_srgb(f32::from(value) / 255.0) * 255.0)
        .round()
        .clamp(0.0, 255.0) as u8
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::as_conversions,
    clippy::cast_precision_loss
)]
fn srgb_byte_to_linear_u16(value: u8) -> u16 {
    (srgb_to_linear(f32::from(value) / 255.0) * 65535.0)
        .round()
        .clamp(0.0, 65535.0) as u16
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::as_conversions,
    clippy::cast_precision_loss
)]
fn linear_u16_to_srgb_byte(value: u16) -> u8 {
    (linear_to_srgb(f32::from(value) / 65535.0) * 255.0)
        .round()
        .clamp(0.0, 255.0) as u8
}

fn expected_bilinear_channel(
    top_left: u8,
    top_right: u8,
    bottom_left: u8,
    bottom_right: u8,
    x_upper_weight: u16,
    y_upper_weight: u16,
) -> u8 {
    let x_lower_weight = 256_u16.saturating_sub(x_upper_weight);
    let y_lower_weight = 256_u16.saturating_sub(y_upper_weight);

    let top = u32::from(srgb_byte_to_linear_u16(top_left)) * u32::from(x_lower_weight)
        + u32::from(srgb_byte_to_linear_u16(top_right)) * u32::from(x_upper_weight);
    let bottom = u32::from(srgb_byte_to_linear_u16(bottom_left)) * u32::from(x_lower_weight)
        + u32::from(srgb_byte_to_linear_u16(bottom_right)) * u32::from(x_upper_weight);
    let blended = u16::try_from(
        (u64::from(top) * u64::from(y_lower_weight)
            + u64::from(bottom) * u64::from(y_upper_weight))
            >> 16,
    )
    .expect("bilinear blend stays within u16 range");

    linear_u16_to_srgb_byte(blended)
}

// ── Topology Position Generation ────────────────────────────────────────────

#[test]
fn strip_left_to_right_positions() {
    let positions = generate_positions(&LedTopology::Strip {
        count: 5,
        direction: StripDirection::LeftToRight,
    });
    assert_eq!(positions.len(), 5);
    // First LED at left edge, last at right edge, all at y=0.5.
    assert_eq!(positions[0], NormalizedPosition::new(0.0, 0.5));
    assert_eq!(positions[4], NormalizedPosition::new(1.0, 0.5));
    // Midpoint LED at center.
    assert!((positions[2].x - 0.5).abs() < f32::EPSILON);
    assert!((positions[2].y - 0.5).abs() < f32::EPSILON);
}

#[test]
fn strip_right_to_left_mirrors_left_to_right() {
    let ltr = generate_positions(&LedTopology::Strip {
        count: 4,
        direction: StripDirection::LeftToRight,
    });
    let rtl = generate_positions(&LedTopology::Strip {
        count: 4,
        direction: StripDirection::RightToLeft,
    });
    assert_eq!(rtl.len(), 4);
    for (l, r) in ltr.iter().zip(rtl.iter().rev()) {
        assert!((l.x - r.x).abs() < f32::EPSILON);
        assert!((l.y - r.y).abs() < f32::EPSILON);
    }
}

#[test]
fn strip_top_to_bottom_positions() {
    let positions = generate_positions(&LedTopology::Strip {
        count: 3,
        direction: StripDirection::TopToBottom,
    });
    assert_eq!(positions.len(), 3);
    assert_eq!(positions[0], NormalizedPosition::new(0.5, 0.0));
    assert_eq!(positions[2], NormalizedPosition::new(0.5, 1.0));
}

#[test]
fn strip_single_led_at_center() {
    let positions = generate_positions(&LedTopology::Strip {
        count: 1,
        direction: StripDirection::LeftToRight,
    });
    assert_eq!(positions.len(), 1);
    assert!((positions[0].x - 0.5).abs() < f32::EPSILON);
    assert!((positions[0].y - 0.5).abs() < f32::EPSILON);
}

#[test]
fn matrix_2x2_top_left_positions() {
    let positions = generate_positions(&LedTopology::Matrix {
        width: 2,
        height: 2,
        serpentine: false,
        start_corner: Corner::TopLeft,
    });
    assert_eq!(positions.len(), 4);
    // Row 0: (0,0), (1,0). Row 1: (0,1), (1,1).
    assert_eq!(positions[0], NormalizedPosition::new(0.0, 0.0));
    assert_eq!(positions[1], NormalizedPosition::new(1.0, 0.0));
    assert_eq!(positions[2], NormalizedPosition::new(0.0, 1.0));
    assert_eq!(positions[3], NormalizedPosition::new(1.0, 1.0));
}

#[test]
fn matrix_2x2_bottom_right_mirrors() {
    let positions = generate_positions(&LedTopology::Matrix {
        width: 2,
        height: 2,
        serpentine: false,
        start_corner: Corner::BottomRight,
    });
    assert_eq!(positions.len(), 4);
    // BottomRight flips both axes.
    assert_eq!(positions[0], NormalizedPosition::new(1.0, 1.0));
    assert_eq!(positions[1], NormalizedPosition::new(0.0, 1.0));
    assert_eq!(positions[2], NormalizedPosition::new(1.0, 0.0));
    assert_eq!(positions[3], NormalizedPosition::new(0.0, 0.0));
}

#[test]
fn ring_positions_are_circular() {
    let count = 8;
    let positions = generate_positions(&LedTopology::Ring {
        count,
        start_angle: 0.0,
        direction: Winding::Clockwise,
    });
    assert_eq!(
        positions.len(),
        usize::try_from(count).expect("count fits in usize")
    );
    // All positions should be equidistant from center (0.5, 0.5).
    let center = NormalizedPosition::new(0.5, 0.5);
    let expected_radius = 0.45_f32;
    for pos in &positions {
        let dist = NormalizedPosition::distance(*pos, center);
        assert!(
            (dist - expected_radius).abs() < 1e-5,
            "Ring LED at ({}, {}) has distance {dist} from center, expected {expected_radius}",
            pos.x,
            pos.y,
        );
    }
}

#[test]
fn point_topology_single_center_led() {
    let positions = generate_positions(&LedTopology::Point);
    assert_eq!(positions.len(), 1);
    assert!((positions[0].x - 0.5).abs() < f32::EPSILON);
    assert!((positions[0].y - 0.5).abs() < f32::EPSILON);
}

#[test]
fn custom_topology_preserves_positions() {
    let custom = vec![
        NormalizedPosition::new(0.1, 0.2),
        NormalizedPosition::new(0.8, 0.9),
    ];
    let positions = generate_positions(&LedTopology::Custom {
        positions: custom.clone(),
    });
    assert_eq!(positions, custom);
}

#[test]
fn perimeter_loop_correct_count() {
    let positions = generate_positions(&LedTopology::PerimeterLoop {
        top: 5,
        right: 3,
        bottom: 5,
        left: 3,
        start_corner: Corner::TopLeft,
        direction: Winding::Clockwise,
    });
    assert_eq!(positions.len(), 16);
}

#[test]
fn concentric_rings_correct_count() {
    use hypercolor_types::spatial::RingDef;
    let positions = generate_positions(&LedTopology::ConcentricRings {
        rings: vec![
            RingDef {
                count: 12,
                radius: 1.0,
                start_angle: 0.0,
                direction: Winding::Clockwise,
            },
            RingDef {
                count: 4,
                radius: 0.5,
                start_angle: 0.0,
                direction: Winding::Clockwise,
            },
        ],
    });
    assert_eq!(positions.len(), 16);
}

// ── Solid Canvas Sampling ───────────────────────────────────────────────────

#[test]
fn solid_red_canvas_all_leds_get_red() {
    let red = Rgba::new(255, 0, 0, 255);
    let canvas = solid_canvas(32, 20, red);

    let zone = full_canvas_zone(
        "strip",
        LedTopology::Strip {
            count: 10,
            direction: StripDirection::LeftToRight,
        },
    );
    let layout = test_layout(vec![zone], 32, 20);
    let engine = SpatialEngine::new(layout);
    let result = engine.sample(&canvas);

    assert_eq!(result.len(), 1);
    assert_eq!(result[0].zone_id, "strip");
    assert_eq!(result[0].colors.len(), 10);
    for color in &result[0].colors {
        assert_eq!(color[0], 255, "Red channel should be 255");
        assert_eq!(color[1], 0, "Green channel should be 0");
        assert_eq!(color[2], 0, "Blue channel should be 0");
    }
}

#[test]
fn solid_white_canvas_matrix_all_white() {
    let white = Rgba::new(255, 255, 255, 255);
    let canvas = solid_canvas(32, 20, white);

    let zone = full_canvas_zone(
        "matrix",
        LedTopology::Matrix {
            width: 4,
            height: 3,
            serpentine: false,
            start_corner: Corner::TopLeft,
        },
    );
    let layout = test_layout(vec![zone], 32, 20);
    let engine = SpatialEngine::new(layout);
    let result = engine.sample(&canvas);

    assert_eq!(result.len(), 1);
    assert_eq!(result[0].colors.len(), 12);
    for color in &result[0].colors {
        assert_eq!(color, &[255, 255, 255]);
    }
}

// ── Horizontal Gradient Sampling ────────────────────────────────────────────

#[test]
fn horizontal_gradient_strip_samples_gradient() {
    let black = Rgba::new(0, 0, 0, 255);
    let white = Rgba::new(255, 255, 255, 255);
    let canvas = horizontal_gradient(256, 1, black, white);

    let zone = full_canvas_zone(
        "strip",
        LedTopology::Strip {
            count: 5,
            direction: StripDirection::LeftToRight,
        },
    );
    // Use nearest sampling for predictable integer results.
    let mut zone_with_nearest = zone;
    zone_with_nearest.sampling_mode = Some(SamplingMode::Nearest);

    let layout = test_layout(vec![zone_with_nearest], 256, 1);
    let engine = SpatialEngine::new(layout);
    let result = engine.sample(&canvas);

    let colors = &result[0].colors;
    assert_eq!(colors.len(), 5);

    // First LED at leftmost pixel: black.
    assert_eq!(colors[0][0], 0, "First LED should be black (r=0)");
    // Last LED at rightmost pixel: white.
    assert_eq!(colors[4][0], 255, "Last LED should be white (r=255)");
    // Middle LED at center: ~128.
    let mid_r = colors[2][0];
    assert!(
        (120..=135).contains(&mid_r),
        "Middle LED red channel should be ~128, got {mid_r}"
    );
    // Monotonically increasing from left to right.
    for i in 1..colors.len() {
        assert!(
            colors[i][0] >= colors[i - 1][0],
            "LED colors should increase left to right: {} < {} at index {i}",
            colors[i][0],
            colors[i - 1][0],
        );
    }
}

// ── Sampling Mode Tests ─────────────────────────────────────────────────────

#[test]
fn nearest_sampling_snaps_to_pixel() {
    // 4x1 canvas with distinct pixel colors.
    let mut canvas = Canvas::new(4, 1);
    canvas.set_pixel(0, 0, Rgba::new(255, 0, 0, 255));
    canvas.set_pixel(1, 0, Rgba::new(0, 255, 0, 255));
    canvas.set_pixel(2, 0, Rgba::new(0, 0, 255, 255));
    canvas.set_pixel(3, 0, Rgba::new(255, 255, 0, 255));

    let zone = custom_zone(
        "strip",
        LedTopology::Strip {
            count: 4,
            direction: StripDirection::LeftToRight,
        },
        NormalizedPosition::new(0.5, 0.5),
        NormalizedPosition::new(1.0, 1.0),
        Some(SamplingMode::Nearest),
    );

    let layout = test_layout(vec![zone], 4, 1);
    let engine = SpatialEngine::new(layout);
    let result = engine.sample(&canvas);

    let colors = &result[0].colors;
    assert_eq!(colors[0], [255, 0, 0], "LED 0 should snap to red pixel");
    assert_eq!(colors[1], [0, 255, 0], "LED 1 should snap to green pixel");
    assert_eq!(colors[2], [0, 0, 255], "LED 2 should snap to blue pixel");
    assert_eq!(
        colors[3],
        [255, 255, 0],
        "LED 3 should snap to yellow pixel"
    );
}

#[test]
fn bilinear_sampling_interpolates() {
    // 2x1 canvas: left=black, right=white.
    let mut canvas = Canvas::new(2, 1);
    canvas.set_pixel(0, 0, Rgba::new(0, 0, 0, 255));
    canvas.set_pixel(1, 0, Rgba::new(255, 255, 255, 255));

    // Single LED at center of canvas.
    let zone = custom_zone(
        "point",
        LedTopology::Point,
        NormalizedPosition::new(0.5, 0.5),
        NormalizedPosition::new(1.0, 1.0),
        Some(SamplingMode::Bilinear),
    );

    let layout = test_layout(vec![zone], 2, 1);
    let engine = SpatialEngine::new(layout);
    let result = engine.sample(&canvas);

    // At center of a 2-pixel canvas, linear-light bilinear interpolation lands at
    // 50% linear intensity, which re-encodes to ~188 in sRGB.
    let color = &result[0].colors[0];
    let expected_midpoint = linear_byte_to_srgb(128);
    assert!(
        (expected_midpoint.saturating_sub(1)..=expected_midpoint.saturating_add(1))
            .contains(&color[0]),
        "Bilinear center should be ~{expected_midpoint}, got {}",
        color[0]
    );
}

#[test]
fn area_average_samples_region() {
    // 4x4 canvas: top half red, bottom half blue.
    let mut canvas = Canvas::new(4, 4);
    let red = Rgba::new(255, 0, 0, 255);
    let blue = Rgba::new(0, 0, 255, 255);
    for y in 0..4 {
        for x in 0..4 {
            canvas.set_pixel(x, y, if y < 2 { red } else { blue });
        }
    }

    // Point zone at center with area average sampling.
    let zone = custom_zone(
        "bulb",
        LedTopology::Point,
        NormalizedPosition::new(0.5, 0.5),
        NormalizedPosition::new(1.0, 1.0),
        Some(SamplingMode::AreaAverage {
            radius_x: 2.0,
            radius_y: 2.0,
        }),
    );

    let layout = test_layout(vec![zone], 4, 4);
    let engine = SpatialEngine::new(layout);
    let result = engine.sample(&canvas);

    let color = &result[0].colors[0];
    let expected_red = linear_byte_to_srgb(153);
    let expected_blue = linear_byte_to_srgb(102);
    // The 5x5 kernel clamps at the edges, so this samples 60% red and 40% blue
    // in linear light, then re-encodes to sRGB.
    assert!(
        (expected_red.saturating_sub(2)..=expected_red.saturating_add(2)).contains(&color[0]),
        "Area avg red should be ~{expected_red}, got {}",
        color[0]
    );
    assert!(
        (expected_blue.saturating_sub(2)..=expected_blue.saturating_add(2)).contains(&color[2]),
        "Area avg blue should be ~{expected_blue}, got {}",
        color[2]
    );
}

#[test]
fn matrix_sampling_preserves_solid_color() {
    let canvas = solid_canvas(32, 20, Rgba::new(196, 124, 170, 255));
    let zone = full_canvas_zone(
        "keyboard",
        LedTopology::Matrix {
            width: 16,
            height: 6,
            serpentine: false,
            start_corner: Corner::TopLeft,
        },
    );

    let layout = test_layout(vec![zone], 32, 20);
    let engine = SpatialEngine::new(layout);
    let result = engine.sample(&canvas);
    let color = &result[0].colors[0];

    assert_eq!(color, &[196, 124, 170]);
}

#[test]
fn matrix_sampling_leaves_neutral_grays_alone() {
    let canvas = solid_canvas(32, 20, Rgba::new(128, 128, 128, 255));
    let zone = full_canvas_zone(
        "keyboard-gray",
        LedTopology::Matrix {
            width: 16,
            height: 6,
            serpentine: false,
            start_corner: Corner::TopLeft,
        },
    );

    let layout = test_layout(vec![zone], 32, 20);
    let engine = SpatialEngine::new(layout);
    let result = engine.sample(&canvas);
    let color = &result[0].colors[0];

    assert!(
        (i16::from(color[0]) - 128).unsigned_abs() <= 1
            && (i16::from(color[1]) - 128).unsigned_abs() <= 1
            && (i16::from(color[2]) - 128).unsigned_abs() <= 1,
        "neutral gray should remain neutral, got {color:?}"
    );
}

// ── Multi-Zone Tests ────────────────────────────────────────────────────────

#[test]
fn multiple_zones_produce_separate_results() {
    let canvas = solid_canvas(32, 20, Rgba::new(100, 150, 200, 255));

    let zone1 = full_canvas_zone(
        "strip1",
        LedTopology::Strip {
            count: 5,
            direction: StripDirection::LeftToRight,
        },
    );
    let zone2 = full_canvas_zone(
        "ring1",
        LedTopology::Ring {
            count: 8,
            start_angle: 0.0,
            direction: Winding::Clockwise,
        },
    );

    let layout = test_layout(vec![zone1, zone2], 32, 20);
    let engine = SpatialEngine::new(layout);
    let result = engine.sample(&canvas);

    assert_eq!(result.len(), 2, "Should produce one ZoneColors per zone");
    assert_eq!(result[0].zone_id, "strip1");
    assert_eq!(result[0].colors.len(), 5);
    assert_eq!(result[1].zone_id, "ring1");
    assert_eq!(result[1].colors.len(), 8);
}

#[test]
fn display_viewport_zones_are_skipped_by_spatial_sampling() {
    let canvas = solid_canvas(32, 20, Rgba::new(100, 150, 200, 255));

    let led_zone = full_canvas_zone(
        "strip1",
        LedTopology::Strip {
            count: 5,
            direction: StripDirection::LeftToRight,
        },
    );
    let mut display_zone = full_canvas_zone(
        "display",
        LedTopology::Matrix {
            width: 480,
            height: 480,
            serpentine: false,
            start_corner: Corner::TopLeft,
        },
    );
    display_zone.zone_name = Some("Display".to_owned());

    let layout = test_layout(vec![led_zone, display_zone], 32, 20);
    let engine = SpatialEngine::new(layout);
    let result = engine.sample(&canvas);

    assert_eq!(
        result.len(),
        1,
        "display viewport zones should not be sampled"
    );
    assert_eq!(result[0].zone_id, "strip1");
    assert_eq!(result[0].colors.len(), 5);
    assert_eq!(result[0].colors[0], [100, 150, 200]);
}

#[test]
fn spatial_engine_exposes_prepared_sampling_plan() {
    let zone = full_canvas_zone(
        "strip",
        LedTopology::Strip {
            count: 4,
            direction: StripDirection::LeftToRight,
        },
    );
    let engine = SpatialEngine::new(test_layout(vec![zone], 100, 20));

    let plan = engine.sampling_plan();
    assert_eq!(plan.len(), 1);
    assert_eq!(plan[0].zone_id, "strip");
    assert_eq!(plan[0].prepared_canvas_width, 100);
    assert_eq!(plan[0].prepared_canvas_height, 20);
    match &plan[0].prepared_samples {
        PreparedZoneSamples::Bilinear(samples) => assert_eq!(samples.len(), 4),
        other => panic!("expected bilinear prepared samples, got {other:?}"),
    }
}

// ── Layout Update Tests ─────────────────────────────────────────────────────

#[test]
fn update_layout_recomputes_positions() {
    let canvas = solid_canvas(32, 20, Rgba::new(50, 100, 150, 255));

    let zone = full_canvas_zone(
        "strip",
        LedTopology::Strip {
            count: 3,
            direction: StripDirection::LeftToRight,
        },
    );
    let layout = test_layout(vec![zone], 32, 20);
    let mut engine = SpatialEngine::new(layout);

    let result1 = engine.sample(&canvas);
    assert_eq!(result1[0].colors.len(), 3);

    // Update to a larger strip.
    let new_zone = full_canvas_zone(
        "strip",
        LedTopology::Strip {
            count: 10,
            direction: StripDirection::LeftToRight,
        },
    );
    let new_layout = test_layout(vec![new_zone], 32, 20);
    engine.update_layout(new_layout);

    let result2 = engine.sample(&canvas);
    assert_eq!(
        result2[0].colors.len(),
        10,
        "Should reflect new LED count after update"
    );
}

#[test]
fn update_layout_recomputes_cached_sample_positions() {
    let black = Rgba::new(0, 0, 0, 255);
    let white = Rgba::new(255, 255, 255, 255);
    let canvas = horizontal_gradient(100, 1, black, white);

    let left_zone = custom_zone(
        "moving_strip",
        LedTopology::Strip {
            count: 3,
            direction: StripDirection::LeftToRight,
        },
        NormalizedPosition::new(0.125, 0.5),
        NormalizedPosition::new(0.25, 1.0),
        Some(SamplingMode::Nearest),
    );
    let mut engine = SpatialEngine::new(test_layout(vec![left_zone], 100, 1));

    let left_colors = &engine.sample(&canvas)[0].colors;
    for color in left_colors {
        assert!(
            color[0] < 80,
            "LED in left quarter should be dark before layout update, got r={}",
            color[0]
        );
    }

    let right_zone = custom_zone(
        "moving_strip",
        LedTopology::Strip {
            count: 3,
            direction: StripDirection::LeftToRight,
        },
        NormalizedPosition::new(0.875, 0.5),
        NormalizedPosition::new(0.25, 1.0),
        Some(SamplingMode::Nearest),
    );
    engine.update_layout(test_layout(vec![right_zone], 100, 1));

    let right_colors = &engine.sample(&canvas)[0].colors;
    for color in right_colors {
        assert!(
            color[0] > 175,
            "LED in right quarter should be bright after layout update, got r={}",
            color[0]
        );
    }
}

#[test]
fn sample_into_reuses_zone_and_color_buffers() {
    let canvas_a = solid_canvas(32, 20, Rgba::new(10, 20, 30, 255));
    let canvas_b = solid_canvas(32, 20, Rgba::new(40, 50, 60, 255));
    let layout = test_layout(
        vec![
            full_canvas_zone(
                "strip1",
                LedTopology::Strip {
                    count: 5,
                    direction: StripDirection::LeftToRight,
                },
            ),
            full_canvas_zone(
                "strip2",
                LedTopology::Strip {
                    count: 8,
                    direction: StripDirection::LeftToRight,
                },
            ),
        ],
        32,
        20,
    );
    let engine = SpatialEngine::new(layout);
    let mut zones = Vec::<ZoneColors>::new();

    engine.sample_into(&canvas_a, &mut zones);

    let zones_ptr = zones.as_ptr();
    let color_ptrs = zones
        .iter()
        .map(|zone| zone.colors.as_ptr())
        .collect::<Vec<_>>();

    engine.sample_into(&canvas_b, &mut zones);

    assert_eq!(zones.as_ptr(), zones_ptr);
    assert_eq!(zones.len(), 2);
    assert_eq!(zones[0].colors, vec![[40, 50, 60]; 5]);
    assert_eq!(zones[1].colors, vec![[40, 50, 60]; 8]);

    for (zone, color_ptr) in zones.iter().zip(color_ptrs) {
        assert_eq!(zone.colors.as_ptr(), color_ptr);
    }
}

#[test]
fn append_sample_into_appends_zone_samples() {
    let canvas = solid_canvas(32, 20, Rgba::new(40, 50, 60, 255));
    let layout = test_layout(
        vec![
            full_canvas_zone(
                "strip1",
                LedTopology::Strip {
                    count: 5,
                    direction: StripDirection::LeftToRight,
                },
            ),
            full_canvas_zone(
                "strip2",
                LedTopology::Strip {
                    count: 8,
                    direction: StripDirection::LeftToRight,
                },
            ),
        ],
        32,
        20,
    );
    let engine = SpatialEngine::new(layout);
    let mut zones = vec![ZoneColors {
        zone_id: "existing".into(),
        colors: vec![[1, 2, 3]],
    }];

    engine.append_sample_into(&canvas, &mut zones);

    assert_eq!(zones.len(), 3);
    assert_eq!(zones[0].zone_id, "existing");
    assert_eq!(zones[1].colors, vec![[40, 50, 60]; 5]);
    assert_eq!(zones[2].colors, vec![[40, 50, 60]; 8]);
}

#[test]
fn sample_append_into_at_reuses_existing_zone_buffers() {
    let canvas = solid_canvas(32, 20, Rgba::new(10, 20, 30, 255));
    let layout = test_layout(
        vec![
            full_canvas_zone(
                "strip1",
                LedTopology::Strip {
                    count: 5,
                    direction: StripDirection::LeftToRight,
                },
            ),
            full_canvas_zone(
                "strip2",
                LedTopology::Strip {
                    count: 8,
                    direction: StripDirection::LeftToRight,
                },
            ),
        ],
        32,
        20,
    );
    let engine = SpatialEngine::new(layout);
    let mut zones = vec![
        ZoneColors {
            zone_id: "existing".into(),
            colors: vec![[1, 2, 3]],
        },
        ZoneColors {
            zone_id: "strip1".into(),
            colors: vec![[0, 0, 0]; 5],
        },
        ZoneColors {
            zone_id: "strip2".into(),
            colors: vec![[0, 0, 0]; 8],
        },
    ];
    let strip1_ptr = zones[1].colors.as_ptr();
    let strip2_ptr = zones[2].colors.as_ptr();

    let next_index = engine.sample_append_into_at(&canvas, &mut zones, 1);

    assert_eq!(next_index, 3);
    assert_eq!(zones[1].colors, vec![[10, 20, 30]; 5]);
    assert_eq!(zones[2].colors, vec![[10, 20, 30]; 8]);
    assert_eq!(zones[1].colors.as_ptr(), strip1_ptr);
    assert_eq!(zones[2].colors.as_ptr(), strip2_ptr);
}

// ── Empty Layout ────────────────────────────────────────────────────────────

#[test]
fn empty_layout_produces_empty_results() {
    let canvas = solid_canvas(32, 20, Rgba::new(255, 0, 0, 255));
    let layout = test_layout(vec![], 32, 20);
    let engine = SpatialEngine::new(layout);
    let result = engine.sample(&canvas);
    assert!(result.is_empty());
}

// ── Zero-LED Zone ───────────────────────────────────────────────────────────

#[test]
fn zero_led_strip_produces_empty_colors() {
    let canvas = solid_canvas(32, 20, Rgba::new(255, 0, 0, 255));
    let zone = full_canvas_zone(
        "empty_strip",
        LedTopology::Strip {
            count: 0,
            direction: StripDirection::LeftToRight,
        },
    );
    let layout = test_layout(vec![zone], 32, 20);
    let engine = SpatialEngine::new(layout);
    let result = engine.sample(&canvas);
    assert_eq!(result.len(), 1);
    assert!(result[0].colors.is_empty());
}

#[test]
fn bilinear_sampling_blends_corner_channels_in_linear_space() {
    let mut canvas = Canvas::new(2, 2);
    canvas.set_pixel(0, 0, Rgba::new(255, 0, 0, 255));
    canvas.set_pixel(1, 0, Rgba::new(0, 255, 0, 255));
    canvas.set_pixel(0, 1, Rgba::new(0, 0, 255, 255));
    canvas.set_pixel(1, 1, Rgba::new(255, 255, 255, 255));

    let zone = full_canvas_zone("point", LedTopology::Point);
    let color = sample_led(
        &canvas,
        NormalizedPosition::new(0.25, 0.75),
        &zone,
        &SamplingMode::Bilinear,
        EdgeBehavior::Clamp,
    );

    assert_eq!(color.r, expected_bilinear_channel(255, 0, 0, 255, 64, 192));
    assert_eq!(color.g, expected_bilinear_channel(0, 255, 0, 255, 64, 192));
    assert_eq!(color.b, expected_bilinear_channel(0, 0, 255, 255, 64, 192));
    assert_eq!(color.a, 255);
}

// ── Zone Positioning ────────────────────────────────────────────────────────

#[test]
fn zone_positioned_at_left_half_samples_left_side() {
    let black = Rgba::new(0, 0, 0, 255);
    let white = Rgba::new(255, 255, 255, 255);
    let canvas = horizontal_gradient(100, 1, black, white);

    // Zone covering only the left quarter of the canvas.
    let zone = custom_zone(
        "left_strip",
        LedTopology::Strip {
            count: 3,
            direction: StripDirection::LeftToRight,
        },
        NormalizedPosition::new(0.125, 0.5), // center at 12.5%
        NormalizedPosition::new(0.25, 1.0),  // width = 25% of canvas
        Some(SamplingMode::Nearest),
    );

    let layout = test_layout(vec![zone], 100, 1);
    let engine = SpatialEngine::new(layout);
    let result = engine.sample(&canvas);

    let colors = &result[0].colors;
    // All LEDs should be in the dark (left) portion of the gradient.
    for color in colors {
        assert!(
            color[0] < 80,
            "LED in left quarter should be dark, got r={}",
            color[0]
        );
    }
}
