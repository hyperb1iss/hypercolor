#![allow(dead_code)]

#[path = "../src/api.rs"]
mod api;
#[path = "../src/layout_geometry.rs"]
mod layout_geometry;

use api::{ZoneSummary, ZoneTopologySummary};
use hypercolor_types::attachment::{
    AttachmentCanvasSize, AttachmentCategory, AttachmentSuggestedZone,
};
use hypercolor_types::spatial::{LedTopology, NormalizedPosition, StripDirection};
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

#[test]
fn basilisk_v3_uses_signal_sparse_layout_instead_of_flat_matrix() {
    let zone = zone_summary(
        "Main",
        11,
        ZoneTopologySummary::Matrix { rows: 1, cols: 11 },
    );

    let defaults = layout_geometry::default_zone_visuals("Razer Basilisk V3", Some(&zone), 11);

    match defaults.topology {
        LedTopology::Custom { positions } => assert_eq!(positions.len(), 11),
        other => panic!("expected sparse custom topology, got {other:?}"),
    }

    let aspect = defaults.size.x / defaults.size.y;
    assert!((aspect - (7.0 / 8.0)).abs() < 0.05);
    assert!(defaults.size.y > defaults.size.x * 0.9);
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
