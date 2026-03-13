#![allow(dead_code)]

#[path = "../src/api.rs"]
mod api;
#[path = "../src/layout_geometry.rs"]
mod layout_geometry;

use api::{ZoneSummary, ZoneTopologySummary};
use hypercolor_types::attachment::{
    AttachmentCanvasSize, AttachmentCategory, AttachmentSuggestedZone,
};
use hypercolor_types::spatial::{
    DeviceZone, EdgeBehavior, LedTopology, NormalizedPosition, SamplingMode, SpatialLayout,
};
use hypercolor_types::spatial::{StripDirection, ZoneShape};
use layout_geometry::{ResizeHandle, SizeAxis};

fn zone_summary(name: &str, led_count: usize, topology_hint: ZoneTopologySummary) -> ZoneSummary {
    ZoneSummary {
        id: format!("zone-{name}"),
        name: name.to_owned(),
        led_count,
        topology: "custom".to_owned(),
        topology_hint: Some(topology_hint),
    }
}

fn rendered_aspect(size: NormalizedPosition, canvas_width: u32, canvas_height: u32) -> f32 {
    let canvas_width = f32::from(u16::try_from(canvas_width).unwrap_or(u16::MAX));
    let canvas_height = f32::from(u16::try_from(canvas_height).unwrap_or(u16::MAX));
    let canvas_aspect = canvas_width / canvas_height;
    (size.x / size.y) * canvas_aspect
}

fn push2_zone_summaries() -> Vec<ZoneSummary> {
    vec![
        zone_summary("Pads", 64, ZoneTopologySummary::Matrix { rows: 8, cols: 8 }),
        zone_summary("Buttons Above", 8, ZoneTopologySummary::Strip),
        zone_summary("Buttons Below", 8, ZoneTopologySummary::Strip),
        zone_summary("Scene Launch", 8, ZoneTopologySummary::Strip),
        zone_summary("Transport", 4, ZoneTopologySummary::Custom),
        zone_summary("White Buttons", 37, ZoneTopologySummary::Custom),
        zone_summary("Touch Strip", 31, ZoneTopologySummary::Strip),
        zone_summary(
            "Display",
            0,
            ZoneTopologySummary::Display {
                width: 960,
                height: 160,
                circular: false,
            },
        ),
    ]
}

#[test]
fn basilisk_v3_uses_signal_sparse_layout_instead_of_flat_matrix() {
    let zone = zone_summary(
        "Main",
        11,
        ZoneTopologySummary::Matrix { rows: 1, cols: 11 },
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
        ZoneTopologySummary::Display {
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
fn seeded_push2_layout_creates_grouped_device_footprint() {
    let seeded = layout_geometry::seeded_device_layout(
        "usb:2982:1967:001-12",
        "Ableton Push 2",
        &push2_zone_summaries(),
        320,
        200,
        12,
    )
    .expect("push2 should produce a seeded layout");

    assert_eq!(seeded.group_name, "Ableton Push 2");
    assert_eq!(seeded.group_color, "#80ffea");
    assert_eq!(seeded.zones.len(), 8);
    assert!(
        seeded
            .zones
            .iter()
            .all(|zone| zone.group_id.as_deref() == Some(seeded.group_id.as_str()))
    );

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
fn drag_zone_to_position_moves_grouped_members_together() {
    let mut layout = SpatialLayout {
        id: "default".to_owned(),
        name: "Default".to_owned(),
        description: None,
        canvas_width: 320,
        canvas_height: 200,
        zones: vec![
            DeviceZone {
                id: "zone-a".to_owned(),
                name: "A".to_owned(),
                device_id: "usb:a".to_owned(),
                zone_name: Some("A".to_owned()),
                group_id: Some("push2".to_owned()),
                position: NormalizedPosition::new(0.3, 0.4),
                size: NormalizedPosition::new(0.2, 0.1),
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
            },
            DeviceZone {
                id: "zone-b".to_owned(),
                name: "B".to_owned(),
                device_id: "usb:a".to_owned(),
                zone_name: Some("B".to_owned()),
                group_id: Some("push2".to_owned()),
                position: NormalizedPosition::new(0.6, 0.6),
                size: NormalizedPosition::new(0.1, 0.2),
                rotation: 0.0,
                scale: 1.0,
                orientation: None,
                topology: LedTopology::Strip {
                    count: 8,
                    direction: StripDirection::TopToBottom,
                },
                led_positions: Vec::new(),
                led_mapping: None,
                sampling_mode: None,
                edge_behavior: None,
                shape: Some(ZoneShape::Rectangle),
                shape_preset: None,
                display_order: 1,
                attachment: None,
            },
        ],
        groups: Vec::new(),
        default_sampling_mode: SamplingMode::Bilinear,
        default_edge_behavior: EdgeBehavior::Clamp,
        spaces: None,
        version: 1,
    };

    assert!(layout_geometry::drag_zone_to_position(
        &mut layout,
        "zone-a",
        NormalizedPosition::new(0.45, 0.55),
    ));

    assert!((layout.zones[0].position.x - 0.45).abs() < 0.001);
    assert!((layout.zones[0].position.y - 0.55).abs() < 0.001);
    assert!((layout.zones[1].position.x - 0.75).abs() < 0.001);
    assert!((layout.zones[1].position.y - 0.75).abs() < 0.001);
}

#[test]
fn drag_zone_to_position_unlocks_independent_movement_and_clamps_group_bounds() {
    let mut layout = SpatialLayout {
        id: "default".to_owned(),
        name: "Default".to_owned(),
        description: None,
        canvas_width: 320,
        canvas_height: 200,
        zones: vec![
            DeviceZone {
                id: "zone-a".to_owned(),
                name: "A".to_owned(),
                device_id: "usb:a".to_owned(),
                zone_name: Some("A".to_owned()),
                group_id: Some("push2".to_owned()),
                position: NormalizedPosition::new(0.35, 0.4),
                size: NormalizedPosition::new(0.2, 0.1),
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
            },
            DeviceZone {
                id: "zone-b".to_owned(),
                name: "B".to_owned(),
                device_id: "usb:a".to_owned(),
                zone_name: Some("B".to_owned()),
                group_id: Some("push2".to_owned()),
                position: NormalizedPosition::new(0.8, 0.4),
                size: NormalizedPosition::new(0.2, 0.1),
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
                display_order: 1,
                attachment: None,
            },
        ],
        groups: Vec::new(),
        default_sampling_mode: SamplingMode::Bilinear,
        default_edge_behavior: EdgeBehavior::Clamp,
        spaces: None,
        version: 1,
    };

    assert!(layout_geometry::drag_zone_to_position(
        &mut layout,
        "zone-a",
        NormalizedPosition::new(0.9, 0.4),
    ));
    assert!((layout.zones[0].position.x - 0.45).abs() < 0.001);
    assert!((layout.zones[1].position.x - 0.9).abs() < 0.001);

    layout.zones[1].group_id = None;
    assert!(layout_geometry::drag_zone_to_position(
        &mut layout,
        "zone-b",
        NormalizedPosition::new(0.2, 0.7),
    ));
    assert!((layout.zones[0].position.x - 0.45).abs() < 0.001);
    assert!((layout.zones[0].position.y - 0.4).abs() < 0.001);
    assert!((layout.zones[1].position.x - 0.2).abs() < 0.001);
    assert!((layout.zones[1].position.y - 0.7).abs() < 0.001);
}

#[test]
fn repair_legacy_lcd_defaults_updates_untouched_square_display_zone() {
    let mut layout = SpatialLayout {
        id: "default".to_owned(),
        name: "Default".to_owned(),
        description: None,
        canvas_width: 320,
        canvas_height: 200,
        zones: vec![DeviceZone {
            id: "lcd-zone".to_owned(),
            name: "LCD".to_owned(),
            device_id: "usb:lcd".to_owned(),
            zone_name: Some("Display".to_owned()),
            group_id: None,
            position: NormalizedPosition::new(0.5, 0.5),
            size: NormalizedPosition::new(0.24, 0.24),
            rotation: 0.0,
            scale: 1.0,
            orientation: None,
            topology: LedTopology::Matrix {
                width: 480,
                height: 480,
                serpentine: false,
                start_corner: hypercolor_types::spatial::Corner::TopLeft,
            },
            led_positions: Vec::new(),
            led_mapping: None,
            sampling_mode: Some(SamplingMode::Bilinear),
            edge_behavior: Some(EdgeBehavior::Clamp),
            shape: Some(ZoneShape::Rectangle),
            shape_preset: Some("lcd-display".to_owned()),
            display_order: 0,
            attachment: None,
        }],
        groups: Vec::new(),
        default_sampling_mode: SamplingMode::Bilinear,
        default_edge_behavior: EdgeBehavior::Clamp,
        spaces: None,
        version: 1,
    };

    assert!(layout_geometry::repair_legacy_lcd_defaults(&mut layout));
    assert!((layout.zones[0].size.x - 0.15).abs() < 0.001);
    assert!((layout.zones[0].size.y - 0.24).abs() < 0.001);
    assert!((rendered_aspect(layout.zones[0].size, 320, 200) - 1.0).abs() < 0.01);
}

#[test]
fn attachment_strip_size_preserves_thin_signal_like_aspect() {
    let suggested = AttachmentSuggestedZone {
        slot_id: "gpu".to_owned(),
        template_id: "powercolor-reddevil-rx7800xt".to_owned(),
        template_name: "PowerColor RX 7800XT Red Devil - 20 LED".to_owned(),
        name: "GPU".to_owned(),
        instance: 0,
        led_start: 0,
        led_count: 20,
        category: AttachmentCategory::Strip,
        default_size: AttachmentCanvasSize {
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
    let suggested = AttachmentSuggestedZone {
        slot_id: "channel-1".to_owned(),
        template_id: "lian-li-sl-unifan-fan".to_owned(),
        template_name: "Lian Li UNIFan SL120 - 16 LED".to_owned(),
        name: "Front Fan".to_owned(),
        instance: 0,
        led_start: 0,
        led_count: 16,
        category: AttachmentCategory::Fan,
        default_size: AttachmentCanvasSize {
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
fn editor_normalization_gives_horizontal_strips_visible_height() {
    let size = layout_geometry::normalize_zone_size_for_editor(
        NormalizedPosition::new(0.5, 0.5),
        NormalizedPosition::new(0.24, 0.004),
        &LedTopology::Strip {
            count: 60,
            direction: StripDirection::LeftToRight,
        },
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
    );

    assert!((size.x - 0.05).abs() < 0.001);
    assert!((size.y - (0.05 / 60.0)).abs() < 0.0002);
}
