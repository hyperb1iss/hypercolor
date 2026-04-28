use hypercolor_driver_nanoleaf::{
    NanoleafPanelLayout, NanoleafShapeType, build_device_info, panel_ids_from_layout,
};
use hypercolor_types::device::{DeviceFamily, DeviceTopologyHint};

#[test]
fn shape_type_filters_non_lighting_components() {
    assert!(
        !NanoleafShapeType::ShapesController.has_leds(),
        "controller modules should not become addressable zones"
    );
    assert!(
        !NanoleafShapeType::PowerConnector.has_leds(),
        "power connectors should not become addressable zones"
    );
    assert!(
        NanoleafShapeType::HexagonShapes.has_leds(),
        "lighting panels should remain addressable"
    );
}

#[test]
fn shape_type_maps_strip_topologies() {
    assert_eq!(
        NanoleafShapeType::LightLines.to_topology_hint(),
        DeviceTopologyHint::Strip
    );
    assert_eq!(
        NanoleafShapeType::LightLinesSingleZone.to_topology_hint(),
        DeviceTopologyHint::Strip
    );
    assert_eq!(
        NanoleafShapeType::TriangleShapes.to_topology_hint(),
        DeviceTopologyHint::Point
    );
}

#[test]
fn build_device_info_filters_non_led_panels_and_preserves_order() {
    let panels = vec![
        NanoleafPanelLayout {
            panel_id: 10,
            x: 0,
            y: 0,
            o: 0,
            shape_type: u8::from(NanoleafShapeType::ShapesController),
        },
        NanoleafPanelLayout {
            panel_id: 11,
            x: 10,
            y: 10,
            o: 90,
            shape_type: u8::from(NanoleafShapeType::HexagonShapes),
        },
        NanoleafPanelLayout {
            panel_id: 12,
            x: 20,
            y: 20,
            o: 180,
            shape_type: u8::from(NanoleafShapeType::LightLines),
        },
    ];

    let info = build_device_info(
        "living-room",
        "Living Room Shapes",
        Some("Shapes"),
        Some("12.3.4"),
        panels.as_slice(),
    );

    assert_eq!(
        info.family,
        DeviceFamily::new_static("nanoleaf", "Nanoleaf")
    );
    assert_eq!(info.total_led_count(), 2);
    assert_eq!(info.zones.len(), 2);
    assert_eq!(info.zones[0].name, "Panel 11");
    assert_eq!(info.zones[0].topology, DeviceTopologyHint::Point);
    assert_eq!(info.zones[1].name, "Panel 12");
    assert_eq!(info.zones[1].topology, DeviceTopologyHint::Strip);
    assert_eq!(panel_ids_from_layout(panels.as_slice()), vec![11, 12]);
}
