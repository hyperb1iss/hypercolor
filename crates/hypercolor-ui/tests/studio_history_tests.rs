use hypercolor_types::api::scene::{MemberEdit, MemberState, SceneDocument};
use hypercolor_ui::compound_selection::CompoundDepth;
use hypercolor_ui::layout_history::LayoutEditorSnapshot;
use hypercolor_ui::pages::studio::history::{StudioEdit, StudioJournal, prepare_members_replay};
use std::collections::{HashMap, HashSet};

fn snapshot() -> LayoutEditorSnapshot {
    LayoutEditorSnapshot {
        zones: Vec::new(),
        selected_zone_ids: HashSet::new(),
        compound_depth: CompoundDepth::Root,
        removed_zone_cache: HashMap::new(),
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
