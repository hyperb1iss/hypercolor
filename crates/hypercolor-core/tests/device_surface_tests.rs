//! Placement of device touch surfaces on a render canvas.
//!
//! A touch at a device-surface position must land on the canvas point that
//! the LED under the finger samples, through the same position, size,
//! rotation, scale, start corner, and edge behavior the sampler uses.

use std::f32::consts::FRAC_PI_2;

use hypercolor_core::device::BackendManager;
use hypercolor_core::spatial::{DISPLAY_ZONE_NAME, DeviceSurfaceMap, SpatialEngine};
use hypercolor_types::device::DeviceId;
use hypercolor_types::spatial::{
    Corner, EdgeBehavior, LedTopology, NormalizedPosition, Output, SamplingMode, SpatialLayout,
};

const EPSILON: f32 = 1.0e-5;

fn layout(zones: Vec<Output>) -> SpatialLayout {
    SpatialLayout {
        id: "touch".into(),
        name: "Touch".into(),
        description: None,
        canvas_width: 320,
        canvas_height: 200,
        zones,
        default_sampling_mode: SamplingMode::Bilinear,
        default_edge_behavior: EdgeBehavior::Clamp,
        version: 1,
    }
}

fn matrix(columns: u32, rows: u32, start_corner: Corner) -> LedTopology {
    LedTopology::Matrix {
        width: columns,
        height: rows,
        serpentine: false,
        start_corner,
    }
}

fn output(id: &str, device_id: &str, topology: LedTopology) -> Output {
    Output {
        id: id.into(),
        name: id.into(),
        device_id: device_id.into(),
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

fn placed(position: (f32, f32), size: (f32, f32), rotation: f32, topology: LedTopology) -> Output {
    Output {
        position: NormalizedPosition::new(position.0, position.1),
        size: NormalizedPosition::new(size.0, size.1),
        rotation,
        ..output("pad", "blocks:lightpad", topology)
    }
}

fn single_device(device: DeviceId) -> impl FnMut(&str) -> Option<DeviceId> {
    move |layout_device_id| (layout_device_id == "blocks:lightpad").then_some(device)
}

fn assert_close(actual: NormalizedPosition, expected: NormalizedPosition) {
    assert!(
        (actual.x - expected.x).abs() < EPSILON && (actual.y - expected.y).abs() < EPSILON,
        "expected {expected:?}, got {actual:?}"
    );
}

/// Device-surface position of LED `(column, row)` on a `columns`×`rows`
/// matrix, in the device's own row-major frame.
fn led_surface_position(column: u32, row: u32, columns: u32, rows: u32) -> (f32, f32) {
    let x = f32::from(u16::try_from(column).expect("small grid"))
        / f32::from(u16::try_from(columns - 1).expect("small grid"));
    let y = f32::from(u16::try_from(row).expect("small grid"))
        / f32::from(u16::try_from(rows - 1).expect("small grid"));
    (x, y)
}

/// Every LED's sampled canvas point must equal the touch mapped from the
/// surface point over that LED.
fn assert_touch_lands_on_sampled_leds(output: Output, columns: u32, rows: u32) {
    let device = DeviceId::new();
    let layout = layout(vec![output]);
    let engine = SpatialEngine::new(layout.clone());
    let plan = engine.sampling_plan();
    let sampled = &plan[0].sample_positions;
    let surfaces = DeviceSurfaceMap::from_layout(&layout, single_device(device));

    for row in 0..rows {
        for column in 0..columns {
            let index = usize::try_from(row * columns + column).expect("small grid");
            let (x, y) = led_surface_position(column, row, columns, rows);
            let touch = surfaces
                .canvas_position(device, x, y)
                .expect("placed device should map");
            assert_close(touch, sampled[index]);
        }
    }
}

#[test]
fn full_canvas_surface_maps_to_identical_canvas_coordinates() {
    let device = DeviceId::new();
    let layout = layout(vec![output(
        "pad",
        "blocks:lightpad",
        matrix(15, 15, Corner::TopLeft),
    )]);
    let surfaces = DeviceSurfaceMap::from_layout(&layout, single_device(device));

    assert_eq!(surfaces.len(), 1);
    assert!(surfaces.contains(device));
    assert_close(
        surfaces
            .canvas_position(device, 0.25, 0.75)
            .expect("placed device should map"),
        NormalizedPosition::new(0.25, 0.75),
    );
}

#[test]
fn surface_follows_output_position_and_size() {
    let device = DeviceId::new();
    let layout = layout(vec![placed(
        (0.75, 0.25),
        (0.5, 0.5),
        0.0,
        matrix(15, 15, Corner::TopLeft),
    )]);
    let surfaces = DeviceSurfaceMap::from_layout(&layout, single_device(device));

    assert_close(
        surfaces.canvas_position(device, 0.0, 0.0).expect("mapped"),
        NormalizedPosition::new(0.5, 0.0),
    );
    assert_close(
        surfaces.canvas_position(device, 1.0, 1.0).expect("mapped"),
        NormalizedPosition::new(1.0, 0.5),
    );
    assert_close(
        surfaces.canvas_position(device, 0.5, 0.5).expect("mapped"),
        NormalizedPosition::new(0.75, 0.25),
    );
}

#[test]
fn rotated_surface_turns_with_its_output() {
    let device = DeviceId::new();
    let layout = layout(vec![placed(
        (0.5, 0.5),
        (0.5, 0.5),
        FRAC_PI_2,
        matrix(15, 15, Corner::TopLeft),
    )]);
    let surfaces = DeviceSurfaceMap::from_layout(&layout, single_device(device));

    // A quarter turn swings the surface's right edge below the center.
    assert_close(
        surfaces.canvas_position(device, 1.0, 0.5).expect("mapped"),
        NormalizedPosition::new(0.5, 0.75),
    );
    // And its top edge to the right of the center.
    assert_close(
        surfaces.canvas_position(device, 0.5, 0.0).expect("mapped"),
        NormalizedPosition::new(0.75, 0.5),
    );
}

#[test]
fn touches_land_on_the_leds_they_cover_under_rotation_and_scale() {
    let mut rotated = placed((0.4, 0.6), (0.3, 0.45), 0.7, matrix(5, 4, Corner::TopLeft));
    rotated.scale = 1.3;
    assert_touch_lands_on_sampled_leds(rotated, 5, 4);
}

#[test]
fn touches_follow_the_matrix_start_corner() {
    for corner in [
        Corner::TopLeft,
        Corner::TopRight,
        Corner::BottomLeft,
        Corner::BottomRight,
    ] {
        assert_touch_lands_on_sampled_leds(
            placed((0.5, 0.5), (0.6, 0.6), -0.4, matrix(4, 3, corner)),
            4,
            3,
        );
    }
}

#[test]
fn surface_hanging_off_canvas_follows_output_edge_behavior() {
    let device = DeviceId::new();
    let clamped = layout(vec![placed(
        (1.0, 0.5),
        (0.5, 0.5),
        0.0,
        matrix(15, 15, Corner::TopLeft),
    )]);
    let surfaces = DeviceSurfaceMap::from_layout(&clamped, single_device(device));
    assert_close(
        surfaces.canvas_position(device, 1.0, 0.5).expect("mapped"),
        NormalizedPosition::new(1.0, 0.5),
    );

    let mut fading = placed((1.0, 0.5), (0.5, 0.5), 0.0, matrix(15, 15, Corner::TopLeft));
    fading.edge_behavior = Some(EdgeBehavior::FadeToBlack { falloff: 1.0 });
    let fading = layout(vec![fading]);
    let surfaces = DeviceSurfaceMap::from_layout(&fading, single_device(device));
    assert_close(
        surfaces.canvas_position(device, 1.0, 0.5).expect("mapped"),
        NormalizedPosition::new(1.25, 0.5),
    );
}

#[test]
fn device_without_an_output_has_no_canvas_position() {
    let device = DeviceId::new();
    let elsewhere = DeviceId::new();
    let layout = layout(vec![output(
        "pad",
        "blocks:lightpad",
        matrix(15, 15, Corner::TopLeft),
    )]);
    let surfaces = DeviceSurfaceMap::from_layout(&layout, single_device(device));

    assert!(!surfaces.contains(elsewhere));
    assert_eq!(surfaces.canvas_position(elsewhere, 0.5, 0.5), None);
}

#[test]
fn unbound_layout_outputs_place_nothing() {
    let layout = layout(vec![output(
        "pad",
        "blocks:lightpad",
        matrix(15, 15, Corner::TopLeft),
    )]);
    let surfaces = DeviceSurfaceMap::from_layout(&layout, |_| None);

    assert!(surfaces.is_empty());
    assert_eq!(surfaces, DeviceSurfaceMap::default());
}

#[test]
fn device_with_several_outputs_is_ambiguous_and_unplaced() {
    let device = DeviceId::new();
    let other = DeviceId::new();
    let layout = layout(vec![
        output("left", "blocks:lightpad", matrix(15, 15, Corner::TopLeft)),
        output("other", "blocks:other", matrix(15, 15, Corner::TopLeft)),
        placed((0.2, 0.2), (0.1, 0.1), 0.0, matrix(15, 15, Corner::TopLeft)),
    ]);
    let surfaces =
        DeviceSurfaceMap::from_layout(&layout, |layout_device_id| match layout_device_id {
            "blocks:lightpad" => Some(device),
            "blocks:other" => Some(other),
            _ => None,
        });

    assert!(!surfaces.contains(device));
    assert_eq!(surfaces.canvas_position(device, 0.5, 0.5), None);
    assert!(surfaces.contains(other));
}

#[test]
fn display_viewports_are_not_touch_surfaces() {
    let device = DeviceId::new();
    let mut display = output("screen", "blocks:lightpad", LedTopology::Point);
    display.zone_name = Some(DISPLAY_ZONE_NAME.to_owned());
    let layout = layout(vec![
        display,
        placed((0.25, 0.25), (0.5, 0.5), 0.0, matrix(8, 8, Corner::TopLeft)),
    ]);
    let surfaces = DeviceSurfaceMap::from_layout(&layout, single_device(device));

    assert_close(
        surfaces.canvas_position(device, 0.0, 0.0).expect("mapped"),
        NormalizedPosition::new(0.0, 0.0),
    );
}

#[test]
fn non_finite_points_have_no_canvas_position_and_out_of_range_points_clamp() {
    let device = DeviceId::new();
    let layout = layout(vec![output(
        "pad",
        "blocks:lightpad",
        matrix(15, 15, Corner::TopLeft),
    )]);
    let surfaces = DeviceSurfaceMap::from_layout(&layout, single_device(device));

    assert_eq!(surfaces.canvas_position(device, f32::NAN, 0.5), None);
    assert_eq!(surfaces.canvas_position(device, 0.5, f32::INFINITY), None);
    assert_close(
        surfaces.canvas_position(device, 1.5, -0.5).expect("mapped"),
        NormalizedPosition::new(1.0, 0.0),
    );
}

#[test]
fn backend_manager_reports_layout_bindings_and_their_generation() {
    let mut manager = BackendManager::new();
    let device = DeviceId::new();
    let initial = manager.routing_mapping_generation();
    assert_eq!(manager.layout_device_bindings().count(), 0);

    manager.map_device("blocks:lightpad", "blocks", device);
    let mapped = manager.routing_mapping_generation();
    assert!(mapped > initial);
    assert_eq!(
        manager.layout_device_bindings().collect::<Vec<_>>(),
        vec![("blocks:lightpad", device)]
    );

    assert!(manager.unmap_device("blocks:lightpad"));
    assert!(manager.routing_mapping_generation() > mapped);
    assert_eq!(manager.layout_device_bindings().count(), 0);
}
