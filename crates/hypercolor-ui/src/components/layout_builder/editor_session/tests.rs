use super::{ZoneDraft, merge_draft, reconcile_replay};
use crate::compound_selection::CompoundDepth;
use crate::layout_history::{LayoutEditorSnapshot, RemovedOutputCache};
use hypercolor_types::spatial::{
    EdgeBehavior, LedTopology, NormalizedPosition, Output, SamplingMode, SpatialLayout,
    StripDirection,
};

fn zone(id: &str, x: f32) -> Output {
    Output {
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

fn layout(zones: Vec<Output>) -> SpatialLayout {
    SpatialLayout {
        id: "layout".to_owned(),
        name: "Layout".to_owned(),
        description: None,
        canvas_width: 320,
        canvas_height: 200,
        zones,
        default_sampling_mode: SamplingMode::Bilinear,
        default_edge_behavior: EdgeBehavior::Clamp,
        version: 1,
    }
}

fn snapshot(layout: &SpatialLayout, selected: &[&str]) -> LayoutEditorSnapshot {
    LayoutEditorSnapshot {
        zones: layout.zones.clone(),
        selected_zone_ids: selected.iter().map(|id| (*id).to_owned()).collect(),
        compound_depth: CompoundDepth::Root,
        removed_zone_cache: RemovedOutputCache::new(),
    }
}

#[test]
fn draft_merge_keeps_only_local_field_changes() {
    let baseline = layout(vec![zone("one", 0.2)]);
    let mut local = baseline.clone();
    local.zones[0].position.x = 0.7;
    let mut canonical = baseline.clone();
    canonical.zones[0].rotation = 1.0;
    canonical.zones[0].name = "Updated device name".to_owned();
    canonical.zones.push(zone("new", 0.9));
    let merged = merge_draft(
        &canonical,
        &ZoneDraft {
            baseline,
            snapshot: snapshot(&local, &["one"]),
        },
    );
    assert_eq!(merged.zones[0].position.x, 0.7);
    assert_eq!(merged.zones[0].rotation, 1.0);
    assert_eq!(merged.zones[0].name, "Updated device name");
    assert_eq!(merged.zones[1], canonical.zones[1]);
}

#[test]
fn clean_draft_accepts_canonical_placement_changes() {
    let baseline = layout(vec![zone("one", 0.2)]);
    let canonical = layout(vec![zone("one", 0.8)]);
    let merged = merge_draft(
        &canonical,
        &ZoneDraft {
            snapshot: snapshot(&baseline, &[]),
            baseline,
        },
    );
    assert_eq!(merged.zones, canonical.zones);
}

#[test]
fn brightness_draft_survives_refresh_and_replays_both_directions() {
    let before = layout(vec![zone("one", 0.2)]);
    let mut after = before.clone();
    after.zones[0].brightness = Some(0.4);
    let mut canonical = before.clone();
    canonical.zones[0].position.x = 0.8;
    let merged = merge_draft(
        &canonical,
        &ZoneDraft {
            baseline: before.clone(),
            snapshot: snapshot(&after, &["one"]),
        },
    );
    assert_eq!(merged.zones[0].brightness, Some(0.4));
    assert_eq!(merged.zones[0].position.x, 0.8);

    canonical.zones = merged.zones;
    let mut undo = snapshot(&before, &["one"]);
    reconcile_replay(&canonical, &snapshot(&after, &[]), &mut undo);
    assert_eq!(undo.zones[0].brightness, None);
    assert_eq!(undo.zones[0].position.x, 0.8);

    canonical.zones = undo.zones;
    let mut redo = snapshot(&after, &["one"]);
    reconcile_replay(&canonical, &snapshot(&before, &[]), &mut redo);
    assert_eq!(redo.zones[0].brightness, Some(0.4));
    assert_eq!(redo.zones[0].position.x, 0.8);
}

#[test]
fn replay_preserves_current_membership_and_binding() {
    let old = layout(vec![
        zone("removed", 0.1),
        zone("one", 0.2),
        zone("rebound", 0.3),
    ]);
    let mut canonical = layout(vec![
        zone("one", 0.8),
        zone("new", 0.9),
        zone("rebound", 0.6),
    ]);
    canonical.zones[2].device_id = "replacement-device".to_owned();
    let mut previous = snapshot(&old, &[]);
    previous.zones[1].position.x = 0.8;
    previous.zones[2].position.x = 0.6;
    let mut replay = snapshot(&old, &["removed", "one"]);
    reconcile_replay(&canonical, &previous, &mut replay);
    assert_eq!(replay.zones.len(), 3);
    assert_eq!(replay.zones[0].position.x, 0.2);
    assert_eq!(replay.zones[1], canonical.zones[1]);
    assert_eq!(replay.zones[2], canonical.zones[2]);
    assert_eq!(
        replay.selected_zone_ids,
        ["one".to_owned()].into_iter().collect()
    );
}

#[test]
fn undo_position_preserves_unrelated_remote_fields_and_other_outputs() {
    let before = layout(vec![zone("one", 0.2), zone("two", 0.3)]);
    let mut after = before.clone();
    after.zones[0].position.x = 0.7;
    let mut current = after.clone();
    current.zones[0].size = NormalizedPosition::new(0.25, 0.35);
    current.zones[0].position.y = 0.8;
    current.zones[1].position.x = 0.9;
    let mut undo = snapshot(&before, &["one"]);
    reconcile_replay(&current, &snapshot(&after, &["one"]), &mut undo);
    assert_eq!(undo.zones[0].position, NormalizedPosition::new(0.2, 0.8));
    assert_eq!(undo.zones[0].size, current.zones[0].size);
    assert_eq!(undo.zones[1], current.zones[1]);

    current.zones.clone_from(&undo.zones);
    let mut redo = snapshot(&after, &["one"]);
    reconcile_replay(&current, &snapshot(&before, &["one"]), &mut redo);
    assert_eq!(redo.zones[0].position, NormalizedPosition::new(0.7, 0.8));
    assert_eq!(redo.zones[0].size, current.zones[0].size);
    assert_eq!(redo.zones[1], current.zones[1]);
}
