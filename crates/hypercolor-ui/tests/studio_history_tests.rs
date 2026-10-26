use hypercolor_types::api::scene::{MemberEdit, MemberState, PatchZoneRequest, SceneDocument};
use hypercolor_types::scene::ZoneId;
use hypercolor_ui::compound_selection::CompoundDepth;
use hypercolor_ui::layout_history::LayoutEditorSnapshot;
use hypercolor_ui::pages::studio::history::{
    DeletedZone, StudioEdit, StudioJournal, prepare_members_replay, zone_patch_before,
};
use std::collections::HashSet;

fn snapshot() -> LayoutEditorSnapshot {
    LayoutEditorSnapshot {
        zones: Vec::new(),
        selected_zone_ids: HashSet::new(),
        compound_depth: CompoundDepth::Root,
    }
}

fn scene() -> SceneDocument {
    serde_json::from_value(serde_json::json!({
        "id":"33333333-3333-4333-8333-333333333333", "name":"Desk", "kind":"named", "is_default":false,"revision":8,
        "zones":[{"id":"11111111-1111-4111-8111-111111111111","name":"Case","role":"primary","enabled":true,"brightness":1,
            "members":[{"id":"stand","device_id":"usb:stand","segment":"Main","name":"Stand"}],
            "layout":{"placements":[{"member":"stand","position":{"x":0.7,"y":0.5},"size":{"x":0.1,"y":0.1},"topology":{"type":"strip","count":15,"direction":"left_to_right"}}]},"layers":[] }]
    })).expect("valid scene fixture")
}

#[test]
fn journal_interleaves_membership_and_geometry_and_discards_only_redo_branch() {
    let mut journal = StudioJournal::default();
    assert!(journal.undo_edit().is_none());
    assert!(journal.redo_edit().is_none());
    let layout = StudioEdit::Layout {
        zone_id: "zone".into(),
        before: Box::new(snapshot()),
        after: Box::new(snapshot()),
    };
    let members = StudioEdit::Members(Vec::new());
    journal.record(layout.clone());
    journal.record(members.clone());
    assert_eq!(journal.undo_edit(), Some(&members));
    journal.complete(false);
    assert_eq!(journal.undo_edit(), Some(&layout));
    assert_eq!(journal.redo_edit(), Some(&members));
    journal.complete(false);
    assert!(journal.undo_edit().is_none());
    journal.complete(true);
    assert_eq!(journal.redo_edit(), Some(&members));
    journal.record(layout.clone());
    assert!(journal.redo_edit().is_none());
    assert_eq!(journal.undo_edit(), Some(&layout));
}

#[test]
fn undoing_a_move_returns_an_untouched_output_to_its_recorded_placement() {
    let scene = scene();
    let zone = &scene.zones[0];
    // The move left the output where it sits now; before it, the output
    // sat at x 0.2 in another zone.
    let landed = hypercolor_ui::api::zone_outputs(zone).remove(0);
    let mut original = landed.clone();
    original.position.x = 0.2;
    original.size.x = 0.3;
    let undo = MemberEdit {
        before: Some(MemberState {
            zone_id: zone.id,
            output: landed,
            index: 0,
        }),
        after: Some(MemberState {
            zone_id: zone.id,
            output: original,
            index: 0,
        }),
    };
    let replay = prepare_members_replay(&scene, vec![undo]).expect("same binding");
    let restored = &replay[0].after.as_ref().expect("after").output;
    assert_eq!(restored.position.x, 0.2);
    assert_eq!(restored.size.x, 0.3);
}

#[test]
fn membership_replay_preserves_newer_geometry_but_rejects_changed_binding() {
    let scene = scene();
    let zone = &scene.zones[0];
    let mut output = hypercolor_ui::api::zone_outputs(zone).remove(0);
    output.position.x = 0.2;
    let before = MemberState {
        zone_id: zone.id,
        output: output.clone(),
        index: 0,
    };
    let after = MemberState {
        zone_id: zone.id,
        output,
        index: 1,
    };
    let change = MemberEdit {
        before: Some(before),
        after: Some(after),
    };
    let replay = prepare_members_replay(&scene, vec![change.clone()]).expect("same binding");
    assert_eq!(
        replay[0].before.as_ref().expect("before").output.position.x,
        0.7
    );
    assert_eq!(
        replay[0].after.as_ref().expect("after").output.position.x,
        0.7
    );
    let mut replaced = scene.clone();
    replaced.zones[0].members[0].device_id = "different".into();
    assert!(prepare_members_replay(&replaced, vec![change]).is_err());
}

#[test]
fn placement_replay_restores_the_geometry_the_edit_recorded() {
    let scene = scene();
    let zone = &scene.zones[0];
    let live = hypercolor_ui::api::zone_outputs(zone).remove(0);
    // A drop moved the output from x 0.7 to x 0.3. Replaying it while the
    // output still sits where the drop found it lands it at the recorded
    // 0.3, rather than leaving it at its live spot.
    let mut dropped = live.clone();
    dropped.position.x = 0.3;
    dropped.size.x = 0.2;
    let original = live.clone();
    let change = MemberEdit {
        before: Some(MemberState {
            zone_id: zone.id,
            output: original,
            index: 0,
        }),
        after: Some(MemberState {
            zone_id: zone.id,
            output: dropped,
            index: 0,
        }),
    };
    let replay = prepare_members_replay(&scene, vec![change]).expect("same binding");
    let after = &replay[0].after.as_ref().expect("after").output;
    assert_eq!(after.position.x, 0.3);
    assert_eq!(after.size.x, 0.2);
    assert_eq!(after.name, live.name);
    assert_eq!(
        replay[0].before.as_ref().expect("before").output.position.x,
        0.7
    );
}

#[test]
fn removed_output_restoration_keeps_recorded_snapshot_without_live_hardware() {
    let scene = scene();
    let zone = &scene.zones[0];
    let output = hypercolor_ui::api::zone_outputs(zone).remove(0);
    let change = MemberEdit {
        before: None,
        after: Some(MemberState {
            zone_id: zone.id,
            output,
            index: 0,
        }),
    };
    assert_eq!(
        prepare_members_replay(&scene, vec![change.clone()]).expect("server validates restore"),
        vec![change]
    );
}

#[test]
fn a_zone_patch_remembers_only_the_fields_it_changes() {
    let mut scene = scene();
    scene.zones[0].color = Some("#ff00aa".to_owned());
    let zone = &scene.zones[0];
    let rename = PatchZoneRequest {
        name: Some("Desk".to_owned()),
        ..Default::default()
    };
    assert_eq!(
        zone_patch_before(zone, 2, &rename),
        PatchZoneRequest {
            name: Some("Case".to_owned()),
            ..Default::default()
        }
    );
    let restyle = PatchZoneRequest {
        color: Some(None),
        enabled: Some(false),
        index: Some(0),
        ..Default::default()
    };
    assert_eq!(
        zone_patch_before(zone, 2, &restyle),
        PatchZoneRequest {
            color: Some(Some("#ff00aa".to_owned())),
            enabled: Some(true),
            index: Some(2),
            ..Default::default()
        }
    );
}

#[test]
fn a_deletion_is_undoable_once_and_follows_its_zone_to_a_new_id() {
    let scene = scene();
    let zone = &scene.zones[0];
    let old = zone.id.to_string();
    let mut journal = StudioJournal::default();
    journal.record(StudioEdit::ZonePatched {
        zone_id: old.clone(),
        before: PatchZoneRequest::default(),
        after: PatchZoneRequest::default(),
    });
    journal.record(StudioEdit::ZoneDeleted(Box::new(DeletedZone::of(
        zone, 0, 3,
    ))));
    assert_eq!(journal.undoable_deletion(3), Some(1));
    assert_eq!(journal.undoable_deletion(4), None);

    // Undoing the deletion steps past it; it can no longer be undone.
    journal.complete(false);
    assert_eq!(journal.undoable_deletion(3), None);

    // The restore made the zone again under a new id; older entries follow.
    let new = ZoneId::new();
    journal.remap_zone(&old, new);
    match journal.undo_edit() {
        Some(StudioEdit::ZonePatched { zone_id, .. }) => assert_eq!(*zone_id, new.to_string()),
        other => panic!("expected the rename, found {other:?}"),
    }
    match journal.redo_edit() {
        Some(StudioEdit::ZoneDeleted(record)) => assert_eq!(record.zone_id, new.to_string()),
        other => panic!("expected the deletion, found {other:?}"),
    }
}
