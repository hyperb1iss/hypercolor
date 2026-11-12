//! Where a layer's source lands on its zone's canvas.
//!
//! A layer with a scope fills the frame of its outputs, and its own
//! transform places the source inside that frame. One value,
//! `LayerPlacement`, carries the whole mapping: the compositor draws through
//! it and input inverts a touch through it. These tests hold it to the
//! placement math the LED sampler uses, to the transform math an unscoped
//! layer already draws with, and to itself in both directions.

use std::collections::HashMap;
use std::f32::consts::{FRAC_PI_2, FRAC_PI_4, PI};
use std::sync::Arc;

use hypercolor_core::spatial::{
    DeviceSurfaceMap, FrameEdge, LayerFrame, LayerFraming, LayerPlacement, SourceRepeat,
    SpatialEngine,
};
use hypercolor_types::device::DeviceId;
use hypercolor_types::effect::EffectId;
use hypercolor_types::layer::{LayerScope, LayerTransform, SceneLayer, SceneLayerId};
use hypercolor_types::scene::{Zone, ZoneId, ZoneRole};
use hypercolor_types::spatial::{
    Corner, EdgeBehavior, LedTopology, NormalizedPosition, Output, SamplingMode, SpatialLayout,
};
use hypercolor_types::viewport::FitMode;

const ZONE: (u32, u32) = (640, 480);

fn output(id: &str, position: (f32, f32), size: (f32, f32), rotation: f32) -> Output {
    Output {
        id: id.into(),
        name: id.into(),
        device_id: format!("mock:{id}"),
        zone_name: None,
        position: NormalizedPosition::new(position.0, position.1),
        size: NormalizedPosition::new(size.0, size.1),
        rotation,
        scale: 1.0,
        orientation: None,
        topology: LedTopology::Matrix {
            width: 5,
            height: 3,
            serpentine: false,
            start_corner: Corner::TopLeft,
        },
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

fn layout(zone: (u32, u32), zones: Vec<Output>) -> SpatialLayout {
    SpatialLayout {
        id: "zone".into(),
        name: "Zone".into(),
        description: None,
        canvas_width: zone.0,
        canvas_height: zone.1,
        zones,
        default_sampling_mode: SamplingMode::Bilinear,
        default_edge_behavior: EdgeBehavior::Clamp,
        version: 1,
    }
}

fn scope(ids: &[&str]) -> LayerScope {
    LayerScope {
        outputs: ids.iter().map(|id| (*id).to_owned()).collect(),
    }
}

fn zone(layout: SpatialLayout) -> Zone {
    Zone {
        id: ZoneId::new(),
        name: "Zone".into(),
        description: None,
        layers: Vec::new(),
        layout,
        brightness: 1.0,
        enabled: true,
        color: None,
        display_target: None,
        role: ZoneRole::Custom,
        controls_version: 0,
        layers_version: 0,
    }
}

fn layer(scope: Option<LayerScope>) -> SceneLayer {
    let mut layer = SceneLayer::from_effect(
        SceneLayerId::new(),
        EffectId::new(uuid::Uuid::now_v7()),
        HashMap::new(),
        HashMap::new(),
        None,
    );
    layer.scope = scope;
    layer
}

fn transform(anchor: (f32, f32), scale: (f32, f32), rotation: f32, fit: FitMode) -> LayerTransform {
    LayerTransform {
        anchor: NormalizedPosition::new(anchor.0, anchor.1),
        scale: [scale.0, scale.1],
        rotation,
        fit,
    }
}

fn assert_near(actual: [f32; 2], expected: [f32; 2], tolerance: f32, what: &str) {
    assert!(
        (actual[0] - expected[0]).abs() <= tolerance
            && (actual[1] - expected[1]).abs() <= tolerance,
        "{what}: expected {expected:?}, got {actual:?}"
    );
}

/// The zone pixel a point of an output's local box lands on, by the chain
/// the LED sampler places LEDs with: scale by the output's size, rotate in
/// normalized units, translate, then scale to pixels.
fn output_local_to_zone_px(output: &Output, local: [f32; 2], zone: (u32, u32)) -> [f32; 2] {
    let sx = (local[0] - 0.5) * output.size.x * output.scale;
    let sy = (local[1] - 0.5) * output.size.y * output.scale;
    let (sin, cos) = output.rotation.sin_cos();
    let nx = output.position.x + sx * cos - sy * sin;
    let ny = output.position.y + sx * sin + sy * cos;
    [nx * zone.0 as f32, ny * zone.1 as f32]
}

/// The source point a target pixel shows under the transform math an
/// unscoped layer draws with: anchor, scale, and rotation in target pixels,
/// then the fit. The second value is how far inside the drawn source the
/// pixel is, as a fraction of it; negative is outside.
fn transform_source_point(
    target_px: [f32; 2],
    transform: LayerTransform,
    source: (u32, u32),
    target: (u32, u32),
) -> (Option<[f32; 2]>, f32) {
    let (source_w, source_h) = (source.0 as f32, source.1 as f32);
    let (target_w, target_h) = (target.0 as f32, target.1 as f32);
    let (sin, cos) = transform.rotation.sin_cos();
    let dx = target_px[0] - transform.anchor.x * target_w;
    let dy = target_px[1] - transform.anchor.y * target_h;
    let local_x = (cos * dx + sin * dy) / transform.scale[0];
    let local_y = (-sin * dx + cos * dy) / transform.scale[1];
    let source_aspect = source_w / source_h;
    let target_aspect = target_w / target_h;
    let (draw, crop) = match transform.fit {
        FitMode::Stretch => ([target_w, target_h], [0.0, 0.0, source_w, source_h]),
        FitMode::Contain => (
            if target_aspect > source_aspect {
                [target_h * source_aspect, target_h]
            } else {
                [target_w, target_w / source_aspect]
            },
            [0.0, 0.0, source_w, source_h],
        ),
        FitMode::Cover => (
            [target_w, target_h],
            if target_aspect > source_aspect {
                let height = source_w / target_aspect;
                [0.0, (source_h - height) * 0.5, source_w, height]
            } else {
                let width = source_h * target_aspect;
                [(source_w - width) * 0.5, 0.0, width, source_h]
            },
        ),
        FitMode::Tile | FitMode::Mirror => unreachable!("repeat modes are checked on their own"),
    };
    let u = local_x / draw[0] + 0.5;
    let v = local_y / draw[1] + 0.5;
    let margin = u.min(1.0 - u).min(v).min(1.0 - v);
    (
        (margin >= 0.0).then(|| [crop[0] + u * crop[2], crop[1] + v * crop[3]]),
        margin,
    )
}

/// A small deterministic generator, so the property tests need no seed
/// plumbing and fail the same way every run.
struct Lcg(u64);

impl Lcg {
    fn unit(&mut self) -> f32 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((self.0 >> 40) as f32) / ((1_u64 << 24) as f32)
    }

    fn between(&mut self, low: f32, high: f32) -> f32 {
        low + (high - low) * self.unit()
    }

    fn pick<T: Copy>(&mut self, options: &[T]) -> T {
        let index = (self.unit() * options.len() as f32) as usize;
        options[index.min(options.len() - 1)]
    }
}

#[test]
fn an_unscoped_layer_with_no_transform_shows_each_zone_pixel_as_itself() {
    let placement = LayerPlacement::new(&LayerFrame::ZONE, LayerTransform::default(), ZONE, ZONE)
        .expect("the whole zone is a frame");

    assert_eq!(placement.repeat, SourceRepeat::None);
    for zone_px in [[0.5, 0.5], [320.0, 240.0], [639.5, 479.5], [17.25, 401.75]] {
        let source = placement.source_point(zone_px).expect("inside the zone");
        assert_near(source, zone_px, 1.0e-3, "identity placement");
    }
    assert_eq!(placement.source_texel([320.5, 240.5]), Some([320, 240]));
    assert_eq!(placement.edge, FrameEdge::Extend);
    assert_eq!(placement.frame_point([-1.0, 10.0]), None);
    assert_eq!(placement.frame_point([10.0, 480.5]), None);
}

#[test]
fn a_whole_zone_placement_equals_the_transform_an_unscoped_layer_draws_with() {
    let cases = [
        (
            transform((0.5, 0.5), (1.0, 1.0), 0.0, FitMode::Cover),
            (320, 200),
        ),
        (
            transform((0.3, 0.7), (0.5, 0.8), 0.6, FitMode::Stretch),
            (640, 480),
        ),
        (
            transform((0.5, 0.5), (1.5, 1.5), -FRAC_PI_4, FitMode::Contain),
            (200, 400),
        ),
        (
            transform((0.8, 0.2), (0.4, 1.2), FRAC_PI_2, FitMode::Cover),
            (1024, 256),
        ),
    ];
    for (layer_transform, source) in cases {
        let placement = LayerPlacement::new(&LayerFrame::ZONE, layer_transform, source, ZONE)
            .expect("placement");
        for y in (0..ZONE.1).step_by(37) {
            for x in (0..ZONE.0).step_by(41) {
                let zone_px = [x as f32 + 0.5, y as f32 + 0.5];
                let (expected, margin) =
                    transform_source_point(zone_px, layer_transform, source, ZONE);
                let actual = placement.source_point(zone_px);
                match (expected, actual) {
                    (Some(expected), Some(actual)) => {
                        assert_near(actual, expected, 2.0e-2, "whole-zone placement");
                    }
                    (None, None) => {}
                    // A pixel on the very edge of the drawn source may fall
                    // either side of it.
                    (expected, actual) => assert!(
                        margin.abs() < 1.0e-4,
                        "{layer_transform:?} at {zone_px:?}: expected {expected:?}, got {actual:?}"
                    ),
                }
            }
        }
    }
}

#[test]
fn a_framed_source_fills_its_outputs_rotated_rectangle_on_a_canvas_that_is_not_square() {
    let keyboard = output("keyboard", (0.4, 0.55), (0.5, 0.2), 0.7);
    let frame = LayerFrame::of_output(&keyboard).expect("placed output");
    let source = (640, 256);
    let placement = LayerPlacement::new(
        &frame,
        transform((0.5, 0.5), (1.0, 1.0), 0.0, FitMode::Stretch),
        source,
        ZONE,
    )
    .expect("placement");

    for local in [
        [0.0, 0.0],
        [1.0, 0.0],
        [0.0, 1.0],
        [1.0, 1.0],
        [0.5, 0.5],
        [0.2, 0.9],
    ] {
        let zone_px = output_local_to_zone_px(&keyboard, local, ZONE);
        let source_px = [local[0] * source.0 as f32, local[1] * source.1 as f32];
        let landed = placement.zone_point(source_px).expect("invertible");
        assert_near(landed, zone_px, 2.0e-2, "source corner on the canvas");
    }

    // A rotation in normalized units on a 4:3 canvas shears in pixels: the
    // frame's edges are not perpendicular there, and are not the extent
    // times the canvas size.
    let [width, height] = frame.pixel_size(ZONE).expect("frame size");
    let origin = output_local_to_zone_px(&keyboard, [0.0, 0.0], ZONE);
    let along_x = output_local_to_zone_px(&keyboard, [1.0, 0.0], ZONE);
    let along_y = output_local_to_zone_px(&keyboard, [0.0, 1.0], ZONE);
    let edge_x = [along_x[0] - origin[0], along_x[1] - origin[1]];
    let edge_y = [along_y[0] - origin[0], along_y[1] - origin[1]];
    assert!((edge_x[0].hypot(edge_x[1]) - width).abs() < 1.0e-2);
    assert!((edge_y[0].hypot(edge_y[1]) - height).abs() < 1.0e-2);
    assert!(
        (edge_x[0] * edge_y[0] + edge_x[1] * edge_y[1]).abs() > 1.0,
        "the placed frame should be sheared in pixels"
    );
    assert!((width - 0.5 * 640.0).abs() > 1.0);
}

#[test]
fn a_source_scaled_past_its_frame_shows_only_what_the_frame_holds() {
    let pad = output("pad", (0.5, 0.5), (0.25, 0.25), 0.0);
    let frame = LayerFrame::of_output(&pad).expect("placed output");
    let placement = LayerPlacement::new(
        &frame,
        transform((0.5, 0.5), (3.0, 3.0), 0.0, FitMode::Stretch),
        (64, 64),
        ZONE,
    )
    .expect("placement");

    // The source is three frames wide, so its middle third shows.
    let centre = placement
        .source_point([320.0, 240.0])
        .expect("frame centre");
    assert_near(centre, [32.0, 32.0], 1.0e-3, "frame centre");
    let inside_edge = placement.source_point([240.5, 240.0]).expect("just inside");
    assert!((inside_edge[0] - 21.4).abs() < 0.2, "{inside_edge:?}");
    // Beyond the frame the source has more pixels, and none of them show:
    // the frame's own edge runs on instead.
    assert_eq!(placement.frame_point([239.0, 240.0]), None);
    for outside in [[239.0, 240.0], [100.0, 240.0], [0.5, 240.0]] {
        let edge = placement.source_point(outside).expect("the edge runs on");
        assert_near(edge, [64.0 / 3.0, 32.0], 1.0e-2, "the frame's left edge");
    }
    let corner = placement
        .source_point([630.0, 5.0])
        .expect("the edge runs on");
    assert_near(
        corner,
        [64.0 * 2.0 / 3.0, 64.0 / 3.0],
        1.0e-2,
        "the frame's corner",
    );
}

/// A device's LEDs sit on the border of its rectangle, and the sampler
/// reads the pixel nearest each one, which can lie just outside the frame.
/// The frame's edge runs on, so every LED of the output reads the layer.
#[test]
fn every_led_of_a_framed_output_reads_the_layer() {
    let mut beyond_the_frame = 0;
    for zone in [(640, 480), (641, 479), (320, 200)] {
        for (position, size) in [((0.3, 0.4), (0.37, 0.23)), ((0.71, 0.62), (0.41, 0.33))] {
            let keys = output("keys", position, size, 0.0);
            let layout = layout(zone, vec![keys.clone()]);
            let plan = SpatialEngine::new(layout).sampling_plan();
            let frame = LayerFrame::of_output(&keys).expect("placed output");
            let source = frame.render_extent(zone).expect("extent");
            let placement = LayerPlacement::new(&frame, LayerTransform::default(), source, zone)
                .expect("placement");
            for sampled in &plan[0].sample_positions {
                let (pixel_x, pixel_y) = sampled.to_pixel(zone.0, zone.1);
                let pixel = [pixel_x.round() + 0.5, pixel_y.round() + 0.5];
                assert!(
                    placement.source_texel(pixel).is_some(),
                    "an LED of the output read no layer at {pixel:?}"
                );
                if placement.frame_point(pixel).is_none() {
                    beyond_the_frame += 1;
                }
            }
        }
    }
    assert!(
        beyond_the_frame > 0,
        "the cases should hold border LEDs that read past the frame"
    );
}

#[test]
fn a_window_shows_each_zone_pixel_as_itself_inside_its_frame_alone() {
    let pad = output("pad", (0.5, 0.5), (0.25, 0.5), 0.4);
    let frame = LayerFrame::of_output(&pad).expect("placed output");
    let window = LayerPlacement::window(&frame, ZONE).expect("window");

    assert_eq!(window.edge, FrameEdge::Clip);
    assert_eq!(window.source_size, [640, 480]);
    let centre = window
        .source_point([320.0, 240.0])
        .expect("the frame's centre");
    assert_near(centre, [320.0, 240.0], 1.0e-2, "window centre");
    assert_eq!(window.source_texel([300.5, 250.5]), Some([300, 250]));
    assert!(window.covers([320.0, 240.0]));
    // Outside the turned rectangle nothing shows.
    assert!(!window.covers([150.0, 240.0]));
    assert_eq!(window.source_point([150.0, 240.0]), None);
    assert_eq!(window.source_point([320.0, 20.0]), None);
    let [left, top, right, bottom] = window.frame_bounds().expect("bounds");
    assert!(left < 300.0 && right > 340.0 && top < 140.0 && bottom > 340.0);
}

#[test]
fn a_source_scaled_down_leaves_the_rest_of_its_frame_empty() {
    let pad = output("pad", (0.5, 0.5), (0.5, 0.5), 0.0);
    let frame = LayerFrame::of_output(&pad).expect("placed output");
    let placement = LayerPlacement::new(
        &frame,
        transform((0.5, 0.5), (0.5, 0.5), 0.0, FitMode::Stretch),
        (64, 64),
        ZONE,
    )
    .expect("placement");

    assert!(placement.source_point([320.0, 240.0]).is_some());
    // Inside the frame, outside the half-size source.
    assert!(placement.frame_point([170.0, 240.0]).is_some());
    assert_eq!(placement.source_point([170.0, 240.0]), None);
    assert_eq!(placement.source_texel([170.0, 240.0]), None);
}

#[test]
fn tiled_and_mirrored_sources_repeat_in_source_pixels() {
    let pad = output("pad", (0.5, 0.5), (0.5, 0.5), 0.0);
    let frame = LayerFrame::of_output(&pad).expect("placed output");
    let source = (16, 16);
    let tile = LayerPlacement::new(
        &frame,
        transform((0.0, 0.0), (1.0, 1.0), 0.0, FitMode::Tile),
        source,
        ZONE,
    )
    .expect("placement");
    assert_eq!(tile.repeat, SourceRepeat::Tile);
    // The frame starts at zone pixel (160, 120); one source pixel is one
    // frame pixel, and the source starts over every sixteen.
    assert_eq!(tile.source_texel([160.5, 120.5]), Some([0, 0]));
    assert_eq!(tile.source_texel([175.5, 120.5]), Some([15, 0]));
    assert_eq!(tile.source_texel([176.5, 137.5]), Some([0, 1]));
    assert_eq!(tile.frame_point([159.5, 120.5]), None);

    let mirror = LayerPlacement::new(
        &frame,
        transform((0.0, 0.0), (1.0, 1.0), 0.0, FitMode::Mirror),
        source,
        ZONE,
    )
    .expect("placement");
    assert_eq!(mirror.repeat, SourceRepeat::Mirror);
    assert_eq!(mirror.source_texel([175.5, 120.5]), Some([15, 0]));
    assert_eq!(mirror.source_texel([176.5, 120.5]), Some([15, 0]));
    assert_eq!(mirror.source_texel([191.5, 120.5]), Some([0, 0]));
    assert_eq!(mirror.source_texel([192.5, 120.5]), Some([0, 0]));
}

#[test]
fn several_outputs_share_the_bounding_box_of_their_placed_rectangles() {
    let left = output("left", (0.25, 0.5), (0.2, 0.2), 0.0);
    let right = output("right", (0.75, 0.4), (0.2, 0.4), FRAC_PI_2);
    let other = output("other", (0.5, 0.9), (1.0, 0.1), 0.0);
    let zone = zone(layout(ZONE, vec![left, other, right]));

    let frame = LayerFrame::of_scope(&zone, &scope(&["left", "right"])).expect("frame");

    // The right output is turned a quarter, so it spans 0.4 wide and 0.2
    // tall around (0.75, 0.4).
    assert!((frame.center.x - 0.55).abs() < 1.0e-5, "{frame:?}");
    assert!((frame.center.y - 0.45).abs() < 1.0e-5, "{frame:?}");
    assert!((frame.extent.x - 0.8).abs() < 1.0e-5, "{frame:?}");
    assert!((frame.extent.y - 0.3).abs() < 1.0e-5, "{frame:?}");
    assert!(frame.rotation.abs() < f32::EPSILON);

    // One source spans both: its left edge is on the left output's left
    // edge and its right edge on the right output's.
    let placement = LayerPlacement::new(
        &frame,
        transform((0.5, 0.5), (1.0, 1.0), 0.0, FitMode::Stretch),
        (640, 240),
        ZONE,
    )
    .expect("placement");
    let left_edge = placement.zone_point([0.0, 120.0]).expect("invertible");
    let right_edge = placement.zone_point([640.0, 120.0]).expect("invertible");
    assert_near(left_edge, [0.15 * 640.0, 0.45 * 480.0], 1.0e-2, "left edge");
    assert_near(
        right_edge,
        [0.95 * 640.0, 0.45 * 480.0],
        1.0e-2,
        "right edge",
    );
}

#[test]
fn a_scope_keeps_only_the_outputs_the_zone_places() {
    let keyboard = output("keyboard", (0.5, 0.5), (0.5, 0.2), 0.0);
    let zone = zone(layout(ZONE, vec![keyboard.clone()]));

    assert_eq!(LayerFraming::of(&zone, &layer(None)), LayerFraming::Zone);
    assert_eq!(
        LayerFraming::of(&zone, &layer(Some(scope(&["gone", "keyboard"])))),
        LayerFraming::Framed(LayerFrame::of_output(&keyboard).expect("placed output"))
    );
    let dormant = LayerFraming::of(&zone, &layer(Some(scope(&["gone"]))));
    assert!(dormant.is_dormant());
    assert_eq!(dormant.frame(), None);
    assert_eq!(dormant.render_extent(ZONE), None);
}

/// Dormancy is membership alone. A scope that names an output with no area
/// still frames its layer: on the outputs that have area when any does, and
/// otherwise on the collapsed outputs themselves, a pixel wide wherever
/// they have no extent.
#[test]
fn a_scope_naming_an_output_with_no_area_still_frames_its_layer() {
    let pixel = 1.0 / 640.0;
    let dot = output("dot", (0.3, 0.6), (0.0, 0.0), 0.0);
    let line = output("line", (0.5, 0.5), (0.5, 0.0), FRAC_PI_4);
    let keyboard = output("keyboard", (0.5, 0.5), (0.5, 0.2), 0.0);
    let mut unplaceable = output("unplaceable", (0.5, 0.5), (0.5, 0.5), 0.0);
    unplaceable.scale = f32::INFINITY;
    let zone = zone(layout(ZONE, vec![dot, line, keyboard.clone(), unplaceable]));
    let framing = |ids: &[&str]| LayerFraming::of(&zone, &layer(Some(scope(ids))));

    let point = framing(&["dot"]);
    assert!(!point.is_dormant());
    let frame = point.frame().expect("a point still has a frame");
    assert_eq!(frame.center, NormalizedPosition::new(0.3, 0.6));
    assert_eq!(frame.extent, NormalizedPosition::new(pixel, pixel));
    let source = point.render_extent(ZONE).expect("a point still renders");
    let placement = LayerPlacement::new(&frame, LayerTransform::default(), source, ZONE)
        .expect("a point still places its source");
    // Every LED of the point samples its centre, which shows the centre of
    // the source.
    #[allow(clippy::cast_precision_loss)]
    let source_centre = [source.0 as f32 * 0.5, source.1 as f32 * 0.5];
    assert_near(
        placement
            .source_point([0.3 * 640.0, 0.6 * 480.0])
            .expect("the centre shows the source"),
        source_centre,
        1.0e-2,
        "point centre",
    );

    // A strip with no thickness is framed along its own line.
    let strip = framing(&["line"])
        .frame()
        .expect("a line still has a frame");
    assert_eq!(strip.extent, NormalizedPosition::new(0.5, pixel));
    assert!((strip.rotation - FRAC_PI_4).abs() < f32::EPSILON);
    assert_eq!(framing(&["line"]).render_extent(ZONE), Some((640, 16)));

    // Beside an output that has area, a collapsed one does not move the
    // frame.
    assert_eq!(
        framing(&["dot", "keyboard", "line"]),
        LayerFraming::Framed(LayerFrame::of_output(&keyboard).expect("placed output"))
    );

    // Nothing to form a frame from: the source fills the zone canvas.
    assert_eq!(
        framing(&["unplaceable"]),
        LayerFraming::Framed(LayerFrame::ZONE)
    );
    assert!(framing(&["gone"]).is_dormant());
}

#[test]
fn a_framed_layer_renders_at_its_frames_aspect_on_the_zones_long_side() {
    let extent = |size: (f32, f32), rotation: f32, zone: (u32, u32)| {
        LayerFrame::of_output(&output("frame", (0.5, 0.5), size, rotation))
            .and_then(|frame| frame.render_extent(zone))
    };

    assert_eq!(LayerFraming::Zone.render_extent(ZONE), Some(ZONE));
    // A keyboard of aspect 4.4 in a 640 by 480 zone.
    assert_eq!(extent((0.55, 0.166_666_67), 0.0, ZONE), Some((640, 146)));
    // A frame that is square in pixels renders square, at the long side.
    assert_eq!(extent((0.3, 0.4), 0.0, ZONE), Some((640, 640)));
    // A tall frame keeps the long side on its height.
    assert_eq!(extent((0.1, 0.8), 0.0, ZONE), Some((106, 640)));
    // A turn keeps the frame's own axes, so on a square canvas it changes
    // nothing. On a 4:3 canvas the same box covers other pixels turned.
    assert_eq!(extent((0.6, 0.2), 0.0, (640, 640)), Some((640, 214)));
    assert_eq!(extent((0.6, 0.2), FRAC_PI_2, (640, 640)), Some((640, 214)));
    assert_eq!(extent((0.6, 0.2), 0.0, ZONE), Some((640, 160)));
    assert_eq!(extent((0.6, 0.2), FRAC_PI_2, ZONE), Some((640, 284)));
    // A sliver never renders under sixteen pixels on its short side.
    assert_eq!(extent((0.9, 0.01), 0.0, ZONE), Some((640, 16)));
    assert_eq!(extent((0.0, 0.5), 0.0, ZONE), None);
}

#[test]
fn degenerate_frames_and_sources_have_no_placement() {
    let frame = LayerFrame::of_output(&output("pad", (0.5, 0.5), (0.5, 0.5), 0.0)).expect("frame");
    assert!(LayerPlacement::new(&frame, LayerTransform::default(), (0, 16), ZONE).is_none());
    assert!(LayerPlacement::new(&frame, LayerTransform::default(), (16, 16), (0, 480)).is_none());
    assert!(LayerFrame::of_output(&output("flat", (0.5, 0.5), (0.5, 0.0), 0.0)).is_none());
    assert!(LayerFrame::of_output(&output("nan", (f32::NAN, 0.5), (0.5, 0.5), 0.0)).is_none());
}

/// For random frames, canvas aspects, and layer transforms, a source point
/// carried to the canvas and back through the placement is itself, and it
/// lands where the output's own placement chain puts it.
#[test]
fn random_placements_round_trip_source_points_through_the_canvas() {
    let mut random = Lcg(0x005e_ed95);
    for case in 0..2_000 {
        let zone = random.pick(&[(640, 480), (640, 630), (320, 200), (480, 480), (200, 800)]);
        let source = random.pick(&[(640, 146), (64, 640), (640, 640), (320, 200), (17, 9)]);
        let scoped = output(
            "scoped",
            (random.between(0.2, 0.8), random.between(0.2, 0.8)),
            (random.between(0.05, 0.9), random.between(0.05, 0.9)),
            random.between(-PI, PI),
        );
        let fit = random.pick(&[FitMode::Stretch, FitMode::Contain, FitMode::Cover]);
        let layer_transform = transform(
            (random.between(0.0, 1.0), random.between(0.0, 1.0)),
            (random.between(0.25, 3.0), random.between(0.25, 3.0)),
            random.between(-PI, PI),
            fit,
        );
        let frame = LayerFrame::of_output(&scoped).expect("placed output");
        let placement = LayerPlacement::new(&frame, layer_transform, source, zone)
            .unwrap_or_else(|| panic!("case {case}: placement"));
        let [crop_x, crop_y, crop_w, crop_h] = placement.source_crop;

        for _ in 0..8 {
            let source_px = [
                crop_x + crop_w * random.between(0.02, 0.98),
                crop_y + crop_h * random.between(0.02, 0.98),
            ];
            let zone_px = placement
                .zone_point(source_px)
                .unwrap_or_else(|| panic!("case {case}: invertible"));
            let frame_units = {
                let frame_point = [
                    placement.zone_to_frame[0] * zone_px[0]
                        + placement.zone_to_frame[1] * zone_px[1]
                        + placement.zone_to_frame[2],
                    placement.zone_to_frame[3] * zone_px[0]
                        + placement.zone_to_frame[4] * zone_px[1]
                        + placement.zone_to_frame[5],
                ];
                // The frame's own chain puts that frame point on the same
                // zone pixel the sampler would.
                let via_output = output_local_to_zone_px(&scoped, frame_point, zone);
                assert_near(via_output, zone_px, 5.0e-2, "frame point on the canvas");
                frame_point
            };
            // Inside the frame the pixel shows that source point. Beyond
            // it the frame's edge runs on, and the point is not shown.
            if frame_units
                .iter()
                .all(|unit| (0.001..=0.999).contains(unit))
            {
                let back = placement
                    .source_point(zone_px)
                    .unwrap_or_else(|| panic!("case {case}: {source_px:?} was not shown"));
                assert_near(back, source_px, 5.0e-2, "source point round trip");
            }
        }
    }
}

/// For random rotated outputs and layer transforms, the source point a touch
/// resolves to is the point whose pixels the LED under the finger samples:
/// the placement of that LED's own sampled canvas pixel.
#[test]
fn a_touch_resolves_to_the_source_point_its_led_samples() {
    let mut random = Lcg(0x00c0_ffee);
    let device = DeviceId::new();
    let other_device = DeviceId::new();
    let mut resolved = 0_u32;
    for case in 0..500 {
        let zone = random.pick(&[(640, 480), (640, 630), (320, 200)]);
        let scoped = output(
            "scoped",
            (random.between(0.3, 0.7), random.between(0.3, 0.7)),
            (random.between(0.2, 0.5), random.between(0.2, 0.5)),
            random.between(-PI, PI),
        );
        let outside = output("outside", (0.5, 0.5), (0.9, 0.9), 0.0);
        let layout = layout(zone, vec![scoped.clone(), outside]);
        let engine = SpatialEngine::new(layout.clone());
        let plan = engine.sampling_plan();
        let surfaces =
            DeviceSurfaceMap::from_layout(&layout, |layout_device_id| match layout_device_id {
                "mock:scoped" => Some(device),
                "mock:outside" => Some(other_device),
                _ => None,
            });
        let layer_scope = scope(&["scoped"]);
        let frame =
            LayerFrame::of_scope(&crate::zone(layout.clone()), &layer_scope).expect("frame");
        let source = frame.render_extent(zone).expect("extent");
        let layer_transform = transform(
            (random.between(0.3, 0.7), random.between(0.3, 0.7)),
            (random.between(0.6, 1.6), random.between(0.6, 1.6)),
            random.between(-PI, PI),
            random.pick(&[FitMode::Stretch, FitMode::Contain, FitMode::Cover]),
        );
        let placement = LayerPlacement::new(&frame, layer_transform, source, zone)
            .unwrap_or_else(|| panic!("case {case}: placement"));
        let layer_surfaces =
            surfaces.for_layer(placement, zone, Some(Arc::new(layer_scope.clone())));

        // The matrix is 5 by 3, row-major from the top left.
        for (led, sampled) in plan[0].sample_positions.iter().enumerate() {
            let surface_x = (led % 5) as f32 / 4.0;
            let surface_y = (led / 5) as f32 / 2.0;
            let (pixel_x, pixel_y) = sampled.to_pixel(zone.0, zone.1);
            let expected = placement.source_point([pixel_x + 0.5, pixel_y + 0.5]);
            let touched =
                layer_surfaces.segment_source_position(device, None, surface_x, surface_y);
            match (expected, touched) {
                (Some(expected), Some(touched)) => {
                    resolved += 1;
                    // A normalized position spans the centres of the first
                    // and last pixel, so a point in the outer half pixel
                    // reports the centre of the edge pixel.
                    let expected = [
                        expected[0].clamp(0.5, source.0 as f32 - 0.5),
                        expected[1].clamp(0.5, source.1 as f32 - 0.5),
                    ];
                    let (touched_x, touched_y) = touched.to_pixel(source.0, source.1);
                    assert_near(
                        [touched_x + 0.5, touched_y + 0.5],
                        expected,
                        5.0e-2,
                        "touch on the source",
                    );
                }
                (None, None) => {}
                other => panic!("case {case} led {led}: placement and touch disagree: {other:?}"),
            }
            // The other device's output is outside the scope.
            assert_eq!(
                layer_surfaces.segment_source_position(other_device, None, surface_x, surface_y),
                None
            );
            // The zone's own map still reports the canvas point.
            assert!(
                surfaces
                    .canvas_position(device, surface_x, surface_y)
                    .is_some()
            );
        }
    }
    assert!(
        resolved > 2_000,
        "only {resolved} touches landed on a source"
    );
}

#[test]
fn an_unscoped_layer_with_a_transform_gets_the_point_its_transform_draws() {
    let device = DeviceId::new();
    let pad = output("pad", (0.5, 0.5), (0.6, 0.6), 0.0);
    let layout = layout(ZONE, vec![pad]);
    let surfaces = DeviceSurfaceMap::from_layout(&layout, |_| Some(device));
    let layer_transform = transform((0.25, 0.5), (0.6, 0.6), 0.0, FitMode::Stretch);
    let placement =
        LayerPlacement::new(&LayerFrame::ZONE, layer_transform, ZONE, ZONE).expect("placement");
    let layer_surfaces = surfaces.for_layer(placement, ZONE, None);

    // The pad's centre samples the canvas centre. The layer is drawn at
    // six tenths of its size around (160, 240), so canvas x 320 is near its
    // right edge: the published canvas point, 0.5, would put the ripple in
    // the middle of the layer instead.
    let canvas = surfaces.canvas_position(device, 0.5, 0.5).expect("placed");
    assert!((canvas.x - 0.5).abs() < 1.0e-5 && (canvas.y - 0.5).abs() < 1.0e-5);
    let source = layer_surfaces
        .segment_source_position(device, None, 0.5, 0.5)
        .expect("under the layer");
    let (source_x, source_y) = source.to_pixel(ZONE.0, ZONE.1);
    let expected = transform_source_point(
        [canvas.x * 639.0 + 0.5, canvas.y * 479.0 + 0.5],
        layer_transform,
        ZONE,
        ZONE,
    )
    .0
    .expect("under the layer");
    assert_near(
        [source_x + 0.5, source_y + 0.5],
        expected,
        5.0e-2,
        "corrected point",
    );
    assert!(source.x > 0.9, "the layer's right edge is under the finger");

    // A finger past the layer's drawn edge is not on its source.
    assert_eq!(
        layer_surfaces.segment_source_position(device, None, 1.0, 0.5),
        None
    );
}
