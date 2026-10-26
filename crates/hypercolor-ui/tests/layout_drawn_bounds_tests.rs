//! Boxes stay on the canvas by the bounds they draw: rotation turns them
//! in pixels on a canvas that is rarely square, scale grows them, and a
//! circle draws square in pixels.

use std::collections::HashSet;
use std::f32::consts::{FRAC_PI_2, FRAC_PI_4};

use hypercolor_types::spatial::{
    EdgeBehavior, LedTopology, NormalizedPosition, Output, SamplingMode, SpatialLayout,
    StripDirection, Winding, ZoneShape,
};
use hypercolor_ui::layout_geometry;

const RIG_CANVAS: (u32, u32) = (640, 630);
const WIDE_CANVAS: (u32, u32) = (640, 480);

fn output(id: &str, position: (f32, f32), size: (f32, f32), rotation: f32) -> Output {
    Output {
        id: id.to_owned(),
        name: id.to_owned(),
        device_id: format!("dev:{id}"),
        zone_name: None,
        position: NormalizedPosition::new(position.0, position.1),
        size: NormalizedPosition::new(size.0, size.1),
        rotation,
        scale: 1.0,
        display_order: 0,
        orientation: None,
        topology: LedTopology::Custom {
            positions: Vec::new(),
        },
        led_positions: Vec::new(),
        led_mapping: None,
        sampling_mode: None,
        edge_behavior: None,
        shape: Some(ZoneShape::Rectangle),
        shape_preset: None,
        attachment: None,
        brightness: None,
    }
}

fn layout(canvas: (u32, u32), zones: Vec<Output>) -> SpatialLayout {
    SpatialLayout {
        id: "rig".to_owned(),
        name: "Rig".to_owned(),
        description: None,
        canvas_width: canvas.0,
        canvas_height: canvas.1,
        zones,
        default_sampling_mode: SamplingMode::Bilinear,
        default_edge_behavior: EdgeBehavior::Clamp,
        version: 1,
    }
}

/// The rig's rear fan: a wide box turned three quarters, so it draws tall
/// and thin against the right edge.
fn rear_fan() -> Output {
    output("rear", (0.92887, 0.66454), (0.25105, 0.08493), 4.712_389)
}

fn position_of(layout: &SpatialLayout, id: &str) -> NormalizedPosition {
    layout
        .zones
        .iter()
        .find(|zone| zone.id == id)
        .map(|zone| zone.position)
        .expect("zone")
}

fn approx(actual: f32, expected: f32) -> bool {
    (actual - expected).abs() < 1e-4
}

#[test]
fn a_quarter_turned_box_at_the_right_edge_survives_a_vertical_drag() {
    let mut rig = layout(RIG_CANVAS, vec![rear_fan()]);

    assert!(layout_geometry::drag_zone_to_position(
        &mut rig,
        "rear",
        NormalizedPosition::new(0.92887, 0.7),
    ));

    let moved = position_of(&rig, "rear");
    assert!(approx(moved.x, 0.92887), "{moved:?}");
    assert!(approx(moved.y, 0.7), "{moved:?}");
}

#[test]
fn a_quarter_turned_box_keeps_its_place_in_a_vertical_group_drag() {
    let mut rig = layout(
        RIG_CANVAS,
        vec![
            rear_fan(),
            output("bottom", (0.76987, 0.5), (0.25105, 0.08493), 0.0),
        ],
    );
    let initial = vec![
        ("rear".to_owned(), NormalizedPosition::new(0.92887, 0.66454)),
        ("bottom".to_owned(), NormalizedPosition::new(0.76987, 0.5)),
    ];

    layout_geometry::translate_zones(&mut rig, &initial, NormalizedPosition::new(0.0, 0.05));

    let rear = position_of(&rig, "rear");
    assert!(approx(rear.x, 0.92887), "{rear:?}");
    assert!(approx(rear.y, 0.71454), "{rear:?}");
    assert!(approx(position_of(&rig, "bottom").x, 0.76987));
}

#[test]
fn a_quarter_turned_box_stops_where_its_drawn_edge_meets_the_canvas() {
    let mut rig = layout(RIG_CANVAS, vec![rear_fan()]);
    let aspect = 640.0 / 630.0;

    layout_geometry::drag_zone_to_position(&mut rig, "rear", NormalizedPosition::new(1.0, 1.0));

    // Turned, the box is 0.08493 of the height wide in pixels and 0.25105
    // of the width tall in pixels.
    let half_width = 0.08493 / aspect * 0.5;
    let half_height = 0.25105 * aspect * 0.5;
    let stopped = position_of(&rig, "rear");
    assert!(approx(stopped.x, 1.0 - half_width), "{stopped:?}");
    assert!(approx(stopped.y, 1.0 - half_height), "{stopped:?}");
}

#[test]
fn an_unrotated_box_still_clamps_by_its_size() {
    let mut rig = layout(
        RIG_CANVAS,
        vec![output("bar", (0.5, 0.5), (0.25, 0.1), 0.0)],
    );

    layout_geometry::drag_zone_to_position(&mut rig, "bar", NormalizedPosition::new(0.99, 0.01));

    let stopped = position_of(&rig, "bar");
    assert!(approx(stopped.x, 0.875), "{stopped:?}");
    assert!(approx(stopped.y, 0.05), "{stopped:?}");
}

#[test]
fn a_45_degree_box_clamps_by_its_rotated_bounds() {
    let aspect = 640.0 / 480.0;
    let mut rig = layout(
        WIDE_CANVAS,
        vec![output("tilted", (0.5, 0.5), (0.3, 0.1), FRAC_PI_4)],
    );

    layout_geometry::drag_zone_to_position(&mut rig, "tilted", NormalizedPosition::new(1.0, 0.0));

    // In pixels the box is 0.4 by 0.1 of the height; turned 45 degrees,
    // each half extent is (0.2 + 0.05) * cos 45.
    let half_extent = (0.2 + 0.05) * FRAC_PI_4.cos();
    let stopped = position_of(&rig, "tilted");
    assert!(approx(stopped.x, 1.0 - half_extent / aspect), "{stopped:?}");
    assert!(approx(stopped.y, half_extent), "{stopped:?}");

    let extent = layout_geometry::drawn_extent(&rig.zones[0], aspect);
    assert!(approx(extent.x, 2.0 * half_extent / aspect), "{extent:?}");
    assert!(approx(extent.y, 2.0 * half_extent), "{extent:?}");
}

#[test]
fn scale_grows_the_bounds_a_box_keeps_on_the_canvas() {
    let mut scaled = output("big", (0.5, 0.5), (0.2, 0.1), 0.0);
    scaled.scale = 2.0;
    let mut rig = layout(RIG_CANVAS, vec![scaled]);

    layout_geometry::drag_zone_to_position(&mut rig, "big", NormalizedPosition::new(1.0, 0.5));

    assert!(approx(position_of(&rig, "big").x, 0.8));
}

#[test]
fn a_circle_keeps_its_bounds_at_any_rotation() {
    let aspect = 640.0 / 480.0;
    let mut ring = output("fan", (0.5, 0.5), (0.2, 0.2 * aspect), FRAC_PI_4);
    ring.shape = Some(ZoneShape::Ring);
    ring.topology = LedTopology::Ring {
        count: 24,
        start_angle: 0.0,
        direction: Winding::Clockwise,
    };

    let extent = layout_geometry::drawn_extent(&ring, aspect);

    assert!(approx(extent.x, 0.2), "{extent:?}");
    assert!(approx(extent.y, 0.2 * aspect), "{extent:?}");
}

#[test]
fn rotating_a_group_keeps_each_box_on_the_canvas_as_it_will_draw() {
    let aspect = 640.0 / 480.0;
    let mut rig = layout(
        WIDE_CANVAS,
        vec![
            output("bar", (0.5, 0.95), (0.5, 0.05), 0.0),
            output("twin", (0.5, 0.95), (0.5, 0.05), 0.0),
        ],
    );
    let ids: HashSet<String> = ["bar", "twin"].iter().map(|id| (*id).to_owned()).collect();

    assert!(layout_geometry::rotate_compound(&mut rig, &ids, FRAC_PI_2));

    // A quarter turn stands the bar up: 0.5 of the width becomes its
    // pixel height, so its center must rise to keep its foot on canvas.
    let half_height = 0.5 * aspect * 0.5;
    let bar = position_of(&rig, "bar");
    assert!(approx(bar.y, 1.0 - half_height), "{bar:?}");
}

#[test]
fn group_bounds_wrap_the_boxes_as_drawn() {
    let aspect = 640.0 / 630.0;
    let rig = layout(
        RIG_CANVAS,
        vec![rear_fan(), output("bottom", (0.5, 0.5), (0.2, 0.1), 0.0)],
    );
    let ids: HashSet<String> = ["rear", "bottom"]
        .iter()
        .map(|id| (*id).to_owned())
        .collect();

    let bounds = layout_geometry::compound_bounding_box(&rig, &ids).expect("bounds");

    let right = bounds.center.x + bounds.size.x * 0.5;
    assert!(
        approx(right, 0.92887 + 0.08493 / aspect * 0.5),
        "{bounds:?}"
    );
    let bottom = bounds.center.y + bounds.size.y * 0.5;
    assert!(
        approx(bottom, 0.66454 + 0.25105 * aspect * 0.5),
        "{bounds:?}"
    );
}

#[test]
fn a_strip_beside_an_edge_keeps_its_length_through_editor_normalization() {
    // A strip turned to stand vertically can sit right at the right edge;
    // its length runs along its own axis, not toward that edge.
    let size = layout_geometry::normalize_zone_size_for_editor(
        NormalizedPosition::new(0.98, 0.5),
        NormalizedPosition::new(0.3, 0.03),
        &LedTopology::Strip {
            count: 30,
            direction: StripDirection::LeftToRight,
        },
        None,
        640.0 / 480.0,
    );

    assert!(approx(size.x, 0.3), "{size:?}");
    assert!(size.x / size.y <= 8.01, "{size:?}");
}
