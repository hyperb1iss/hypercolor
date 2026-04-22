#![allow(dead_code)]

#[path = "../src/compound_selection.rs"]
mod compound_selection;
#[path = "../src/layout_history.rs"]
mod layout_history;

use hypercolor_types::spatial::{
    DeviceZone, EdgeBehavior, LedTopology, NormalizedPosition, SamplingMode, SpatialLayout,
    StripDirection,
};

use compound_selection::CompoundDepth;
use layout_history::{LayoutEditorSnapshot, LayoutHistoryState, RemovedZoneCache};

fn zone(id: &str, x: f32) -> DeviceZone {
    DeviceZone {
        id: id.to_owned(),
        name: id.to_owned(),
        device_id: "device-1".to_owned(),
        zone_name: None,
        position: NormalizedPosition::new(x, 0.5),
        size: NormalizedPosition::new(0.1, 0.1),
        rotation: 0.0,
        scale: 1.0,
        display_order: 0,
        orientation: None,
        topology: LedTopology::Strip {
            count: 30,
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

fn layout(zones: Vec<DeviceZone>) -> SpatialLayout {
    SpatialLayout {
        id: "layout".to_owned(),
        name: "Layout".to_owned(),
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

fn snapshot(layout: &SpatialLayout, selected: &[&str]) -> LayoutEditorSnapshot {
    LayoutEditorSnapshot {
        zones: layout.zones.clone(),
        selected_zone_ids: selected.iter().map(|id| (*id).to_owned()).collect(),
        compound_depth: CompoundDepth::Root,
        removed_zone_cache: RemovedZoneCache::new(),
    }
}

#[test]
fn record_edit_pushes_previous_snapshot() {
    let before = layout(vec![zone("zone-1", 0.2)]);
    let after = layout(vec![zone("zone-1", 0.7)]);
    let current = snapshot(&after, &["zone-1"]);
    let mut history = LayoutHistoryState::default();

    history.record_edit(snapshot(&before, &["zone-1"]), &current);

    let restored = history.undo(current.clone()).expect("undo snapshot");
    assert_eq!(restored, snapshot(&before, &["zone-1"]));
    assert_eq!(history.redo(restored).expect("redo snapshot"), current);
}

#[test]
fn interaction_groups_drag_into_single_undo_step() {
    let start = layout(vec![zone("zone-1", 0.2)]);
    let mid = layout(vec![zone("zone-1", 0.4)]);
    let end = layout(vec![zone("zone-1", 0.8)]);
    let mut history = LayoutHistoryState::default();

    history.begin_interaction(snapshot(&start, &["zone-1"]));
    history.record_edit(snapshot(&start, &["zone-1"]), &snapshot(&mid, &["zone-1"]));
    history.record_edit(snapshot(&mid, &["zone-1"]), &snapshot(&end, &["zone-1"]));
    history.finish_interaction(&snapshot(&end, &["zone-1"]));

    let restored = history
        .undo(snapshot(&end, &["zone-1"]))
        .expect("undo grouped interaction");
    assert_eq!(restored, snapshot(&start, &["zone-1"]));
}

#[test]
fn reset_clears_undo_and_redo_stacks() {
    let before = layout(vec![zone("zone-1", 0.2)]);
    let after = layout(vec![zone("zone-1", 0.7)]);
    let mut history = LayoutHistoryState::default();

    history.record_edit(snapshot(&before, &[]), &snapshot(&after, &[]));
    let _ = history.undo(snapshot(&after, &[]));
    assert!(history.can_redo());

    history.reset();

    assert!(!history.can_undo());
    assert!(!history.can_redo());
}
