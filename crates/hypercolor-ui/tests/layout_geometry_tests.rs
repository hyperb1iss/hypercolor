#![allow(dead_code)]

#[path = "../src/api/mod.rs"]
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

fn suggested_attachment(
    slot_id: &str,
    name: &str,
    instance: u32,
    led_start: u32,
    category: AttachmentCategory,
    topology: LedTopology,
) -> AttachmentSuggestedZone {
    AttachmentSuggestedZone {
        slot_id: slot_id.to_owned(),
        template_id: format!("{slot_id}-{instance}"),
        template_name: name.to_owned(),
        name: name.to_owned(),
        instance,
        led_start,
        led_count: topology.led_count(),
        category,
        default_size: AttachmentCanvasSize {
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
        zones: vec![DeviceZone {
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
        spaces: None,
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
                AttachmentCategory::Fan,
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
                AttachmentCategory::Fan,
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
                AttachmentCategory::Fan,
                LedTopology::Ring {
                    count: 20,
                    start_angle: 0.0,
                    direction: hypercolor_types::spatial::Winding::Clockwise,
                },
            ),
        ],
        7,
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
            AttachmentCategory::Strip,
            LedTopology::Strip {
                count: 60,
                direction: StripDirection::LeftToRight,
            },
        )],
        3,
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

// ── Compound bounding box ────────────────────────────────────────────────

fn plain_zone(id: &str, device_id: &str, x: f32, y: f32, w: f32, h: f32) -> DeviceZone {
    DeviceZone {
        id: id.to_owned(),
        name: id.to_owned(),
        device_id: device_id.to_owned(),
        zone_name: None,
        position: NormalizedPosition::new(x, y),
        size: NormalizedPosition::new(w, h),
        rotation: 0.0,
        scale: 1.0,
        display_order: 0,
        orientation: None,
        topology: LedTopology::Strip {
            count: 10,
            direction: StripDirection::LeftToRight,
        },
        led_positions: Vec::new(),
        led_mapping: None,
        sampling_mode: None,
        edge_behavior: None,
        shape: None,
        shape_preset: None,
        attachment: None,
        brightness: None,
    }
}

fn simple_layout(zones: Vec<DeviceZone>) -> SpatialLayout {
    SpatialLayout {
        id: "test".to_owned(),
        name: "Test".to_owned(),
        description: None,
        canvas_width: 320,
        canvas_height: 200,
        zones,
        default_sampling_mode: SamplingMode::Bilinear,
        default_edge_behavior: EdgeBehavior::Clamp,
        spaces: None,
        version: 1,
    }
}

#[test]
fn compound_bounding_box_two_zones() {
    // Zone A: center (0.3, 0.4), size (0.2, 0.1) -> spans x [0.2, 0.4], y [0.35, 0.45]
    // Zone B: center (0.7, 0.6), size (0.2, 0.1) -> spans x [0.6, 0.8], y [0.55, 0.65]
    // Combined AABB: x [0.2, 0.8], y [0.35, 0.65]
    // Expected center: (0.5, 0.5), size: (0.6, 0.3)
    let layout = simple_layout(vec![
        plain_zone("a", "dev", 0.3, 0.4, 0.2, 0.1),
        plain_zone("b", "dev", 0.7, 0.6, 0.2, 0.1),
    ]);

    let ids: std::collections::HashSet<String> =
        ["a".to_owned(), "b".to_owned()].into_iter().collect();
    let bounds = layout_geometry::compound_bounding_box(&layout, &ids)
        .expect("should produce bounds for two zones");

    assert!((bounds.center.x - 0.5).abs() < 0.001);
    assert!((bounds.center.y - 0.5).abs() < 0.001);
    assert!((bounds.size.x - 0.6).abs() < 0.001);
    assert!((bounds.size.y - 0.3).abs() < 0.001);
}

#[test]
fn compound_bounding_box_single_zone() {
    let layout = simple_layout(vec![plain_zone("solo", "dev", 0.5, 0.5, 0.2, 0.1)]);

    let ids: std::collections::HashSet<String> = ["solo".to_owned()].into_iter().collect();
    let bounds = layout_geometry::compound_bounding_box(&layout, &ids)
        .expect("should produce bounds for single zone");

    assert!((bounds.center.x - 0.5).abs() < 0.001);
    assert!((bounds.center.y - 0.5).abs() < 0.001);
    assert!((bounds.size.x - 0.2).abs() < 0.001);
    assert!((bounds.size.y - 0.1).abs() < 0.001);
}

#[test]
fn compound_bounding_box_empty_returns_none() {
    let layout = simple_layout(vec![plain_zone("a", "dev", 0.5, 0.5, 0.2, 0.1)]);

    let ids: std::collections::HashSet<String> = std::collections::HashSet::new();
    assert!(layout_geometry::compound_bounding_box(&layout, &ids).is_none());
}

// ── Translate zones ──────────────────────────────────────────────────────

#[test]
fn translate_zones_preserves_relative_positions() {
    let mut layout = simple_layout(vec![
        plain_zone("a", "dev", 0.3, 0.3, 0.1, 0.1),
        plain_zone("b", "dev", 0.5, 0.3, 0.1, 0.1),
    ]);

    let initial_positions = vec![
        ("a".to_owned(), NormalizedPosition::new(0.3, 0.3)),
        ("b".to_owned(), NormalizedPosition::new(0.5, 0.3)),
    ];
    let delta = NormalizedPosition::new(0.1, 0.2);

    layout_geometry::translate_zones(&mut layout, &initial_positions, delta);

    let a = layout.zones.iter().find(|z| z.id == "a").expect("zone a");
    let b = layout.zones.iter().find(|z| z.id == "b").expect("zone b");

    // Relative offset between a and b should be preserved (0.2 horizontal, 0.0 vertical)
    assert!((b.position.x - a.position.x - 0.2).abs() < 0.001);
    assert!((b.position.y - a.position.y).abs() < 0.001);
}

#[test]
fn translate_zones_clamps_to_canvas() {
    let mut layout = simple_layout(vec![plain_zone("a", "dev", 0.5, 0.5, 0.1, 0.1)]);

    let initial_positions = vec![("a".to_owned(), NormalizedPosition::new(0.5, 0.5))];
    // Large delta that would push past [0, 1]
    let delta = NormalizedPosition::new(5.0, 5.0);

    layout_geometry::translate_zones(&mut layout, &initial_positions, delta);

    let a = &layout.zones[0];
    assert!(a.position.x <= 1.0);
    assert!(a.position.y <= 1.0);
    assert!(a.position.x >= 0.0);
    assert!(a.position.y >= 0.0);
}

// ── Group centroid ──────────────────────────────────────────────────────

#[test]
fn group_centroid_averages_zone_positions() {
    let layout = simple_layout(vec![
        plain_zone("a", "dev", 0.2, 0.3, 0.1, 0.1),
        plain_zone("b", "dev", 0.6, 0.3, 0.1, 0.1),
        plain_zone("c", "dev", 0.4, 0.7, 0.1, 0.1),
    ]);

    let ids: std::collections::HashSet<String> =
        ["a", "b", "c"].iter().map(|s| s.to_string()).collect();
    let centroid = layout_geometry::group_centroid(&layout, &ids)
        .expect("should compute centroid for 3 zones");

    assert!((centroid.x - 0.4).abs() < 0.001);
    assert!((centroid.y - (0.3 + 0.3 + 0.7) / 3.0).abs() < 0.001);
}

#[test]
fn group_centroid_empty_returns_none() {
    let layout = simple_layout(vec![plain_zone("a", "dev", 0.5, 0.5, 0.1, 0.1)]);
    let ids = std::collections::HashSet::new();
    assert!(layout_geometry::group_centroid(&layout, &ids).is_none());
}

// ── Group translate ─────────────────────────────────────────────────────

#[test]
fn translate_group_moves_centroid_preserving_relative_positions() {
    let mut layout = simple_layout(vec![
        plain_zone("a", "dev", 0.3, 0.4, 0.1, 0.1),
        plain_zone("b", "dev", 0.5, 0.4, 0.1, 0.1),
    ]);

    let ids: std::collections::HashSet<String> = ["a", "b"].iter().map(|s| s.to_string()).collect();

    // Centroid is (0.4, 0.4). Move it to (0.6, 0.6).
    layout_geometry::translate_group(&mut layout, &ids, NormalizedPosition::new(0.6, 0.6));

    let a = layout.zones.iter().find(|z| z.id == "a").expect("a");
    let b = layout.zones.iter().find(|z| z.id == "b").expect("b");

    // Relative offset should be preserved: b is 0.2 to the right of a
    assert!((b.position.x - a.position.x - 0.2).abs() < 0.001);
    assert!((b.position.y - a.position.y).abs() < 0.001);

    // New centroid should be at (0.6, 0.6)
    assert!(((a.position.x + b.position.x) / 2.0 - 0.6).abs() < 0.001);
    assert!(((a.position.y + b.position.y) / 2.0 - 0.6).abs() < 0.001);
}

// ── Group rotate ────────────────────────────────────────────────────────

#[test]
fn rotate_group_90_degrees_orbits_and_rotates_zones() {
    let mut layout = simple_layout(vec![
        plain_zone("a", "dev", 0.4, 0.5, 0.1, 0.1),
        plain_zone("b", "dev", 0.6, 0.5, 0.1, 0.1),
    ]);

    let ids: std::collections::HashSet<String> = ["a", "b"].iter().map(|s| s.to_string()).collect();

    // Centroid is (0.5, 0.5). Rotate 90 degrees.
    let delta = std::f32::consts::FRAC_PI_2;
    layout_geometry::rotate_group(&mut layout, &ids, delta);

    let a = layout.zones.iter().find(|z| z.id == "a").expect("a");
    let b = layout.zones.iter().find(|z| z.id == "b").expect("b");

    // Zone A was at (-0.1, 0) offset from centroid.
    // After 90 CCW rotation: (0, -0.1) offset -> position (0.5, 0.4)
    assert!((a.position.x - 0.5).abs() < 0.01);
    assert!((a.position.y - 0.4).abs() < 0.01);

    // Zone B was at (0.1, 0) offset -> after 90: (0, 0.1) -> position (0.5, 0.6)
    assert!((b.position.x - 0.5).abs() < 0.01);
    assert!((b.position.y - 0.6).abs() < 0.01);

    // Both zones' individual rotation should include the 90-degree offset
    assert!((a.rotation - delta).abs() < 0.01);
    assert!((b.rotation - delta).abs() < 0.01);
}

#[test]
fn rotate_group_preserves_centroid() {
    let mut layout = simple_layout(vec![
        plain_zone("a", "dev", 0.3, 0.4, 0.1, 0.1),
        plain_zone("b", "dev", 0.5, 0.4, 0.1, 0.1),
        plain_zone("c", "dev", 0.4, 0.6, 0.1, 0.1),
    ]);

    let ids: std::collections::HashSet<String> =
        ["a", "b", "c"].iter().map(|s| s.to_string()).collect();

    let centroid_before = layout_geometry::group_centroid(&layout, &ids).expect("centroid");
    layout_geometry::rotate_group(&mut layout, &ids, 0.7); // ~40 degrees
    let centroid_after = layout_geometry::group_centroid(&layout, &ids).expect("centroid");

    assert!((centroid_before.x - centroid_after.x).abs() < 0.01);
    assert!((centroid_before.y - centroid_after.y).abs() < 0.01);
}

#[test]
fn rotate_group_zero_delta_returns_false() {
    let mut layout = simple_layout(vec![
        plain_zone("a", "dev", 0.3, 0.4, 0.1, 0.1),
        plain_zone("b", "dev", 0.5, 0.4, 0.1, 0.1),
    ]);
    let ids: std::collections::HashSet<String> = ["a", "b"].iter().map(|s| s.to_string()).collect();

    assert!(!layout_geometry::rotate_group(&mut layout, &ids, 0.0));
}

// ── Group scale ─────────────────────────────────────────────────────────

#[test]
fn scale_group_doubles_spread_and_zone_scales() {
    let mut layout = simple_layout(vec![
        plain_zone("a", "dev", 0.4, 0.5, 0.1, 0.1),
        plain_zone("b", "dev", 0.6, 0.5, 0.1, 0.1),
    ]);

    let ids: std::collections::HashSet<String> = ["a", "b"].iter().map(|s| s.to_string()).collect();

    // Centroid is (0.5, 0.5). Scale 2x.
    layout_geometry::scale_group(&mut layout, &ids, 2.0);

    let a = layout.zones.iter().find(|z| z.id == "a").expect("a");
    let b = layout.zones.iter().find(|z| z.id == "b").expect("b");

    // Zone A was at -0.1 offset from centroid, now should be -0.2 -> position 0.3
    assert!((a.position.x - 0.3).abs() < 0.01);
    // Zone B was at +0.1 offset, now +0.2 -> position 0.7
    assert!((b.position.x - 0.7).abs() < 0.01);

    // Individual scales should double
    assert!((a.scale - 2.0).abs() < 0.01);
    assert!((b.scale - 2.0).abs() < 0.01);
}

#[test]
fn scale_group_preserves_centroid() {
    let mut layout = simple_layout(vec![
        plain_zone("a", "dev", 0.3, 0.4, 0.1, 0.1),
        plain_zone("b", "dev", 0.5, 0.4, 0.1, 0.1),
        plain_zone("c", "dev", 0.4, 0.6, 0.1, 0.1),
    ]);

    let ids: std::collections::HashSet<String> =
        ["a", "b", "c"].iter().map(|s| s.to_string()).collect();

    let centroid_before = layout_geometry::group_centroid(&layout, &ids).expect("centroid");
    layout_geometry::scale_group(&mut layout, &ids, 1.5);
    let centroid_after = layout_geometry::group_centroid(&layout, &ids).expect("centroid");

    assert!((centroid_before.x - centroid_after.x).abs() < 0.01);
    assert!((centroid_before.y - centroid_after.y).abs() < 0.01);
}

#[test]
fn scale_group_identity_returns_false() {
    let mut layout = simple_layout(vec![
        plain_zone("a", "dev", 0.3, 0.4, 0.1, 0.1),
        plain_zone("b", "dev", 0.5, 0.4, 0.1, 0.1),
    ]);
    let ids: std::collections::HashSet<String> = ["a", "b"].iter().map(|s| s.to_string()).collect();

    assert!(!layout_geometry::scale_group(&mut layout, &ids, 1.0));
}

// ── Group align ─────────────────────────────────────────────────────────

#[test]
fn align_group_left_matches_bbox_left_edge() {
    let mut layout = simple_layout(vec![
        plain_zone("a", "dev", 0.20, 0.30, 0.10, 0.10),
        plain_zone("b", "dev", 0.60, 0.30, 0.20, 0.10),
        plain_zone("c", "dev", 0.45, 0.60, 0.10, 0.10),
    ]);
    let ids: std::collections::HashSet<String> =
        ["a", "b", "c"].iter().map(|s| s.to_string()).collect();

    layout_geometry::align_group(
        &mut layout,
        &ids,
        layout_geometry::AlignAxis::X,
        layout_geometry::AlignAnchor::Min,
    );

    let left_edges: Vec<f32> = layout
        .zones
        .iter()
        .filter(|z| ids.contains(&z.id))
        .map(|z| z.position.x - z.size.x * 0.5)
        .collect();
    let first = left_edges[0];
    for edge in &left_edges[1..] {
        assert!(
            (edge - first).abs() < 0.001,
            "left edges should match: {first} vs {edge}"
        );
    }
    // Y coords unchanged
    let a = layout.zones.iter().find(|z| z.id == "a").expect("a");
    assert!((a.position.y - 0.30).abs() < 0.001);
    let c = layout.zones.iter().find(|z| z.id == "c").expect("c");
    assert!((c.position.y - 0.60).abs() < 0.001);
}

#[test]
fn align_group_right_matches_bbox_right_edge() {
    let mut layout = simple_layout(vec![
        plain_zone("a", "dev", 0.20, 0.30, 0.10, 0.10),
        plain_zone("b", "dev", 0.60, 0.30, 0.20, 0.10),
    ]);
    let ids: std::collections::HashSet<String> = ["a", "b"].iter().map(|s| s.to_string()).collect();

    layout_geometry::align_group(
        &mut layout,
        &ids,
        layout_geometry::AlignAxis::X,
        layout_geometry::AlignAnchor::Max,
    );

    // Bbox right edge = 0.60 + 0.10 = 0.70
    let a = layout.zones.iter().find(|z| z.id == "a").expect("a");
    let b = layout.zones.iter().find(|z| z.id == "b").expect("b");
    assert!((a.position.x + a.size.x * 0.5 - 0.70).abs() < 0.001);
    assert!((b.position.x + b.size.x * 0.5 - 0.70).abs() < 0.001);
}

#[test]
fn align_group_center_x_matches_bbox_center() {
    let mut layout = simple_layout(vec![
        plain_zone("a", "dev", 0.20, 0.30, 0.10, 0.10),
        plain_zone("b", "dev", 0.60, 0.30, 0.20, 0.10),
    ]);
    let ids: std::collections::HashSet<String> = ["a", "b"].iter().map(|s| s.to_string()).collect();

    // a spans x [0.15, 0.25], b spans x [0.50, 0.70]. Bbox center x = 0.425.
    layout_geometry::align_group(
        &mut layout,
        &ids,
        layout_geometry::AlignAxis::X,
        layout_geometry::AlignAnchor::Center,
    );

    let a = layout.zones.iter().find(|z| z.id == "a").expect("a");
    let b = layout.zones.iter().find(|z| z.id == "b").expect("b");
    assert!((a.position.x - 0.425).abs() < 0.01);
    assert!((b.position.x - 0.425).abs() < 0.01);
}

#[test]
fn align_group_top_matches_bbox_top_edge() {
    let mut layout = simple_layout(vec![
        plain_zone("a", "dev", 0.3, 0.25, 0.10, 0.10),
        plain_zone("b", "dev", 0.5, 0.40, 0.10, 0.20),
        plain_zone("c", "dev", 0.7, 0.55, 0.10, 0.10),
    ]);
    let ids: std::collections::HashSet<String> =
        ["a", "b", "c"].iter().map(|s| s.to_string()).collect();

    layout_geometry::align_group(
        &mut layout,
        &ids,
        layout_geometry::AlignAxis::Y,
        layout_geometry::AlignAnchor::Min,
    );

    // Bbox top = 0.25 - 0.05 = 0.20
    let tops: Vec<f32> = layout
        .zones
        .iter()
        .filter(|z| ids.contains(&z.id))
        .map(|z| z.position.y - z.size.y * 0.5)
        .collect();
    for t in &tops {
        assert!((t - 0.20).abs() < 0.001);
    }
}

// ── Group distribute ────────────────────────────────────────────────────

#[test]
fn distribute_group_horizontal_equalizes_gaps() {
    let mut layout = simple_layout(vec![
        plain_zone("a", "dev", 0.10, 0.50, 0.05, 0.10),
        plain_zone("b", "dev", 0.30, 0.50, 0.05, 0.10),
        plain_zone("c", "dev", 0.90, 0.50, 0.05, 0.10),
    ]);
    let ids: std::collections::HashSet<String> =
        ["a", "b", "c"].iter().map(|s| s.to_string()).collect();

    layout_geometry::distribute_group(&mut layout, &ids, layout_geometry::AlignAxis::X);

    // First and last should be unchanged
    let a = layout.zones.iter().find(|z| z.id == "a").expect("a");
    let c = layout.zones.iter().find(|z| z.id == "c").expect("c");
    assert!((a.position.x - 0.10).abs() < 0.01);
    assert!((c.position.x - 0.90).abs() < 0.01);

    // Gap between a.right and b.left should equal gap between b.right and c.left
    let b = layout.zones.iter().find(|z| z.id == "b").expect("b");
    let gap1 = (b.position.x - b.size.x * 0.5) - (a.position.x + a.size.x * 0.5);
    let gap2 = (c.position.x - c.size.x * 0.5) - (b.position.x + b.size.x * 0.5);
    assert!((gap1 - gap2).abs() < 0.01);
}

#[test]
fn distribute_group_under_three_zones_is_noop() {
    let mut layout = simple_layout(vec![
        plain_zone("a", "dev", 0.1, 0.5, 0.05, 0.10),
        plain_zone("b", "dev", 0.9, 0.5, 0.05, 0.10),
    ]);
    let ids: std::collections::HashSet<String> = ["a", "b"].iter().map(|s| s.to_string()).collect();
    assert!(!layout_geometry::distribute_group(
        &mut layout,
        &ids,
        layout_geometry::AlignAxis::X
    ));
}

// ── Group pack ──────────────────────────────────────────────────────────

#[test]
fn pack_group_horizontal_butts_zones_edge_to_edge() {
    let mut layout = simple_layout(vec![
        plain_zone("a", "dev", 0.20, 0.50, 0.10, 0.10),
        plain_zone("b", "dev", 0.50, 0.50, 0.10, 0.10),
        plain_zone("c", "dev", 0.80, 0.50, 0.10, 0.10),
    ]);
    let ids: std::collections::HashSet<String> =
        ["a", "b", "c"].iter().map(|s| s.to_string()).collect();

    layout_geometry::pack_group(&mut layout, &ids, layout_geometry::AlignAxis::X);

    let a = layout.zones.iter().find(|z| z.id == "a").expect("a");
    let b = layout.zones.iter().find(|z| z.id == "b").expect("b");
    let c = layout.zones.iter().find(|z| z.id == "c").expect("c");

    // a anchors the sequence at its original position
    assert!((a.position.x - 0.20).abs() < 0.01);
    // b.left == a.right
    let a_right = a.position.x + a.size.x * 0.5;
    let b_left = b.position.x - b.size.x * 0.5;
    assert!((a_right - b_left).abs() < 0.001);
    // c.left == b.right
    let b_right = b.position.x + b.size.x * 0.5;
    let c_left = c.position.x - c.size.x * 0.5;
    assert!((b_right - c_left).abs() < 0.001);
}

// ── Group mirror ────────────────────────────────────────────────────────

#[test]
fn mirror_group_horizontal_flips_positions_around_centroid() {
    let mut layout = simple_layout(vec![
        plain_zone("a", "dev", 0.30, 0.50, 0.10, 0.10),
        plain_zone("b", "dev", 0.70, 0.50, 0.10, 0.10),
    ]);
    let ids: std::collections::HashSet<String> = ["a", "b"].iter().map(|s| s.to_string()).collect();

    // centroid x = 0.50. Mirror should swap effective X positions.
    layout_geometry::mirror_group(&mut layout, &ids, layout_geometry::AlignAxis::X);

    let a = layout.zones.iter().find(|z| z.id == "a").expect("a");
    let b = layout.zones.iter().find(|z| z.id == "b").expect("b");
    assert!((a.position.x - 0.70).abs() < 0.01);
    assert!((b.position.x - 0.30).abs() < 0.01);
    // Y unchanged
    assert!((a.position.y - 0.50).abs() < 0.01);
    assert!((b.position.y - 0.50).abs() < 0.01);
}

#[test]
fn mirror_group_across_vertical_axis_reflects_rotation_to_pi_minus_theta() {
    // Reflecting across a vertical line through the centroid sends a
    // segment at angle θ to angle π − θ (not −θ). Use 45° so the two
    // formulas give different answers — this test catches a bug where
    // the code naïvely negated rotation for both axes.
    let mut layout = simple_layout(vec![
        plain_zone("a", "dev", 0.30, 0.50, 0.10, 0.10),
        plain_zone("b", "dev", 0.70, 0.50, 0.10, 0.10),
    ]);
    layout.zones[0].rotation = std::f32::consts::FRAC_PI_4;
    let ids: std::collections::HashSet<String> = ["a", "b"].iter().map(|s| s.to_string()).collect();

    layout_geometry::mirror_group(&mut layout, &ids, layout_geometry::AlignAxis::X);

    let a = layout.zones.iter().find(|z| z.id == "a").expect("a");
    let expected =
        (std::f32::consts::PI - std::f32::consts::FRAC_PI_4).rem_euclid(std::f32::consts::TAU);
    assert!(
        (a.rotation - expected).abs() < 0.01,
        "expected {expected}, got {}",
        a.rotation
    );
}

#[test]
fn mirror_group_across_horizontal_axis_negates_rotation() {
    let mut layout = simple_layout(vec![
        plain_zone("a", "dev", 0.50, 0.30, 0.10, 0.10),
        plain_zone("b", "dev", 0.50, 0.70, 0.10, 0.10),
    ]);
    layout.zones[0].rotation = std::f32::consts::FRAC_PI_4;
    let ids: std::collections::HashSet<String> = ["a", "b"].iter().map(|s| s.to_string()).collect();

    layout_geometry::mirror_group(&mut layout, &ids, layout_geometry::AlignAxis::Y);

    let a = layout.zones.iter().find(|z| z.id == "a").expect("a");
    let expected = (-std::f32::consts::FRAC_PI_4).rem_euclid(std::f32::consts::TAU);
    assert!(
        (a.rotation - expected).abs() < 0.01,
        "expected {expected}, got {}",
        a.rotation
    );
}

#[test]
fn mirror_group_single_zone_is_noop() {
    let mut layout = simple_layout(vec![plain_zone("a", "dev", 0.5, 0.5, 0.1, 0.1)]);
    layout.zones[0].rotation = std::f32::consts::FRAC_PI_4;
    let ids: std::collections::HashSet<String> = ["a"].iter().map(|s| s.to_string()).collect();

    assert!(!layout_geometry::mirror_group(
        &mut layout,
        &ids,
        layout_geometry::AlignAxis::X,
    ));
    let a = layout.zones.iter().find(|z| z.id == "a").expect("a");
    // Rotation preserved
    assert!((a.rotation - std::f32::consts::FRAC_PI_4).abs() < 0.01);
}
