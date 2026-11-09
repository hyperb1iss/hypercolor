use hypercolor_driver_nanoleaf::{
    NanoleafPanelLayout, NanoleafPanelLayoutDocument, build_device_info, panel_ids_from_layout,
};
use hypercolor_types::device::{DeviceInfo, SegmentInfo};
use hypercolor_types::spatial::{LedTopology, NormalizedPosition, ZoneShape};

fn panel(id: u16, x: i32, y: i32, o: i16, shape_type: u8) -> NanoleafPanelLayout {
    NanoleafPanelLayout {
        panel_id: id,
        x,
        y,
        o,
        shape_type,
    }
}

fn device(panels: &[NanoleafPanelLayout], rotation: f32) -> DeviceInfo {
    build_device_info("fixture", "Fixture", Some("NL42"), None, panels, rotation)
}

fn project(segment: &SegmentInfo, point: NormalizedPosition) -> NormalizedPosition {
    let placement = segment
        .layout_hint
        .as_ref()
        .expect("polygon hint")
        .placement
        .as_ref()
        .expect("placement");
    NormalizedPosition::new(
        placement.position.x + (point.x - 0.5) * placement.size.x,
        placement.position.y + (point.y - 0.5) * placement.size.y,
    )
}

fn vertices(segment: &SegmentInfo) -> Vec<NormalizedPosition> {
    let Some(ZoneShape::Custom { vertices }) = &segment.layout_hint.as_ref().expect("hint").shape
    else {
        panic!("expected a real panel polygon");
    };
    vertices
        .iter()
        .map(|point| project(segment, *point))
        .collect()
}

fn sample(segment: &SegmentInfo) -> NormalizedPosition {
    let Some(LedTopology::Custom { positions }) =
        &segment.layout_hint.as_ref().expect("hint").topology
    else {
        panic!("expected centroid sample");
    };
    assert_eq!(positions.len(), 1);
    project(segment, positions[0])
}

fn close(actual: f32, expected: f32) {
    assert!((actual - expected).abs() < 0.001, "{actual} != {expected}");
}

fn shared_vertices(left: &SegmentInfo, right: &SegmentInfo, tolerance: f32) -> usize {
    let right = vertices(right);
    vertices(left)
        .iter()
        .filter(|a| {
            right
                .iter()
                .any(|b| (a.x - b.x).hypot(a.y - b.y) < tolerance)
        })
        .count()
}

#[test]
fn triangles_preserve_api_centroids_instead_of_using_box_centers() {
    let info = device(&[panel(1, -20, 50, 0, 8)], 0.0);
    let segment = &info.segments[0];
    let center = sample(segment);
    close(center.x, -20.0);
    close(center.y, -50.0);
    let placement = segment
        .layout_hint
        .as_ref()
        .expect("hint")
        .placement
        .as_ref()
        .expect("placement");
    assert!((placement.position.y - center.y).abs() > 10.0);
    close(placement.size.x, 134.0);
    close(placement.size.y, 134.0 * 3.0_f32.sqrt() / 2.0);
}

#[test]
fn documented_light_panels_share_an_edge_after_orientation_and_y_flip() {
    // Panels 1 and 4 from Nanoleaf's six-panel reference layout. Coordinates
    // are rounded to whole API units, so the shared edge differs by 1 unit.
    let info = device(&[panel(1, 100, 100, 0, 0), panel(4, 99, 13, 180, 0)], 0.0);
    assert_eq!(
        shared_vertices(&info.segments[0], &info.segments[1], 1.1),
        2
    );
    close(sample(&info.segments[0]).y, -100.0);
    close(sample(&info.segments[1]).y, -13.0);
}

#[test]
fn mixed_hexagon_and_mini_triangle_keep_matching_edges_and_relative_scale() {
    // A 67-unit triangle sits on the 67-unit top edge of a Shapes hexagon.
    // Its center is 67*sqrt(3)/2 + 67*sqrt(3)/6 = 77.365 units up.
    let info = device(
        &[
            panel(1, 0, 0, 0, 7),
            panel(2, 0, 77, 0, 9),
            panel(3, 250, 0, 0, 8),
        ],
        0.0,
    );
    assert_eq!(vertices(&info.segments[0]).len(), 6);
    assert_eq!(vertices(&info.segments[1]).len(), 3);
    assert_eq!(
        shared_vertices(&info.segments[0], &info.segments[1], 0.4),
        2
    );
    let mini = info.segments[1]
        .layout_hint
        .as_ref()
        .expect("mini")
        .placement
        .as_ref()
        .expect("placement");
    let triangle = info.segments[2]
        .layout_hint
        .as_ref()
        .expect("triangle")
        .placement
        .as_ref()
        .expect("placement");
    close(triangle.size.x, mini.size.x * 2.0);
    close(triangle.size.y, mini.size.y * 2.0);
}

#[test]
fn global_orientation_rotates_polygons_and_samples_together() {
    let panels = [panel(1, 30, -40, 60, 8), panel(2, 140, 100, 0, 7)];
    let original = device(&panels, 0.0);
    let rotated = device(&panels, 90.0);
    for (before, after) in original.segments.iter().zip(&rotated.segments) {
        for (a, b) in vertices(before).iter().zip(vertices(after)) {
            close(b.x, a.y);
            close(b.y, -a.x);
        }
        let a = sample(before);
        let b = sample(after);
        close(b.x, a.y);
        close(b.y, -a.x);
    }
}

#[test]
fn canvas_controller_squares_are_polygons_and_keep_their_shared_edge() {
    let info = device(&[panel(1, 0, 0, 0, 3), panel(2, 100, 0, 90, 2)], 0.0);
    assert_eq!(info.total_led_count(), 2);
    assert_eq!(vertices(&info.segments[0]).len(), 4);
    assert_eq!(
        shared_vertices(&info.segments[0], &info.segments[1], 0.001),
        2
    );
}

#[test]
fn unknown_shapes_remain_addressable_without_invented_polygons() {
    let panels = [panel(1, 0, 0, 0, 12), panel(2, 10, 10, 0, 250)];
    let info = device(&panels, 0.0);
    assert_eq!(panel_ids_from_layout(&panels), vec![2]);
    assert_eq!(info.total_led_count(), 1);
    assert_eq!(info.segments[0].name, "Panel 2");
    assert!(info.segments[0].layout_hint.is_none());
    assert_eq!(info.capabilities.max_fps, 10);
}

#[test]
fn panel_layout_decodes_complete_layout_and_global_orientation() {
    let layout: NanoleafPanelLayoutDocument = serde_json::from_value(serde_json::json!({
            "globalOrientation": { "value": 120, "min": 0, "max": 360 },
            "layout": { "numPanels": 2, "sideLength": 0, "positionData": [
                { "panelId": 1, "x": -30, "y": 40, "o": 60, "shapeType": 7 },
                { "panelId": 2, "x": 30, "y": 80, "o": 180, "shapeType": 9 }
            ] }
    }))
    .expect("Nanoleaf panel-layout fixture");
    close(layout.global_orientation.value, 120.0);
    assert_eq!(layout.layout.position_data.len(), 2);
    assert_eq!(layout.layout.position_data[0].x, -30);
}
