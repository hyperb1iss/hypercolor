use hypercolor_types::spatial::{
    Corner, DeviceZone, EdgeBehavior, LedTopology, NormalizedPosition, NormalizedRect, Orientation,
    RingDef, RoomAdjacency, RoomDimensions, SamplingMode, SpaceDefinition, SpatialLayout,
    StripDirection, Wall, Winding, ZoneGroup, ZoneShape,
};

// ── NormalizedPosition ──────────────────────────────────────────────────────

#[test]
fn normalized_position_new() {
    let pos = NormalizedPosition::new(0.25, 0.75);
    assert!((pos.x - 0.25).abs() < f32::EPSILON);
    assert!((pos.y - 0.75).abs() < f32::EPSILON);
}

#[test]
fn normalized_position_default_is_origin() {
    let pos = NormalizedPosition::default();
    assert!((pos.x).abs() < f32::EPSILON);
    assert!((pos.y).abs() < f32::EPSILON);
}

#[test]
fn from_pixel_standard_canvas() {
    // Pixel 0 -> 0.0, pixel 319 -> 1.0 on a 320-wide canvas
    let origin = NormalizedPosition::from_pixel(0.0, 0.0, 320, 200);
    assert!((origin.x).abs() < f32::EPSILON);
    assert!((origin.y).abs() < f32::EPSILON);

    let corner = NormalizedPosition::from_pixel(319.0, 199.0, 320, 200);
    assert!((corner.x - 1.0).abs() < f32::EPSILON);
    assert!((corner.y - 1.0).abs() < f32::EPSILON);
}

#[test]
fn from_pixel_single_pixel_canvas() {
    // A 1x1 canvas should map to (0.5, 0.5)
    let pos = NormalizedPosition::from_pixel(0.0, 0.0, 1, 1);
    assert!((pos.x - 0.5).abs() < f32::EPSILON);
    assert!((pos.y - 0.5).abs() < f32::EPSILON);
}

#[test]
fn to_pixel_roundtrip() {
    let original = NormalizedPosition::new(0.5, 0.5);
    let (px, py) = original.to_pixel(320, 200);
    let recovered = NormalizedPosition::from_pixel(px, py, 320, 200);
    assert!((recovered.x - original.x).abs() < 1e-5);
    assert!((recovered.y - original.y).abs() < 1e-5);
}

#[test]
fn to_pixel_rounded_clamps() {
    let pos = NormalizedPosition::new(1.5, -0.5);
    let (px, py) = pos.to_pixel_rounded(320, 200);
    // Clamped to canvas bounds
    assert!(px <= 319);
    assert!(py == 0); // negative clamped to 0
}

#[test]
fn to_pixel_zero_canvas() {
    // Degenerate: 0-size canvas should not panic (saturating_sub handles it)
    let pos = NormalizedPosition::new(0.5, 0.5);
    let (px, py) = pos.to_pixel(0, 0);
    assert!((px).abs() < f32::EPSILON);
    assert!((py).abs() < f32::EPSILON);
}

#[test]
fn lerp_midpoint() {
    let a = NormalizedPosition::new(0.0, 0.0);
    let b = NormalizedPosition::new(1.0, 1.0);
    let mid = NormalizedPosition::lerp(a, b, 0.5);
    assert!((mid.x - 0.5).abs() < f32::EPSILON);
    assert!((mid.y - 0.5).abs() < f32::EPSILON);
}

#[test]
fn lerp_endpoints() {
    let a = NormalizedPosition::new(0.2, 0.3);
    let b = NormalizedPosition::new(0.8, 0.9);

    let at_zero = NormalizedPosition::lerp(a, b, 0.0);
    assert!((at_zero.x - a.x).abs() < f32::EPSILON);
    assert!((at_zero.y - a.y).abs() < f32::EPSILON);

    let at_one = NormalizedPosition::lerp(a, b, 1.0);
    assert!((at_one.x - b.x).abs() < f32::EPSILON);
    assert!((at_one.y - b.y).abs() < f32::EPSILON);
}

#[test]
fn distance_same_point_is_zero() {
    let p = NormalizedPosition::new(0.5, 0.5);
    assert!(NormalizedPosition::distance(p, p).abs() < f32::EPSILON);
}

#[test]
fn distance_unit_diagonal() {
    let a = NormalizedPosition::new(0.0, 0.0);
    let b = NormalizedPosition::new(1.0, 1.0);
    let d = NormalizedPosition::distance(a, b);
    assert!((d - std::f32::consts::SQRT_2).abs() < 1e-6);
}

#[test]
fn distance_horizontal() {
    let a = NormalizedPosition::new(0.0, 0.5);
    let b = NormalizedPosition::new(1.0, 0.5);
    let d = NormalizedPosition::distance(a, b);
    assert!((d - 1.0).abs() < f32::EPSILON);
}

#[test]
fn clamp_to_canvas_clamps_oob() {
    let pos = NormalizedPosition::new(-0.5, 1.5);
    let clamped = pos.clamp_to_canvas();
    assert!((clamped.x).abs() < f32::EPSILON);
    assert!((clamped.y - 1.0).abs() < f32::EPSILON);
}

#[test]
fn clamp_to_canvas_noop_for_valid() {
    let pos = NormalizedPosition::new(0.3, 0.7);
    let clamped = pos.clamp_to_canvas();
    assert!((clamped.x - pos.x).abs() < f32::EPSILON);
    assert!((clamped.y - pos.y).abs() < f32::EPSILON);
}

#[test]
fn is_on_canvas_boundaries() {
    assert!(NormalizedPosition::new(0.0, 0.0).is_on_canvas());
    assert!(NormalizedPosition::new(1.0, 1.0).is_on_canvas());
    assert!(NormalizedPosition::new(0.5, 0.5).is_on_canvas());
    assert!(!NormalizedPosition::new(-0.01, 0.5).is_on_canvas());
    assert!(!NormalizedPosition::new(0.5, 1.01).is_on_canvas());
}

// ── LedTopology ─────────────────────────────────────────────────────────────

#[test]
fn led_count_strip() {
    let topo = LedTopology::Strip {
        count: 60,
        direction: StripDirection::LeftToRight,
    };
    assert_eq!(topo.led_count(), 60);
}

#[test]
fn led_count_matrix() {
    let topo = LedTopology::Matrix {
        width: 8,
        height: 4,
        serpentine: true,
        start_corner: Corner::TopLeft,
    };
    assert_eq!(topo.led_count(), 32);
}

#[test]
fn led_count_ring() {
    let topo = LedTopology::Ring {
        count: 16,
        start_angle: 0.0,
        direction: Winding::Clockwise,
    };
    assert_eq!(topo.led_count(), 16);
}

#[test]
fn led_count_concentric_rings() {
    let topo = LedTopology::ConcentricRings {
        rings: vec![
            RingDef {
                count: 16,
                radius: 1.0,
                start_angle: 0.0,
                direction: Winding::Clockwise,
            },
            RingDef {
                count: 8,
                radius: 0.5,
                start_angle: 0.0,
                direction: Winding::CounterClockwise,
            },
        ],
    };
    assert_eq!(topo.led_count(), 24);
}

#[test]
fn led_count_perimeter_loop() {
    let topo = LedTopology::PerimeterLoop {
        top: 10,
        right: 6,
        bottom: 10,
        left: 6,
        start_corner: Corner::TopLeft,
        direction: Winding::Clockwise,
    };
    assert_eq!(topo.led_count(), 32);
}

#[test]
fn led_count_point() {
    assert_eq!(LedTopology::Point.led_count(), 1);
}

#[test]
fn led_count_custom() {
    let topo = LedTopology::Custom {
        positions: vec![
            NormalizedPosition::new(0.0, 0.0),
            NormalizedPosition::new(0.5, 0.5),
            NormalizedPosition::new(1.0, 1.0),
        ],
    };
    assert_eq!(topo.led_count(), 3);
}

// ── StripDirection ──────────────────────────────────────────────────────────

#[test]
fn strip_direction_all_variants() {
    // Ensure all variants are constructible (compile-time coverage).
    let dirs = [
        StripDirection::LeftToRight,
        StripDirection::RightToLeft,
        StripDirection::TopToBottom,
        StripDirection::BottomToTop,
    ];
    assert_eq!(dirs.len(), 4);
}

// ── DeviceZone ──────────────────────────────────────────────────────────────

#[test]
fn device_zone_construction() {
    let expected_mapping: Vec<u32> = (0..24).rev().collect();
    let zone = DeviceZone {
        id: "zone-1".into(),
        name: "ATX Strimer".into(),
        device_id: "hid:prism-s-1".into(),
        zone_name: Some("atx".into()),
        group_id: Some("pc-case".into()),
        position: NormalizedPosition::new(0.5, 0.5),
        size: NormalizedPosition::new(0.3, 0.1),
        rotation: 0.0,
        scale: 1.0,
        orientation: Some(Orientation::Horizontal),
        topology: LedTopology::Strip {
            count: 24,
            direction: StripDirection::LeftToRight,
        },
        led_positions: Vec::new(),
        led_mapping: Some(expected_mapping.clone()),
        sampling_mode: None,
        edge_behavior: None,
        shape: Some(ZoneShape::Rectangle),
        shape_preset: Some("strimer-atx-24pin".into()),
        attachment: None,
    };

    assert_eq!(zone.id, "zone-1");
    assert_eq!(zone.device_id, "hid:prism-s-1");
    assert_eq!(zone.zone_name.as_deref(), Some("atx"));
    assert_eq!(zone.group_id.as_deref(), Some("pc-case"));
    assert_eq!(zone.topology.led_count(), 24);
    assert_eq!(
        zone.led_mapping.as_deref(),
        Some(expected_mapping.as_slice())
    );
    assert!((zone.scale - 1.0).abs() < f32::EPSILON);
}

#[test]
fn device_zone_optional_fields() {
    let zone = DeviceZone {
        id: "z".into(),
        name: "Bulb".into(),
        device_id: "hue:1".into(),
        zone_name: None,
        group_id: None,
        position: NormalizedPosition::new(0.5, 0.5),
        size: NormalizedPosition::new(0.1, 0.1),
        rotation: 0.0,
        scale: 1.0,
        orientation: None,
        topology: LedTopology::Point,
        led_positions: Vec::new(),
        led_mapping: None,
        sampling_mode: None,
        edge_behavior: None,
        shape: None,
        shape_preset: None,
        attachment: None,
    };

    assert!(zone.zone_name.is_none());
    assert!(zone.group_id.is_none());
    assert!(zone.orientation.is_none());
    assert!(zone.sampling_mode.is_none());
    assert!(zone.edge_behavior.is_none());
    assert!(zone.shape.is_none());
    assert!(zone.shape_preset.is_none());
    assert!(zone.led_mapping.is_none());
}

// ── SpatialLayout ───────────────────────────────────────────────────────────

#[test]
fn spatial_layout_empty_zones() {
    let layout = SpatialLayout {
        id: "layout-1".into(),
        name: "Test Layout".into(),
        description: None,
        canvas_width: 320,
        canvas_height: 200,
        zones: vec![],
        groups: vec![],
        default_sampling_mode: SamplingMode::Bilinear,
        default_edge_behavior: EdgeBehavior::Clamp,
        spaces: None,
        version: 1,
    };

    assert_eq!(layout.canvas_width, 320);
    assert_eq!(layout.canvas_height, 200);
    assert!(layout.zones.is_empty());
    assert!(layout.groups.is_empty());
    assert!(layout.spaces.is_none());
}

#[test]
fn spatial_layout_with_zones() {
    let zone = DeviceZone {
        id: "fan-1".into(),
        name: "Front Fan".into(),
        device_id: "hid:prism-s-1".into(),
        zone_name: Some("ch1".into()),
        group_id: Some("pc-case".into()),
        position: NormalizedPosition::new(0.2, 0.3),
        size: NormalizedPosition::new(0.15, 0.15),
        rotation: std::f32::consts::FRAC_PI_4,
        scale: 1.0,
        orientation: Some(Orientation::Radial),
        topology: LedTopology::Ring {
            count: 16,
            start_angle: 0.0,
            direction: Winding::Clockwise,
        },
        led_positions: Vec::new(),
        led_mapping: None,
        sampling_mode: Some(SamplingMode::Bilinear),
        edge_behavior: Some(EdgeBehavior::Clamp),
        shape: Some(ZoneShape::Ring),
        shape_preset: None,
        attachment: None,
    };

    let layout = SpatialLayout {
        id: "layout-2".into(),
        name: "PC Case".into(),
        description: Some("Full case layout".into()),
        canvas_width: 320,
        canvas_height: 200,
        zones: vec![zone],
        groups: vec![ZoneGroup {
            id: "pc-case".into(),
            name: "PC Case".into(),
            color: Some("#e135ff".into()),
        }],
        default_sampling_mode: SamplingMode::Bilinear,
        default_edge_behavior: EdgeBehavior::Clamp,
        spaces: None,
        version: 1,
    };

    assert_eq!(layout.zones.len(), 1);
    assert_eq!(layout.groups.len(), 1);
    assert_eq!(layout.zones[0].topology.led_count(), 16);
}

// ── SamplingMode ────────────────────────────────────────────────────────────

#[test]
fn sampling_mode_variants() {
    let nearest = SamplingMode::Nearest;
    let bilinear = SamplingMode::Bilinear;
    let area = SamplingMode::AreaAverage {
        radius_x: 4.0,
        radius_y: 4.0,
    };
    let gauss = SamplingMode::GaussianArea {
        sigma: 2.0,
        radius: 8,
    };
    assert_ne!(format!("{nearest:?}"), format!("{bilinear:?}"));
    assert_ne!(format!("{area:?}"), format!("{gauss:?}"));
}

// ── EdgeBehavior ────────────────────────────────────────────────────────────

#[test]
fn edge_behavior_variants() {
    let clamp = EdgeBehavior::Clamp;
    let wrap = EdgeBehavior::Wrap;
    let fade = EdgeBehavior::FadeToBlack { falloff: 2.0 };
    let mirror = EdgeBehavior::Mirror;
    assert_ne!(format!("{clamp:?}"), format!("{wrap:?}"));
    assert_ne!(format!("{fade:?}"), format!("{mirror:?}"));
}

// ── ZoneShape ───────────────────────────────────────────────────────────────

#[test]
fn zone_shape_variants() {
    let rect = ZoneShape::Rectangle;
    let arc = ZoneShape::Arc {
        start_angle: 0.0,
        sweep_angle: std::f32::consts::PI,
    };
    let ring = ZoneShape::Ring;
    let custom = ZoneShape::Custom {
        vertices: vec![
            NormalizedPosition::new(0.0, 0.0),
            NormalizedPosition::new(1.0, 0.0),
            NormalizedPosition::new(0.5, 1.0),
        ],
    };
    assert_ne!(format!("{rect:?}"), format!("{arc:?}"));
    assert_ne!(format!("{ring:?}"), format!("{custom:?}"));
}

// ── Serde Round-Trip ────────────────────────────────────────────────────────

#[test]
fn normalized_position_json_roundtrip() {
    let pos = NormalizedPosition::new(0.42, 0.87);
    let json = serde_json::to_string(&pos).expect("serialize NormalizedPosition");
    let recovered: NormalizedPosition =
        serde_json::from_str(&json).expect("deserialize NormalizedPosition");
    assert_eq!(pos, recovered);
}

#[test]
fn spatial_layout_deserializes_missing_groups_and_group_ids() {
    let raw = r#"{
        "id": "legacy-layout",
        "name": "Legacy Layout",
        "description": null,
        "canvas_width": 320,
        "canvas_height": 200,
        "zones": [
            {
                "id": "zone-1",
                "name": "Legacy Zone",
                "device_id": "mock:legacy",
                "zone_name": null,
                "position": {"x": 0.5, "y": 0.5},
                "size": {"x": 0.2, "y": 0.2},
                "rotation": 0.0,
                "scale": 1.0,
                "orientation": null,
                "topology": {"type": "point"},
                "sampling_mode": null,
                "edge_behavior": null,
                "shape": null,
                "shape_preset": null
            }
        ],
        "default_sampling_mode": {"type": "bilinear"},
        "default_edge_behavior": "clamp",
        "spaces": null,
        "version": 1
    }"#;

    let layout: SpatialLayout =
        serde_json::from_str(raw).expect("legacy layouts should still deserialize");

    assert!(layout.groups.is_empty());
    assert_eq!(layout.zones.len(), 1);
    assert!(layout.zones[0].group_id.is_none());
}

#[test]
fn led_topology_strip_json_roundtrip() {
    let topo = LedTopology::Strip {
        count: 30,
        direction: StripDirection::RightToLeft,
    };
    let json = serde_json::to_string(&topo).expect("serialize Strip");
    let recovered: LedTopology = serde_json::from_str(&json).expect("deserialize Strip");
    assert_eq!(recovered.led_count(), 30);
}

#[test]
fn led_topology_matrix_json_roundtrip() {
    let topo = LedTopology::Matrix {
        width: 16,
        height: 9,
        serpentine: true,
        start_corner: Corner::BottomRight,
    };
    let json = serde_json::to_string(&topo).expect("serialize Matrix");
    let recovered: LedTopology = serde_json::from_str(&json).expect("deserialize Matrix");
    assert_eq!(recovered.led_count(), 144);
}

#[test]
fn led_topology_point_json_roundtrip() {
    let topo = LedTopology::Point;
    let json = serde_json::to_string(&topo).expect("serialize Point");
    let recovered: LedTopology = serde_json::from_str(&json).expect("deserialize Point");
    assert_eq!(recovered.led_count(), 1);
}

#[test]
fn sampling_mode_json_roundtrip() {
    let mode = SamplingMode::GaussianArea {
        sigma: 1.5,
        radius: 6,
    };
    let json = serde_json::to_string(&mode).expect("serialize SamplingMode");
    let recovered: SamplingMode = serde_json::from_str(&json).expect("deserialize SamplingMode");
    // Verify it deserialized to the right variant
    assert!(matches!(recovered, SamplingMode::GaussianArea { .. }));
}

#[test]
fn edge_behavior_json_roundtrip() {
    let eb = EdgeBehavior::FadeToBlack { falloff: 3.0 };
    let json = serde_json::to_string(&eb).expect("serialize EdgeBehavior");
    let recovered: EdgeBehavior = serde_json::from_str(&json).expect("deserialize EdgeBehavior");
    assert!(matches!(recovered, EdgeBehavior::FadeToBlack { .. }));
}

#[test]
fn zone_shape_arc_json_roundtrip() {
    let shape = ZoneShape::Arc {
        start_angle: 0.0,
        sweep_angle: std::f32::consts::TAU,
    };
    let json = serde_json::to_string(&shape).expect("serialize ZoneShape::Arc");
    let recovered: ZoneShape = serde_json::from_str(&json).expect("deserialize ZoneShape::Arc");
    assert!(matches!(recovered, ZoneShape::Arc { .. }));
}

#[test]
fn zone_group_construction() {
    let group = ZoneGroup {
        id: "desk".into(),
        name: "Desk".into(),
        color: Some("#80ffea".into()),
    };

    assert_eq!(group.id, "desk");
    assert_eq!(group.name, "Desk");
    assert_eq!(group.color.as_deref(), Some("#80ffea"));
}

#[test]
fn spatial_layout_deserializes_without_groups_fields() {
    let raw = serde_json::json!({
        "id": "legacy-layout",
        "name": "Legacy Layout",
        "description": null,
        "canvas_width": 320,
        "canvas_height": 200,
        "zones": [{
            "id": "zone-1",
            "name": "Legacy Zone",
            "device_id": "wled:desk",
            "zone_name": null,
            "position": { "x": 0.5, "y": 0.5 },
            "size": { "x": 0.3, "y": 0.2 },
            "rotation": 0.0,
            "scale": 1.0,
            "orientation": null,
            "topology": { "type": "point" },
            "sampling_mode": null,
            "edge_behavior": null,
            "shape": null,
            "shape_preset": null
        }],
        "default_sampling_mode": { "type": "bilinear" },
        "default_edge_behavior": "clamp",
        "spaces": null,
        "version": 1
    });

    let layout: SpatialLayout =
        serde_json::from_value(raw).expect("legacy layout JSON should deserialize");

    assert!(layout.groups.is_empty());
    assert_eq!(layout.zones.len(), 1);
    assert!(layout.zones[0].group_id.is_none());
}

// ── Multi-Room Types ────────────────────────────────────────────────────────

#[test]
fn space_definition_construction() {
    let space = SpaceDefinition {
        id: "office".into(),
        name: "Office".into(),
        dimensions: Some(RoomDimensions {
            width: 400.0,
            height: 280.0,
            depth: 350.0,
        }),
        canvas_region: Some(NormalizedRect {
            x: 0.0,
            y: 0.0,
            width: 0.5,
            height: 1.0,
        }),
        zone_ids: vec!["zone-1".into(), "zone-2".into()],
        adjacency: vec![RoomAdjacency {
            neighbor_id: "living-room".into(),
            shared_wall: Wall::East,
            blend_width: 16,
        }],
    };

    assert_eq!(space.id, "office");
    assert_eq!(space.zone_ids.len(), 2);
    assert_eq!(space.adjacency.len(), 1);
    assert_eq!(space.adjacency[0].shared_wall, Wall::East);
}

#[test]
fn layout_with_spaces_json_roundtrip() {
    let layout = SpatialLayout {
        id: "multi-room".into(),
        name: "Full House".into(),
        description: Some("Multi-room layout".into()),
        canvas_width: 640,
        canvas_height: 400,
        zones: vec![],
        groups: vec![ZoneGroup {
            id: "living-room".into(),
            name: "Living Room".into(),
            color: None,
        }],
        default_sampling_mode: SamplingMode::Bilinear,
        default_edge_behavior: EdgeBehavior::Clamp,
        spaces: Some(vec![SpaceDefinition {
            id: "room-1".into(),
            name: "Room 1".into(),
            dimensions: None,
            canvas_region: None,
            zone_ids: vec![],
            adjacency: vec![],
        }]),
        version: 1,
    };

    let json = serde_json::to_string_pretty(&layout).expect("serialize SpatialLayout");
    let recovered: SpatialLayout = serde_json::from_str(&json).expect("deserialize SpatialLayout");
    assert_eq!(recovered.id, "multi-room");
    assert_eq!(recovered.canvas_width, 640);
    assert_eq!(recovered.groups.len(), 1);
    assert!(recovered.spaces.is_some());
}

// ── Wall Enum ───────────────────────────────────────────────────────────────

#[test]
fn wall_all_variants() {
    let walls = [Wall::North, Wall::South, Wall::East, Wall::West];
    assert_eq!(walls.len(), 4);
    assert_ne!(Wall::North, Wall::South);
}

// ── Winding Enum ────────────────────────────────────────────────────────────

#[test]
fn winding_variants() {
    assert_ne!(Winding::Clockwise, Winding::CounterClockwise);
}

// ── Corner Enum ─────────────────────────────────────────────────────────────

#[test]
fn corner_all_variants() {
    let corners = [
        Corner::TopLeft,
        Corner::TopRight,
        Corner::BottomLeft,
        Corner::BottomRight,
    ];
    assert_eq!(corners.len(), 4);
}

// ── Orientation Enum ────────────────────────────────────────────────────────

#[test]
fn orientation_all_variants() {
    let orientations = [
        Orientation::Horizontal,
        Orientation::Vertical,
        Orientation::Diagonal,
        Orientation::Radial,
    ];
    assert_eq!(orientations.len(), 4);
    assert_ne!(Orientation::Horizontal, Orientation::Vertical);
}
