#![allow(dead_code)]

#[path = "../src/compound_selection.rs"]
mod compound_selection;

use std::collections::HashSet;

use compound_selection::{CompoundDepth, device_compound_ids, resolve_click, slot_compound_ids};
use hypercolor_types::spatial::{
    DeviceZone, LedTopology, NormalizedPosition, SpatialLayout, StripDirection, ZoneAttachment,
};

// ── Fixtures ─────────────────────────────────────────────────────────────

fn test_zone(id: &str, device_id: &str, attachment: Option<(&str, u32)>) -> DeviceZone {
 brightness: None,
    DeviceZone {
        id: id.to_owned(),
        name: id.to_owned(),
        device_id: device_id.to_owned(),
        zone_name: None,
        position: NormalizedPosition::new(0.5, 0.5),
        size: NormalizedPosition::new(0.1, 0.1),
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
        attachment: attachment.map(|(slot_id, instance)| ZoneAttachment {
        brightness: None,
            template_id: "test-template".to_owned(),
            slot_id: slot_id.to_owned(),
            instance,
            led_start: None,
            led_count: None,
            led_mapping: None,
        }),
    }
}

fn test_layout(zones: Vec<DeviceZone>) -> SpatialLayout {
    SpatialLayout {
        id: "test".to_owned(),
        name: "Test Layout".to_owned(),
        description: None,
        canvas_width: 320,
        canvas_height: 200,
        zones,
        default_sampling_mode: hypercolor_types::spatial::SamplingMode::Bilinear,
        default_edge_behavior: hypercolor_types::spatial::EdgeBehavior::Clamp,
        spaces: None,
        version: 1,
    }
}

// ── device_compound_ids ──────────────────────────────────────────────────

#[test]
fn device_compound_ids_returns_all_zones_for_device() {
    let layout = test_layout(vec![
        test_zone("z1", "dev-a", None),
        test_zone("z2", "dev-a", None),
        test_zone("z3", "dev-b", None),
        test_zone("z4", "dev-b", None),
        test_zone("z5", "dev-b", None),
    ]);

    let ids = device_compound_ids(&layout, "dev-a");
    assert_eq!(ids, HashSet::from(["z1".to_owned(), "z2".to_owned()]));

    let ids_b = device_compound_ids(&layout, "dev-b");
    assert_eq!(
        ids_b,
        HashSet::from(["z3".to_owned(), "z4".to_owned(), "z5".to_owned()])
    );
}

// ── slot_compound_ids ────────────────────────────────────────────────────

#[test]
fn slot_compound_ids_returns_attachment_zones_for_slot() {
    let layout = test_layout(vec![
        test_zone("fan-0", "controller", Some(("channel-1", 0))),
        test_zone("fan-1", "controller", Some(("channel-1", 1))),
        test_zone("strip-0", "controller", Some(("channel-2", 0))),
    ]);

    let ids = slot_compound_ids(&layout, "controller", "channel-1");
    assert_eq!(ids, HashSet::from(["fan-0".to_owned(), "fan-1".to_owned()]));
}

#[test]
fn slot_compound_ids_excludes_non_attachment_zones() {
    let layout = test_layout(vec![
        test_zone("fan-0", "controller", Some(("channel-1", 0))),
        test_zone("plain", "controller", None),
    ]);

    let ids = slot_compound_ids(&layout, "controller", "channel-1");
    assert_eq!(ids, HashSet::from(["fan-0".to_owned()]));
    assert!(!ids.contains("plain"));
}

// ── resolve_click ────────────────────────────────────────────────────────

#[test]
fn resolve_click_at_root_returns_device_compound() {
    let layout = test_layout(vec![
        test_zone("z1", "dev-a", None),
        test_zone("z2", "dev-a", None),
        test_zone("z3", "dev-b", None),
    ]);

    let selected = resolve_click(&layout, "z1", &CompoundDepth::Root);
    assert_eq!(selected, HashSet::from(["z1".to_owned(), "z2".to_owned()]));
}

#[test]
fn resolve_click_at_device_depth_returns_slot_compound() {
    let layout = test_layout(vec![
        test_zone("fan-0", "ctrl", Some(("ch1", 0))),
        test_zone("fan-1", "ctrl", Some(("ch1", 1))),
        test_zone("strip-0", "ctrl", Some(("ch2", 0))),
    ]);

    let depth = CompoundDepth::Device {
        device_id: "ctrl".to_owned(),
    };
    let selected = resolve_click(&layout, "fan-0", &depth);
    assert_eq!(
        selected,
        HashSet::from(["fan-0".to_owned(), "fan-1".to_owned()])
    );
}

#[test]
fn resolve_click_at_device_depth_returns_single_for_non_attachment() {
    let layout = test_layout(vec![
        test_zone("main-zone", "dev-a", None),
        test_zone("fan-0", "dev-a", Some(("ch1", 0))),
    ]);

    let depth = CompoundDepth::Device {
        device_id: "dev-a".to_owned(),
    };
    let selected = resolve_click(&layout, "main-zone", &depth);
    assert_eq!(selected, HashSet::from(["main-zone".to_owned()]));
}

#[test]
fn resolve_click_at_slot_depth_returns_single_zone() {
    let layout = test_layout(vec![
        test_zone("fan-0", "ctrl", Some(("ch1", 0))),
        test_zone("fan-1", "ctrl", Some(("ch1", 1))),
    ]);

    let depth = CompoundDepth::Slot {
        device_id: "ctrl".to_owned(),
        slot_id: "ch1".to_owned(),
    };
    let selected = resolve_click(&layout, "fan-0", &depth);
    assert_eq!(selected, HashSet::from(["fan-0".to_owned()]));
}
