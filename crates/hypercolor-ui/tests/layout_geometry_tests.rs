use hypercolor_types::attachment::{
    ComponentCanvasSize, ComponentCategory, ComponentSuggestedZone,
};
use hypercolor_types::spatial::{
    EdgeBehavior, LedTopology, NormalizedPosition, Output, SamplingMode, SpatialLayout,
};
use hypercolor_types::spatial::{StripDirection, ZoneShape};
use hypercolor_ui::api::{SegmentSummary, SegmentTopologySummary};
use hypercolor_ui::layout_geometry::{self, ResizeHandle, SizeAxis};

fn zone_summary(
    name: &str,
    led_count: u32,
    topology_hint: SegmentTopologySummary,
) -> SegmentSummary {
    SegmentSummary {
        id: format!("zone-{name}"),
        name: name.to_owned(),
        led_count,
        topology: "custom".to_owned(),
        topology_hint: Some(topology_hint),
    }
}

fn rendered_aspect(size: NormalizedPosition, canvas_width: u32, canvas_height: u32) -> f32 {
    #[allow(clippy::cast_precision_loss)]
    let canvas_width = canvas_width as f32;
    #[allow(clippy::cast_precision_loss)]
    let canvas_height = canvas_height as f32;
    let canvas_aspect = canvas_width / canvas_height;
    (size.x / size.y) * canvas_aspect
}

#[test]
fn default_layout_preserves_aspect_above_legacy_u16_dimensions() {
    let zone = zone_summary(
        "Display",
        0,
        SegmentTopologySummary::Display {
            width: 480,
            height: 480,
            circular: false,
        },
    );
    let defaults = layout_geometry::default_zone_visuals(
        "Large Canvas Display",
        Some(&zone),
        0,
        131_072,
        65_536,
    );

    assert!((rendered_aspect(defaults.size, 131_072, 65_536) - 1.0).abs() < 0.01);
}

fn push2_zone_summaries() -> Vec<SegmentSummary> {
    vec![
        zone_summary(
            "Pads",
            64,
            SegmentTopologySummary::Matrix { rows: 8, cols: 8 },
        ),
        zone_summary("Buttons Above", 8, SegmentTopologySummary::Strip),
        zone_summary("Buttons Below", 8, SegmentTopologySummary::Strip),
        zone_summary("Scene Launch", 8, SegmentTopologySummary::Strip),
        zone_summary("Transport", 4, SegmentTopologySummary::Custom),
        zone_summary("White Buttons", 37, SegmentTopologySummary::Custom),
        zone_summary("Touch Strip", 31, SegmentTopologySummary::Strip),
        zone_summary(
            "Display",
            0,
            SegmentTopologySummary::Display {
                width: 960,
                height: 160,
                circular: false,
            },
        ),
    ]
}

fn suggested_attachment(
    slot_id: &str,
    name: &str,
    instance: u32,
    led_start: u32,
    category: ComponentCategory,
    topology: LedTopology,
) -> ComponentSuggestedZone {
    ComponentSuggestedZone {
        slot_id: slot_id.to_owned(),
        template_id: format!("{slot_id}-{instance}"),
        template_name: name.to_owned(),
        name: name.to_owned(),
        instance,
        led_start,
        led_count: topology.led_count(),
        category,
        default_size: ComponentCanvasSize {
            width: 0.24,
            height: 0.24,
        },
        topology,
        led_mapping: None,
    }
}

#[test]
fn basilisk_v3_uses_signal_sparse_layout_instead_of_flat_matrix() {
    let zone = zone_summary(
        "Main",
        11,
        SegmentTopologySummary::Matrix { rows: 1, cols: 11 },
    );

    let defaults =
        layout_geometry::default_zone_visuals("Razer Basilisk V3", Some(&zone), 11, 320, 200);

    match defaults.topology {
        LedTopology::Custom { positions } => assert_eq!(positions.len(), 11),
        other => panic!("expected sparse custom topology, got {other:?}"),
    }

    let aspect = rendered_aspect(defaults.size, 320, 200);
    assert!((aspect - (7.0 / 8.0)).abs() < 0.05);
    assert!(defaults.size.y > defaults.size.x);
}

#[test]
fn square_lcd_defaults_preserve_square_rendered_aspect_on_default_canvas() {
    let zone = zone_summary(
        "Display",
        0,
        SegmentTopologySummary::Display {
            width: 480,
            height: 480,
            circular: true,
        },
    );

    let defaults =
        layout_geometry::default_zone_visuals("Corsair iCUE LINK LCD", Some(&zone), 0, 320, 200);

    match defaults.topology {
        LedTopology::Matrix { width, height, .. } => {
            assert_eq!((width, height), (480, 480));
        }
        other => panic!("expected matrix display topology, got {other:?}"),
    }

    assert_eq!(defaults.shape_preset.as_deref(), Some("lcd-display"));
    assert!((defaults.size.x - 0.15).abs() < 0.001);
    assert!((defaults.size.y - 0.24).abs() < 0.001);
    assert!((rendered_aspect(defaults.size, 320, 200) - 1.0).abs() < 0.01);
}

#[test]
fn seeded_push2_layout_creates_device_footprint() {
    let seeded = layout_geometry::seeded_device_layout(
        "usb:2982:1967:001-12",
        "Ableton Push 2",
        &push2_zone_summaries(),
        320,
        200,
        12,
    )
    .expect("push2 should produce a seeded layout");

    assert_eq!(seeded.zones.len(), 8);

    let pads = seeded
        .zones
        .iter()
        .find(|zone| zone.zone_name.as_deref() == Some("Pads"))
        .expect("pads zone should be seeded");
    assert_eq!(
        pads.topology,
        LedTopology::Matrix {
            width: 8,
            height: 8,
            serpentine: false,
            start_corner: hypercolor_types::spatial::Corner::BottomLeft,
        }
    );

    let white_buttons = seeded
        .zones
        .iter()
        .find(|zone| zone.zone_name.as_deref() == Some("White Buttons"))
        .expect("white buttons should be seeded");
    match &white_buttons.topology {
        LedTopology::Custom { positions } => {
            assert_eq!(positions.len(), 37);
            assert!(positions.iter().any(|pos| pos.x < 0.1));
            assert!(positions.iter().any(|pos| pos.x > 0.9));

            let repeat = positions[33];
            let accent = positions[34];
            let scale = positions[35];
            let layout = positions[3];
            let note = positions[11];
            let session = positions[12];
            let octave_down = positions[15];
            let octave_up = positions[16];
            let page_left = positions[19];
            let page_right = positions[20];
            let select = positions[9];
            let shift = positions[10];

            assert!(repeat.y < scale.y && scale.y < note.y);
            assert!(accent.y < layout.y && layout.y < session.y);
            assert!(repeat.x < accent.x);
            assert!(scale.x < layout.x);
            assert!(note.x < session.x);
            assert!((select.y - shift.y).abs() < 0.05);
            assert!(shift.x < select.x);
            assert!(octave_up.y < page_left.y);
            assert!(octave_down.y > page_left.y);
            assert!(page_left.x < octave_up.x && octave_up.x < page_right.x);
        }
        other => panic!("expected custom white-button topology, got {other:?}"),
    }

    let scene_launch = seeded
        .zones
        .iter()
        .find(|zone| zone.zone_name.as_deref() == Some("Scene Launch"))
        .expect("scene launch should be seeded");
    assert_eq!(
        scene_launch.topology,
        LedTopology::Strip {
            count: 8,
            direction: StripDirection::TopToBottom,
        }
    );

    let touch_strip = seeded
        .zones
        .iter()
        .find(|zone| zone.zone_name.as_deref() == Some("Touch Strip"))
        .expect("touch strip should be seeded");
    assert_eq!(
        touch_strip.topology,
        LedTopology::Strip {
            count: 31,
            direction: StripDirection::BottomToTop,
        }
    );
    assert!(touch_strip.position.x < pads.position.x);
    assert!(scene_launch.position.x > pads.position.x);

    let display = seeded
        .zones
        .iter()
        .find(|zone| zone.zone_name.as_deref() == Some("Display"))
        .expect("display zone should be seeded");
    assert!(display.position.y < pads.position.y);
    assert!(display.size.x > pads.size.x);
}

#[test]
fn set_zone_rotation_updates_single_zone_without_moving_it() {
    let mut layout = SpatialLayout {
        id: "default".to_owned(),
        name: "Default".to_owned(),
        description: None,
        canvas_width: 320,
        canvas_height: 200,
        zones: vec![Output {
            id: "zone-a".to_owned(),
            name: "A".to_owned(),
            device_id: "usb:a".to_owned(),
            zone_name: Some("A".to_owned()),
            position: NormalizedPosition::new(0.4, 0.6),
            size: NormalizedPosition::new(0.14, 0.1),
            rotation: 0.0,
            scale: 1.0,
            orientation: None,
            topology: LedTopology::Strip {
                count: 8,
                direction: StripDirection::LeftToRight,
            },
            led_positions: Vec::new(),
            led_mapping: None,
            sampling_mode: None,
            edge_behavior: None,
            shape: Some(ZoneShape::Rectangle),
            shape_preset: None,
            display_order: 0,
            attachment: None,
            brightness: None,
        }],
        default_sampling_mode: SamplingMode::Bilinear,
        default_edge_behavior: EdgeBehavior::Clamp,
        version: 1,
    };

    let rotation = 180.0_f32.to_radians();
    assert!(layout_geometry::set_zone_rotation(
        &mut layout,
        "zone-a",
        rotation,
    ));

    assert!((layout.zones[0].position.x - 0.4).abs() < 0.001);
    assert!((layout.zones[0].position.y - 0.6).abs() < 0.001);
    assert!((layout.zones[0].rotation - rotation).abs() < 0.001);
}

#[test]
fn attachment_strip_size_preserves_thin_signal_like_aspect() {
    let suggested = ComponentSuggestedZone {
        slot_id: "gpu".to_owned(),
        template_id: "powercolor-reddevil-rx7800xt".to_owned(),
        template_name: "PowerColor RX 7800XT Red Devil - 20 LED".to_owned(),
        name: "GPU".to_owned(),
        instance: 0,
        led_start: 0,
        led_count: 20,
        category: ComponentCategory::Strip,
        default_size: ComponentCanvasSize {
            width: 0.24,
            height: 0.08,
        },
        topology: LedTopology::Strip {
            count: 20,
            direction: StripDirection::LeftToRight,
        },
        led_mapping: None,
    };

    let size =
        layout_geometry::attachment_zone_size(&suggested, NormalizedPosition::new(0.22, 0.18));

    assert!(size.x > 0.20);
    assert!(size.y < 0.02);
}

#[test]
fn attachment_fan_size_prefers_ring_footprint_over_strip_topology() {
    let suggested = ComponentSuggestedZone {
        slot_id: "channel-1".to_owned(),
        template_id: "lian-li-sl-unifan-fan".to_owned(),
        template_name: "Lian Li UNIFan SL120 - 16 LED".to_owned(),
        name: "Front Fan".to_owned(),
        instance: 0,
        led_start: 0,
        led_count: 16,
        category: ComponentCategory::Fan,
        default_size: ComponentCanvasSize {
            width: 0.24,
            height: 0.08,
        },
        topology: LedTopology::Strip {
            count: 16,
            direction: StripDirection::LeftToRight,
        },
        led_mapping: None,
    };

    let size =
        layout_geometry::attachment_zone_size(&suggested, NormalizedPosition::new(0.22, 0.18));

    assert!((size.x - size.y).abs() < 0.01);
    assert!(size.x > 0.17);
}

#[test]
fn seeded_attachment_layout_arranges_multi_fan_slots_into_horizontal_rows() {
    let seeded = layout_geometry::seeded_attachment_layout(
        "usb:prism8:test",
        "Prism 8",
        &[
            suggested_attachment(
                "channel-1",
                "Front Fan 1",
                0,
                0,
                ComponentCategory::Fan,
                LedTopology::Ring {
                    count: 20,
                    start_angle: 0.0,
                    direction: hypercolor_types::spatial::Winding::Clockwise,
                },
            ),
            suggested_attachment(
                "channel-1",
                "Front Fan 2",
                1,
                20,
                ComponentCategory::Fan,
                LedTopology::Ring {
                    count: 20,
                    start_angle: 0.0,
                    direction: hypercolor_types::spatial::Winding::Clockwise,
                },
            ),
            suggested_attachment(
                "channel-1",
                "Front Fan 3",
                2,
                40,
                ComponentCategory::Fan,
                LedTopology::Ring {
                    count: 20,
                    start_angle: 0.0,
                    direction: hypercolor_types::spatial::Winding::Clockwise,
                },
            ),
        ],
        7,
        640.0 / 480.0,
    );

    assert_eq!(seeded.zones.len(), 3);
    assert!(seeded.zones[0].position.x < seeded.zones[1].position.x);
    assert!(seeded.zones[1].position.x < seeded.zones[2].position.x);
    assert!((seeded.zones[0].position.y - seeded.zones[1].position.y).abs() < 0.001);
    assert!((seeded.zones[1].position.y - seeded.zones[2].position.y).abs() < 0.001);
    assert_eq!(seeded.zones[0].display_order, 7);
    assert_eq!(seeded.zones[2].display_order, 9);
}

#[test]
fn seeded_attachment_layout_handles_single_slot_attachments() {
    let seeded = layout_geometry::seeded_attachment_layout(
        "wled:desk",
        "Desk Controller",
        &[suggested_attachment(
            "main",
            "Desk Strip",
            0,
            0,
            ComponentCategory::Strip,
            LedTopology::Strip {
                count: 60,
                direction: StripDirection::LeftToRight,
            },
        )],
        3,
        640.0 / 480.0,
    );

    assert_eq!(seeded.zones.len(), 1);
    assert_eq!(seeded.zones[0].display_order, 3);
}

#[test]
fn editor_normalization_gives_horizontal_strips_visible_height() {
    let size = layout_geometry::normalize_zone_size_for_editor(
        NormalizedPosition::new(0.5, 0.5),
        NormalizedPosition::new(0.24, 0.004),
        &LedTopology::Strip {
            count: 60,
            direction: StripDirection::LeftToRight,
        },
        None,
        640.0 / 480.0,
    );

    assert!((size.x - 0.24).abs() < 0.001);
    assert!((size.y - 0.03).abs() < 0.001);
    assert!(size.x / size.y <= 8.01);
}

#[test]
fn editor_normalization_gives_vertical_strips_visible_width() {
    let size = layout_geometry::normalize_zone_size_for_editor(
        NormalizedPosition::new(0.5, 0.5),
        NormalizedPosition::new(0.004, 0.24),
        &LedTopology::Strip {
            count: 60,
            direction: StripDirection::TopToBottom,
        },
        None,
        640.0 / 480.0,
    );

    assert!((size.x - 0.03).abs() < 0.001);
    assert!((size.y - 0.24).abs() < 0.001);
    assert!(size.y / size.x <= 8.01);
}

#[test]
fn locked_resize_keeps_original_aspect_ratio() {
    let (position, size) = layout_geometry::resize_zone_from_handle(
        NormalizedPosition::new(0.5, 0.5),
        NormalizedPosition::new(0.2, 0.1),
        NormalizedPosition::new(0.6, 0.55),
        ResizeHandle::SouthEast,
        NormalizedPosition::new(0.72, 0.66),
        true,
        0.0,
    );

    let aspect = size.x / size.y;
    assert!((aspect - 2.0).abs() < 0.01);
    assert!(position.x > 0.5);
    assert!(position.y > 0.5);
}

#[test]
fn locked_size_input_updates_the_other_axis() {
    let updated = layout_geometry::update_zone_size(
        NormalizedPosition::new(0.2, 0.1),
        SizeAxis::Width,
        0.3,
        true,
    );

    assert!((updated.x - 0.3).abs() < 0.001);
    assert!((updated.y - 0.15).abs() < 0.001);
}

#[test]
fn locked_width_input_on_long_strip_does_not_snap_back_up() {
    let updated = layout_geometry::update_zone_size(
        NormalizedPosition::new(0.24, 0.004),
        SizeAxis::Width,
        0.03,
        true,
    );

    assert!((updated.x - 0.03).abs() < 0.001);
    assert!((updated.y - 0.0005).abs() < 0.0002);
}

#[test]
fn free_height_input_on_long_strip_can_stay_thin() {
    let updated = layout_geometry::update_zone_size(
        NormalizedPosition::new(0.24, 0.004),
        SizeAxis::Height,
        0.001,
        false,
    );

    assert!((updated.x - 0.24).abs() < 0.001);
    assert!((updated.y - 0.001).abs() < 0.0002);
}

#[test]
fn locked_resize_can_shrink_long_strip_below_old_aspect_floor() {
    let (_, size) = layout_geometry::resize_zone_from_handle(
        NormalizedPosition::new(0.5, 0.5),
        NormalizedPosition::new(0.24, 0.004),
        NormalizedPosition::new(0.62, 0.502),
        ResizeHandle::SouthEast,
        NormalizedPosition::new(0.43, 0.498_833_33),
        true,
        0.0,
    );

    assert!((size.x - 0.05).abs() < 0.001);
    assert!((size.y - (0.05 / 60.0)).abs() < 0.0002);
}

#[test]
fn resizing_a_flush_edge_box_below_the_floor_does_not_panic() {
    // A 2% attachment seed pinned to the left edge: start_right (0.02) is
    // below the 4% resize floor, which used to hand `clamp` an inverted
    // range and abort the UI.
    let start_center = NormalizedPosition::new(0.01, 0.5);
    let start_size = NormalizedPosition::new(0.02, 0.02);
    let (position, size) = layout_geometry::resize_zone_from_handle(
        start_center,
        start_size,
        NormalizedPosition::new(0.0, 0.49),
        ResizeHandle::NorthWest,
        NormalizedPosition::new(0.05, 0.52),
        false,
        0.0,
    );
    assert!(size.x > 0.0 && size.y > 0.0);
    assert!((0.0..=1.0).contains(&position.x));
}

#[test]
fn resizing_a_rotated_box_keeps_the_opposite_corner_anchored() {
    // A box turned a quarter turn: dragging its (local) south-east handle
    // must leave the (local) north-west corner where it was on screen.
    let rotation = std::f32::consts::FRAC_PI_2;
    let start_center = NormalizedPosition::new(0.5, 0.5);
    let start_size = NormalizedPosition::new(0.2, 0.1);
    let world = |local: NormalizedPosition, center: NormalizedPosition| {
        let (sin, cos) = rotation.sin_cos();
        let dx = local.x - center.x;
        let dy = local.y - center.y;
        NormalizedPosition::new(
            center.x + dx * cos - dy * sin,
            center.y + dx * sin + dy * cos,
        )
    };
    let local_nw = NormalizedPosition::new(0.4, 0.45);
    let anchor_before = world(local_nw, start_center);
    let local_se = NormalizedPosition::new(0.6, 0.55);
    let start_mouse = world(local_se, start_center);
    let current_mouse = world(NormalizedPosition::new(0.7, 0.6), start_center);

    let (position, size) = layout_geometry::resize_zone_from_handle(
        start_center,
        start_size,
        start_mouse,
        ResizeHandle::SouthEast,
        current_mouse,
        false,
        rotation,
    );
    let new_local_nw =
        NormalizedPosition::new(position.x - size.x * 0.5, position.y - size.y * 0.5);
    // The new rect's local NW corner, expressed relative to the new center,
    // rotated about the new center, must land on the old anchor.
    let anchor_after = world(new_local_nw, position);
    assert!((size.x - 0.3).abs() < 1e-3, "width {}", size.x);
    assert!((size.y - 0.15).abs() < 1e-3, "height {}", size.y);
    assert!(
        (anchor_after.x - anchor_before.x).abs() < 1e-3,
        "{anchor_after:?} vs {anchor_before:?}"
    );
    assert!(
        (anchor_after.y - anchor_before.y).abs() < 1e-3,
        "{anchor_after:?} vs {anchor_before:?}"
    );
}

#[test]
fn circular_zones_normalize_to_a_pixel_square() {
    // 640x480 canvas: a pixel square must be taller in normalized units
    // (height fraction = width fraction * 4/3), matching both the CSS
    // aspect-ratio: 1 box and the sampled LED circle.
    let size = layout_geometry::normalize_zone_size_for_editor(
        NormalizedPosition::new(0.5, 0.5),
        NormalizedPosition::new(0.2, 0.2),
        &LedTopology::Ring {
            count: 20,
            start_angle: 0.0,
            direction: hypercolor_types::spatial::Winding::Clockwise,
        },
        Some(&ZoneShape::Ring),
        640.0 / 480.0,
    );
    assert!((size.y - size.x * (640.0 / 480.0)).abs() < 1e-4, "{size:?}");
    // The smaller pixel extent wins: 0.2 of height is the pixel-smaller side.
    assert!((size.y - 0.2).abs() < 1e-4, "{size:?}");
}

#[test]
fn explicit_rectangle_shape_overrides_ring_topology_circularity() {
    assert!(layout_geometry::is_circular_zone(
        None,
        &LedTopology::Ring {
            count: 8,
            start_angle: 0.0,
            direction: hypercolor_types::spatial::Winding::Clockwise,
        },
    ));
    assert!(!layout_geometry::is_circular_zone(
        Some(&ZoneShape::Rectangle),
        &LedTopology::Ring {
            count: 8,
            start_angle: 0.0,
            direction: hypercolor_types::spatial::Winding::Clockwise,
        },
    ));
    assert!(layout_geometry::is_circular_zone(
        Some(&ZoneShape::Ring),
        &LedTopology::Strip {
            count: 8,
            direction: StripDirection::LeftToRight,
        },
    ));
}
