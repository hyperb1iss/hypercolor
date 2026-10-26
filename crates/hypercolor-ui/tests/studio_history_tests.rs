use hypercolor_types::api::scene::{MemberEdit, MemberState, PatchZoneRequest, SceneDocument};
use hypercolor_types::layer::{
    BlendMode, LayerAdjust, LayerSource, LayerTransform, SceneLayer, SceneLayerId,
};
use hypercolor_types::scene::{ZoneId, ZoneRole};
use hypercolor_ui::compound_selection::CompoundDepth;
use hypercolor_ui::layout_history::LayoutEditorSnapshot;
use hypercolor_ui::pages::studio::history::{
    DeletedZone, StudioEdit, StudioJournal, held_document, held_scene, light_neighbor,
    members_landed, prepare_members_replay, rename_is_noop, replay_announcement, resolve_deletion,
    restore_layer_requests, settles_by_key, zone_patch_before,
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

fn layer(name: &str, rgba: [f32; 4], blend: BlendMode, opacity: f32) -> SceneLayer {
    SceneLayer {
        id: SceneLayerId::new(),
        name: Some(name.to_owned()),
        source: LayerSource::ColorFill { rgba },
        blend,
        opacity,
        transform: LayerTransform::default(),
        adjust: LayerAdjust::default(),
        bindings: Vec::new(),
        enabled: false,
    }
}

#[test]
fn a_deleted_zone_keeps_its_layers_and_restores_them_bottom_to_top() {
    let mut scene = scene();
    let wash = layer("Wash", [0.2, 0.4, 0.6, 1.0], BlendMode::Alpha, 1.0);
    let glow = layer("Glow", [1.0, 0.0, 0.5, 1.0], BlendMode::Screen, 0.4);
    scene.zones[0].layers = vec![wash.clone(), glow.clone()];
    let record = DeletedZone::of(&scene.zones[0], 0, 7);
    assert_eq!(record.token, 7);
    assert_eq!(record.zone_id, scene.zones[0].id.to_string());
    assert_eq!(record.layers, vec![wash.clone(), glow.clone()]);

    let requests = restore_layer_requests(&record);
    assert_eq!(requests.len(), 2);
    for (request, layer) in requests.iter().zip([&wash, &glow]) {
        assert_eq!(request.name, layer.name);
        assert_eq!(request.source, layer.source);
        assert_eq!(request.blend, Some(layer.blend));
        assert_eq!(request.opacity, Some(layer.opacity));
        assert_eq!(request.transform, Some(layer.transform));
        assert_eq!(request.adjust, Some(layer.adjust));
        assert_eq!(request.bindings.as_ref(), Some(&layer.bindings));
        assert_eq!(request.enabled, Some(layer.enabled));
    }
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

fn zones_scene(zones: &[(&str, &str)]) -> SceneDocument {
    let zones: Vec<_> = zones
        .iter()
        .map(|(id, role)| {
            serde_json::json!({ "id": id, "name": id, "role": role, "enabled": true,
                "brightness": 1, "members": [], "layers": [] })
        })
        .collect();
    serde_json::from_value(serde_json::json!({
        "id": "33333333-3333-4333-8333-333333333333", "name": "Desk", "kind": "named",
        "is_default": false, "revision": 3, "zones": zones,
    }))
    .expect("valid scene fixture")
}

const ZONE_A: &str = "11111111-1111-4111-8111-111111111111";
const ZONE_B: &str = "22222222-2222-4222-8222-222222222222";
const ZONE_C: &str = "44444444-4444-4444-8444-444444444444";
const SCREEN: &str = "55555555-5555-4555-8555-555555555555";

#[test]
fn a_removed_zone_hands_the_selection_to_the_light_tab_beside_it() {
    let scene = zones_scene(&[
        (ZONE_A, "primary"),
        (SCREEN, "display"),
        (ZONE_B, "custom"),
        (ZONE_C, "custom"),
    ]);
    // The tab before it, skipping Screens; the first tab falls to the next.
    assert_eq!(light_neighbor(&scene, ZONE_B).as_deref(), Some(ZONE_A));
    assert_eq!(light_neighbor(&scene, ZONE_C).as_deref(), Some(ZONE_B));
    assert_eq!(light_neighbor(&scene, ZONE_A).as_deref(), Some(ZONE_B));
}

#[test]
fn the_only_light_zone_or_an_unknown_one_has_no_neighbor() {
    let scene = zones_scene(&[(ZONE_A, "primary"), (SCREEN, "display")]);
    assert_eq!(light_neighbor(&scene, ZONE_A), None);
    assert_eq!(light_neighbor(&scene, ZONE_C), None);
}

// ── Undo and redo announcements ──────────────────────────────────────────

fn stand_move(from: Option<&str>, to: Option<&str>) -> MemberEdit {
    let scene = scene();
    let output = hypercolor_ui::api::zone_outputs(&scene.zones[0]).remove(0);
    let state = |zone: &str| MemberState {
        zone_id: zone.parse::<uuid::Uuid>().map(ZoneId).expect("zone id"),
        output: output.clone(),
        index: 0,
    };
    MemberEdit {
        before: from.map(state),
        after: to.map(state),
    }
}

fn zone_names(id: &str) -> Option<(String, ZoneRole)> {
    match id {
        ZONE_A => Some(("Case".to_owned(), ZoneRole::Primary)),
        ZONE_B => Some(("Keyboard".to_owned(), ZoneRole::Custom)),
        _ => None,
    }
}

fn whole_stand(_: &str) -> Option<(String, usize)> {
    Some(("Laptop stand".to_owned(), 1))
}

fn hub_channel(_: &str) -> Option<(String, usize)> {
    Some(("Nollie 32".to_owned(), 4))
}

#[test]
fn undoing_a_move_says_where_the_device_went_back_to() {
    let edit = StudioEdit::Members(vec![stand_move(Some(ZONE_A), Some(ZONE_B))]);
    assert_eq!(
        replay_announcement(&edit, false, &zone_names, &whole_stand).as_deref(),
        Some("Undid: moved Laptop stand back to Case")
    );
    assert_eq!(
        replay_announcement(&edit, true, &zone_names, &whole_stand).as_deref(),
        Some("Redid: moved Laptop stand to Keyboard")
    );
}

#[test]
fn a_channel_is_announced_by_its_own_name_and_unassigned_by_name() {
    let edit = StudioEdit::Members(vec![stand_move(None, Some(ZONE_A))]);
    assert_eq!(
        replay_announcement(&edit, false, &zone_names, &hub_channel).as_deref(),
        Some("Undid: moved Stand (Nollie 32) back to Unassigned")
    );
    let placed = StudioEdit::Members(vec![stand_move(Some(ZONE_A), Some(ZONE_A))]);
    assert_eq!(
        replay_announcement(&placed, true, &zone_names, &whole_stand).as_deref(),
        Some("Redid: placed Laptop stand on Case")
    );
}

#[test]
fn zone_steps_are_announced_by_what_they_did() {
    let made = StudioEdit::ZoneCreated {
        zone_id: ZONE_B.to_owned(),
        name: "Keyboard".to_owned(),
        color: None,
        filled: vec![stand_move(Some(ZONE_A), Some(ZONE_B))],
    };
    assert_eq!(
        replay_announcement(&made, false, &zone_names, &whole_stand).as_deref(),
        Some("Undid: moved Laptop stand back to Case")
    );
    assert_eq!(
        replay_announcement(&made, true, &zone_names, &whole_stand).as_deref(),
        Some("Redid: moved Laptop stand to a new zone")
    );
    let empty = StudioEdit::ZoneCreated {
        zone_id: ZONE_B.to_owned(),
        name: "New zone".to_owned(),
        color: None,
        filled: Vec::new(),
    };
    assert_eq!(
        replay_announcement(&empty, false, &zone_names, &whole_stand).as_deref(),
        Some("Undid: created New zone")
    );
    let rename = StudioEdit::ZonePatched {
        zone_id: ZONE_A.to_owned(),
        before: PatchZoneRequest {
            name: Some("Case".to_owned()),
            ..Default::default()
        },
        after: PatchZoneRequest {
            name: Some("Tower".to_owned()),
            ..Default::default()
        },
    };
    assert_eq!(
        replay_announcement(&rename, false, &zone_names, &whole_stand).as_deref(),
        Some("Undid: renamed Case to Tower")
    );
    let pause = StudioEdit::ZonePatched {
        zone_id: ZONE_A.to_owned(),
        before: PatchZoneRequest {
            enabled: Some(true),
            ..Default::default()
        },
        after: PatchZoneRequest {
            enabled: Some(false),
            ..Default::default()
        },
    };
    assert_eq!(
        replay_announcement(&pause, true, &zone_names, &whole_stand).as_deref(),
        Some("Redid: paused Case")
    );
    let deleted = StudioEdit::ZoneDeleted(Box::new(DeletedZone::of(&scene().zones[0], 0, 1)));
    assert_eq!(
        replay_announcement(&deleted, false, &zone_names, &whole_stand).as_deref(),
        Some("Undid: deleted Case")
    );
}

#[test]
fn an_empty_membership_step_has_nothing_to_say() {
    let edit = StudioEdit::Members(Vec::new());
    assert_eq!(
        replay_announcement(&edit, true, &zone_names, &whole_stand),
        None
    );
}

#[test]
fn a_legacy_main_zone_is_announced_and_renamed_by_the_name_its_tab_shows() {
    let legacy = |id: &str| (id == ZONE_A).then(|| ("Default zone".to_owned(), ZoneRole::Primary));
    let pause = StudioEdit::ZonePatched {
        zone_id: ZONE_A.to_owned(),
        before: PatchZoneRequest {
            enabled: Some(true),
            ..Default::default()
        },
        after: PatchZoneRequest {
            enabled: Some(false),
            ..Default::default()
        },
    };
    assert_eq!(
        replay_announcement(&pause, false, &legacy, &whole_stand).as_deref(),
        Some("Undid: paused Main")
    );

    let mut scene = scene();
    scene.zones[0].name = "Default zone".to_owned();
    scene.zones[0].role = ZoneRole::Primary;
    let zone = &scene.zones[0];
    assert!(rename_is_noop(zone, "Main"), "the tab already says Main");
    assert!(rename_is_noop(zone, "  "));
    assert!(rename_is_noop(zone, "Default zone"));
    assert!(!rename_is_noop(zone, "Desk"));
}

#[test]
fn an_unknown_delete_resolves_by_what_the_scene_shows_after_it() {
    let scene = scene();
    let mut layered = scene.zones[0].clone();
    layered.layers = vec![layer("Glow", [1.0, 0.0, 0.5, 1.0], BlendMode::Screen, 0.5)];
    let record = DeletedZone::of(&layered, 0, 3);
    let release = vec![MemberEdit {
        before: Some(MemberState {
            zone_id: scene.zones[0].id,
            output: hypercolor_ui::api::zone_outputs(&scene.zones[0]).remove(0),
            index: 0,
        }),
        after: None,
    }];
    // The delete landed: its record, layers and all, is what Undo rebuilds
    // from.
    match resolve_deletion(true, &record, &release) {
        Some(StudioEdit::ZoneDeleted(deleted)) => assert_eq!(deleted.layers.len(), 1),
        other => panic!("expected a delete, got {other:?}"),
    }
    // It did not: only the release happened.
    assert_eq!(
        resolve_deletion(false, &record, &release),
        Some(StudioEdit::Members(release.clone()))
    );
    // It did not, and nothing was released (a zone of layers alone):
    // nothing changed.
    assert_eq!(resolve_deletion(false, &record, &[]), None);
}

#[test]
fn an_unknown_delete_goes_out_again_under_its_key_only_in_its_own_run() {
    assert!(
        settles_by_key("run-a", "run-a"),
        "its key still means something"
    );
    assert!(
        !settles_by_key("run-a", "run-b"),
        "a new run: the read says what the old one did"
    );
    assert!(
        !settles_by_key("", ""),
        "a daemon that names no runs gets the read"
    );
}

#[test]
fn a_failed_read_is_not_a_switch_to_another_scene() {
    let desk = scene().id;
    let other = hypercolor_types::scene::SceneId::new();
    assert_eq!(
        held_scene(None, Some(desk)),
        Some(desk),
        "a failed read keeps the scene"
    );
    assert_eq!(held_scene(Some(desk), Some(desk)), Some(desk));
    assert_eq!(
        held_scene(Some(other), Some(desk)),
        Some(other),
        "a real switch moves"
    );
    assert_eq!(held_scene(None, None), None);
}

#[test]
fn a_held_membership_edit_reads_as_landed_only_when_the_scene_holds_it() {
    let scene = scene();
    let zone = &scene.zones[0];
    let stand = hypercolor_ui::api::zone_outputs(zone).remove(0);
    let release = vec![MemberEdit {
        before: Some(MemberState {
            zone_id: zone.id,
            output: stand.clone(),
            index: 0,
        }),
        after: None,
    }];
    // The stand is still in the zone, so the release did not land.
    assert!(!members_landed(&scene, &release));
    let mut released = scene.clone();
    released.zones[0].members.clear();
    assert!(members_landed(&released, &release));
    // Putting it back landed where the scene holds it again.
    let restore = vec![MemberEdit {
        before: None,
        after: Some(MemberState {
            zone_id: zone.id,
            output: stand,
            index: 0,
        }),
    }];
    assert!(members_landed(&scene, &restore));
    assert!(!members_landed(&released, &restore));
    // An assignment names no outputs, so a read tells nothing about it.
    assert!(!members_landed(&scene, &[]));
}

#[test]
fn a_held_placement_change_reads_as_landed_only_where_it_put_the_output() {
    let scene = scene();
    let zone = &scene.zones[0];
    let stand = hypercolor_ui::api::zone_outputs(zone).remove(0);
    let mut moved = stand.clone();
    moved.position.x = 0.3;
    let placement = vec![MemberEdit {
        before: Some(MemberState {
            zone_id: zone.id,
            output: stand.clone(),
            index: 0,
        }),
        after: Some(MemberState {
            zone_id: zone.id,
            output: moved,
            index: 0,
        }),
    }];
    // The stand still sits where it was: the placement did not land, though
    // the zone holds it either way.
    assert!(!members_landed(&scene, &placement));
    let mut placed = scene.clone();
    placed.zones[0]
        .layout
        .as_mut()
        .expect("a laid out zone")
        .placements[0]
        .position
        .x = 0.3;
    assert!(members_landed(&placed, &placement));
}

#[test]
fn a_failed_scene_read_leaves_studio_on_the_scene_it_last_read() {
    use leptos::prelude::*;
    let owner = Owner::new();
    owner.with(|| {
        let shared = RwSignal::new(Some(scene()));
        let read = Memo::new(move |_| shared.get());
        let held = held_document(read);
        assert_eq!(held.get().map(|scene| scene.revision), Some(8));
        // The read failed: no news, so Studio stays where it was (a drag
        // under way and the selection with it).
        shared.set(None);
        assert_eq!(held.get().map(|scene| scene.revision), Some(8));
        // A real read moves it on.
        let mut next = scene();
        next.revision = 9;
        shared.set(Some(next));
        assert_eq!(held.get().map(|scene| scene.revision), Some(9));
    });
    owner.cleanup();
}
