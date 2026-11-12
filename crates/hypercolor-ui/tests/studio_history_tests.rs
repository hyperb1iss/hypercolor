use hypercolor_types::api::scene::{
    LayerMove, LayerSlot, MemberEdit, MemberState, PatchZoneRequest, SceneDocument,
};
use hypercolor_types::layer::{
    BlendMode, LayerAdjust, LayerScope, LayerSource, LayerTransform, SceneLayer, SceneLayerId,
};
use hypercolor_types::scene::{ZoneId, ZoneRole};
use hypercolor_ui::api::ApiError;
use hypercolor_ui::compound_selection::CompoundDepth;
use hypercolor_ui::layout_history::LayoutEditorSnapshot;
use hypercolor_ui::pages::studio::history::{
    DeletedZone, Settled, StudioEdit, StudioJournal, carried_announcement, dropped_step_notice,
    held_document, held_scene, light_neighbor, members_landed, members_replay_request,
    prepare_members_replay, refuses_step, rename_is_noop, replaced_layer_id, replay_announcement,
    replay_layer_moves, replayable_scope, resolve_deletion, restore_layer_requests, settles_by_key,
    zone_patch_before,
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
    let members = StudioEdit::members(Vec::new());
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
        scope: None,
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

    let requests = restore_layer_requests(&record, &HashSet::new());
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
        assert_eq!(request.scope, None, "a whole-zone layer stays whole-zone");
    }
}

#[test]
fn a_restored_scoped_layer_is_never_wider_than_it_was() {
    let scoped = |name: &str, outputs: &[&str]| {
        let mut layer = layer(name, [1.0, 0.0, 0.5, 1.0], BlendMode::Screen, 1.0);
        layer.scope = Some(LayerScope {
            outputs: outputs.iter().map(|id| (*id).to_owned()).collect(),
        });
        layer
    };
    let mut scene = scene();
    scene.zones[0].layers = vec![
        layer("Wash", [0.2, 0.4, 0.6, 1.0], BlendMode::Alpha, 1.0),
        scoped("Keys", &["out-keys", "out-logo"]),
        scoped("Strip", &["out-strip"]),
    ];
    let record = DeletedZone::of(&scene.zones[0], 0, 7);

    let all_back = HashSet::from(["out-keys", "out-logo", "out-strip"]);
    let requests = restore_layer_requests(&record, &all_back);
    assert_eq!(
        requests
            .iter()
            .map(|request| request.scope.clone())
            .collect::<Vec<_>>(),
        record
            .layers
            .iter()
            .map(|layer| layer.scope.clone())
            .collect::<Vec<_>>(),
        "every scope comes back as it was"
    );

    // The logo and the strip were claimed by another zone before the undo.
    let some_back = HashSet::from(["out-keys"]);
    let requests = restore_layer_requests(&record, &some_back);
    assert_eq!(
        requests
            .iter()
            .map(|request| (request.name.as_deref(), request.scope.clone()))
            .collect::<Vec<_>>(),
        [
            (Some("Wash"), None),
            (
                Some("Keys"),
                Some(LayerScope {
                    outputs: vec!["out-keys".to_owned()],
                })
            ),
        ],
        "a scope narrows to what came back, and a layer with nothing left is not rebuilt"
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
    let deletion = journal.record(StudioEdit::ZoneDeleted(Box::new(DeletedZone::of(
        zone, 0, 3,
    ))));
    assert_eq!(journal.undoable_deletion(3), Some(deletion));
    assert!(journal.is_latest(deletion));
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

fn patched(name: &str) -> StudioEdit {
    StudioEdit::ZonePatched {
        zone_id: "zone".into(),
        before: PatchZoneRequest::default(),
        after: PatchZoneRequest {
            name: Some(name.to_owned()),
            ..PatchZoneRequest::default()
        },
    }
}

fn unresolved(name: &str) -> StudioEdit {
    StudioEdit::Unresolved {
        name: name.to_owned(),
    }
}

#[test]
fn a_replay_settles_its_own_place_whatever_settles_before_it() {
    let mut journal = StudioJournal::default();
    let held = journal.record(unresolved("the change to Desk"));
    let deletion = patched("Deleted");
    journal.record(deletion.clone());
    // Undo of the newest step starts, and the older held write settles as
    // having done nothing before the undo lands.
    let (replayed, edit) = journal.replayed(false).expect("a step to undo");
    assert_eq!(edit, &deletion);
    journal.settle(held, Settled::Gone);
    journal.settle(replayed, Settled::Unapplied(deletion.clone()));
    assert_eq!(
        journal.undo_edit(),
        None,
        "the undo stepped past its own step"
    );
    assert_eq!(journal.redo_edit(), Some(&deletion));
}

#[test]
fn an_undo_with_no_answer_keeps_its_step_until_it_settles() {
    let first = patched("First");
    let second = patched("Second");
    for landed in [false, true] {
        let mut journal = StudioJournal::default();
        journal.record(first.clone());
        let (id, _) = {
            journal.record(second.clone());
            journal.replayed(false).expect("a step to undo")
        };
        journal.hold(id, "undoing the change".to_owned());
        assert_eq!(journal.waiting_on(false), Some("undoing the change"));
        assert_eq!(journal.waiting_on(true), Some("undoing the change"));
        assert_eq!(journal.undo_edit(), Some(&unresolved("undoing the change")));
        assert_eq!(
            journal.redo_edit(),
            None,
            "the step is not on the redo side"
        );
        if landed {
            journal.settle(id, Settled::Unapplied(second.clone()));
            assert_eq!(journal.undo_edit(), Some(&first));
            assert_eq!(journal.redo_edit(), Some(&second));
        } else {
            journal.settle(id, Settled::Applied(second.clone()));
            assert_eq!(journal.undo_edit(), Some(&second), "Undo can try again");
            assert_eq!(journal.redo_edit(), None);
        }
        assert_eq!(journal.waiting_on(false), None);
    }
}

#[test]
fn a_held_redo_survives_new_edits_and_gives_way_to_them_if_it_did_nothing() {
    let first = patched("First");
    let later = patched("Later");
    let mut journal = StudioJournal::default();
    journal.record(first.clone());
    journal.complete(false);
    let (id, edit) = journal.replayed(true).expect("a step to redo");
    assert_eq!(edit, &first);
    // The redo's place sits among the applied steps while it waits, so a
    // new edit does not drop it along with the redo branch.
    journal.hold(id, "redoing the change".to_owned());
    assert_eq!(journal.waiting_on(true), Some("redoing the change"));
    journal.record(later.clone());
    assert_eq!(journal.edit(id), Some(&unresolved("redoing the change")));
    // It did nothing, and the newer edit took the redo branch it would sit
    // in, so it goes.
    journal.settle(id, Settled::Unapplied(first));
    assert_eq!(journal.edit(id), None);
    assert_eq!(journal.undo_edit(), Some(&later));
    assert_eq!(journal.redo_edit(), None);
}

#[test]
fn a_held_undo_that_lands_after_newer_edits_were_undone_redoes_in_order() {
    let (first, held, newer) = (patched("First"), patched("Held"), patched("Newer"));
    let mut journal = StudioJournal::default();
    journal.record(first.clone());
    let id = journal.record(held.clone());
    journal.hold(id, "undoing the change".to_owned());
    journal.record(newer.clone());
    // Newer is undone while the held undo waits.
    journal.complete(false);
    journal.settle(id, Settled::Unapplied(held.clone()));
    assert_eq!(journal.undo_edit(), Some(&first));
    assert_eq!(journal.redo_edit(), Some(&held));
    journal.complete(true);
    assert_eq!(journal.redo_edit(), Some(&newer));
}

#[test]
fn a_held_step_settles_in_place_and_a_held_write_that_did_nothing_goes() {
    let (first, landed) = (patched("First"), patched("Landed"));
    let mut journal = StudioJournal::default();
    let gone = journal.record(unresolved("the assignment"));
    let kept = journal.record(unresolved("the change to Desk"));
    journal.record(first.clone());
    // A new edit's held write never moves the cursor as it settles, so a
    // redo past it need not wait; an undo of it does.
    journal.complete(false);
    assert_eq!(journal.waiting_on(true), None);
    assert_eq!(journal.waiting_on(false), Some("the change to Desk"));
    journal.complete(true);
    journal.settle(kept, Settled::Applied(landed.clone()));
    journal.settle(gone, Settled::Gone);
    assert_eq!(journal.undo_edit(), Some(&first));
    journal.complete(false);
    assert_eq!(journal.undo_edit(), Some(&landed));
    journal.complete(false);
    assert_eq!(journal.undo_edit(), None);
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
    let edit = StudioEdit::members(vec![stand_move(Some(ZONE_A), Some(ZONE_B))]);
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
    let edit = StudioEdit::members(vec![stand_move(None, Some(ZONE_A))]);
    assert_eq!(
        replay_announcement(&edit, false, &zone_names, &hub_channel).as_deref(),
        Some("Undid: moved Stand (Nollie 32) back to Unassigned")
    );
    let placed = StudioEdit::members(vec![stand_move(Some(ZONE_A), Some(ZONE_A))]);
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
        layer_moves: Vec::new(),
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
        layer_moves: Vec::new(),
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
    let edit = StudioEdit::members(Vec::new());
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
        Some(StudioEdit::members(release.clone()))
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

// ── Layers that follow their outputs, and changes of target ─────────────

const RIPPLES: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
const GLOW: &str = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb";

fn layer_id(id: &str) -> SceneLayerId {
    SceneLayerId(id.parse().expect("layer id"))
}

fn zone_id(id: &str) -> ZoneId {
    id.parse::<uuid::Uuid>().map(ZoneId).expect("zone id")
}

fn scoped(ids: &[&str]) -> LayerScope {
    LayerScope {
        outputs: ids.iter().map(|id| (*id).to_owned()).collect(),
    }
}

fn stack_layer(id: &str, scope: Option<&[&str]>) -> serde_json::Value {
    let mut layer = serde_json::json!({
        "id": id, "name": "Layer",
        "source": { "type": "color_fill", "rgba": [0.1, 0.2, 0.3, 1.0] },
        "blend": "screen", "opacity": 1.0, "enabled": true,
    });
    if let Some(ids) = scope {
        layer["scope"] = serde_json::json!({ "outputs": ids });
    }
    layer
}

/// The scene after the keyboard moved from Case to Keyboard and took its
/// Ripples layer along: Case holds the stand and its base layer, Keyboard
/// holds the keys under a base layer and Ripples. `keyboard_layers` are
/// Keyboard's layers above its base.
fn carried_scene(keyboard_layers: &[serde_json::Value]) -> SceneDocument {
    let base = "cccccccc-cccc-4ccc-8ccc-cccccccccccc";
    let member = |id: &str| serde_json::json!({ "id": id, "device_id": format!("usb:{id}"), "segment": "Main", "name": id });
    let placement = |id: &str| {
        serde_json::json!({ "member": id, "position": { "x": 0.5, "y": 0.5 },
        "size": { "x": 0.1, "y": 0.1 }, "topology": { "type": "strip", "count": 15, "direction": "left_to_right" } })
    };
    let mut layers = vec![stack_layer("dddddddd-dddd-4ddd-8ddd-dddddddddddd", None)];
    layers.extend(keyboard_layers.iter().cloned());
    serde_json::from_value(serde_json::json!({
        "id": "33333333-3333-4333-8333-333333333333", "name": "Desk", "kind": "named",
        "is_default": false, "revision": 9,
        "zones": [
            { "id": ZONE_A, "name": "Case", "role": "primary", "enabled": true, "brightness": 1,
              "members": [member("stand")], "layout": { "placements": [placement("stand")] },
              "layers": [stack_layer(base, None)] },
            { "id": ZONE_B, "name": "Keyboard", "role": "custom", "enabled": true, "brightness": 1,
              "members": [member("keys")], "layout": { "placements": [placement("keys")] },
              "layers": layers },
        ],
    }))
    .expect("valid scene fixture")
}

/// The keyboard's move between two zones, as a receipt holds it.
fn keys_move(from: &str, to: &str) -> MemberEdit {
    let scene = carried_scene(&[]);
    let output = hypercolor_ui::api::zone_outputs(&scene.zones[1]).remove(0);
    let state = |zone: &str| MemberState {
        zone_id: zone_id(zone),
        output: output.clone(),
        index: 0,
    };
    MemberEdit {
        before: Some(state(from)),
        after: Some(state(to)),
    }
}

/// Ripples went from the top of Case's stack to the top of Keyboard's.
fn ripples_carried() -> LayerMove {
    LayerMove {
        layer_id: layer_id(RIPPLES),
        from: LayerSlot {
            zone_id: zone_id(ZONE_A),
            index: 1,
        },
        to: LayerSlot {
            zone_id: zone_id(ZONE_B),
            index: 1,
        },
        scope_before: scoped(&["keys"]),
        scope_after: scoped(&["keys"]),
    }
}

#[test]
fn undoing_a_move_sends_its_carried_layer_back_to_its_place() {
    let scene = carried_scene(&[stack_layer(RIPPLES, Some(&["keys"]))]);
    let recorded = vec![ripples_carried()];
    let back = vec![keys_move(ZONE_B, ZONE_A)];

    let moves = replay_layer_moves(&scene, &back, &recorded, false)
        .expect("the scene still matches the receipt");

    assert_eq!(moves, vec![ripples_carried().reversed()]);
    assert_eq!(moves[0].from.zone_id, zone_id(ZONE_B));
    assert_eq!(
        moves[0].to,
        LayerSlot {
            zone_id: zone_id(ZONE_A),
            index: 1,
        },
        "the layer returns to the stack position it left"
    );
}

#[test]
fn redoing_a_move_carries_the_same_layer_again() {
    // Undone: Ripples is back on top of Case, beside the keys.
    let mut scene = carried_scene(&[]);
    let ripples: SceneLayer =
        serde_json::from_value(stack_layer(RIPPLES, Some(&["keys"]))).expect("a layer");
    scene.zones[0].layers.push(ripples);
    let keys = scene.zones[1].members.remove(0);
    scene.zones[0].members.push(keys);
    let recorded = vec![ripples_carried()];
    let forward = vec![keys_move(ZONE_A, ZONE_B)];

    assert_eq!(
        replay_layer_moves(&scene, &forward, &recorded, true),
        Some(recorded)
    );
}

#[test]
fn a_move_that_carried_nothing_leaves_the_carrying_to_the_daemon() {
    let scene = carried_scene(&[]);
    let back = vec![keys_move(ZONE_B, ZONE_A)];
    assert_eq!(replay_layer_moves(&scene, &back, &[], false), None);
}

#[test]
fn a_carried_layer_replaced_since_is_left_to_the_daemons_rule() {
    // A blend change minted a new id for the layer the receipt names.
    let scene = carried_scene(&[stack_layer(GLOW, Some(&["keys"]))]);
    let back = vec![keys_move(ZONE_B, ZONE_A)];
    assert_eq!(
        replay_layer_moves(&scene, &back, &[ripples_carried()], false),
        None,
        "a move that names a layer no longer there would be refused"
    );

    // The same layer with its target changed since is not where the
    // receipt says either.
    let scene = carried_scene(&[stack_layer(RIPPLES, Some(&["keys", "stand"]))]);
    assert_eq!(
        replay_layer_moves(&scene, &back, &[ripples_carried()], false),
        None
    );
}

#[test]
fn a_layer_added_to_the_moved_output_since_is_not_stranded() {
    // Ripples is where the receipt left it, and a second layer was put on
    // the keys after the move. Naming only Ripples would leave that one
    // behind with no output, so the daemon's rule takes both.
    let scene = carried_scene(&[
        stack_layer(RIPPLES, Some(&["keys"])),
        stack_layer(GLOW, Some(&["keys"])),
    ]);
    let back = vec![keys_move(ZONE_B, ZONE_A)];
    assert_eq!(
        replay_layer_moves(&scene, &back, &[ripples_carried()], false),
        None
    );
}

#[test]
fn undoing_a_move_widens_the_layer_it_narrowed() {
    // Glow lit the stand and the keys; the keys left and it narrowed to
    // the stand, staying in Case at the same place.
    let mut scene = carried_scene(&[]);
    let glow: SceneLayer =
        serde_json::from_value(stack_layer(GLOW, Some(&["stand"]))).expect("a layer");
    scene.zones[0].layers.push(glow);
    let narrowed = LayerMove {
        layer_id: layer_id(GLOW),
        from: LayerSlot {
            zone_id: zone_id(ZONE_A),
            index: 1,
        },
        to: LayerSlot {
            zone_id: zone_id(ZONE_A),
            index: 1,
        },
        scope_before: scoped(&["keys", "stand"]),
        scope_after: scoped(&["stand"]),
    };
    let back = vec![keys_move(ZONE_B, ZONE_A)];

    let moves = replay_layer_moves(&scene, &back, &[narrowed], false)
        .expect("the narrowed layer is where the receipt left it");

    assert_eq!(moves[0].scope_before, scoped(&["stand"]));
    assert_eq!(moves[0].scope_after, scoped(&["keys", "stand"]));
}

#[test]
fn a_move_that_carried_layers_says_how_many() {
    let changes = vec![keys_move(ZONE_A, ZONE_B)];
    let keyboard = |_: &str| Some(("K70".to_owned(), 1));

    assert_eq!(
        carried_announcement(&changes, &[ripples_carried()], &zone_names, &keyboard).as_deref(),
        Some("Moved K70 to Keyboard with 1 layer")
    );
    let mut second = ripples_carried();
    second.layer_id = layer_id(GLOW);
    assert_eq!(
        carried_announcement(
            &changes,
            &[ripples_carried(), second],
            &zone_names,
            &keyboard
        )
        .as_deref(),
        Some("Moved K70 to Keyboard with 2 layers")
    );
}

#[test]
fn a_move_that_only_narrowed_a_layer_reads_as_any_other() {
    let changes = vec![keys_move(ZONE_A, ZONE_B)];
    let mut narrowed = ripples_carried();
    narrowed.to = narrowed.from;
    let keyboard = |_: &str| Some(("K70".to_owned(), 1));

    assert_eq!(
        carried_announcement(&changes, &[narrowed], &zone_names, &keyboard),
        None
    );
    assert_eq!(
        carried_announcement(&changes, &[], &zone_names, &keyboard),
        None
    );
}

fn scope_edit(layer: SceneLayerId) -> StudioEdit {
    StudioEdit::LayerScope {
        zone_id: ZONE_A.to_owned(),
        layer_id: layer,
        name: "Ripples".to_owned(),
        before: None,
        after: Some(scoped(&["keys"])),
    }
}

#[test]
fn a_change_of_target_is_one_step_that_undoes_and_redoes() {
    let mut journal = StudioJournal::default();
    let place = journal.record(scope_edit(layer_id(RIPPLES)));

    let (id, edit) = journal.replayed(false).expect("one step to undo");
    assert_eq!(id, place);
    assert_eq!(edit, &scope_edit(layer_id(RIPPLES)));

    // The undo replaced the layer, which minted a new id: the place keeps
    // the step under it, on the redo side.
    journal.settle(place, Settled::Unapplied(scope_edit(layer_id(GLOW))));
    assert!(journal.undo_edit().is_none(), "one step, taken back whole");
    assert_eq!(journal.redo_edit(), Some(&scope_edit(layer_id(GLOW))));

    journal.settle(place, Settled::Applied(scope_edit(layer_id(RIPPLES))));
    assert_eq!(journal.undo_edit(), Some(&scope_edit(layer_id(RIPPLES))));
    assert!(journal.redo_edit().is_none());
}

#[test]
fn every_step_that_names_a_layer_follows_it_through_a_replace() {
    let mut journal = StudioJournal::default();
    journal.record(StudioEdit::Members {
        changes: vec![keys_move(ZONE_A, ZONE_B)],
        layer_moves: vec![ripples_carried()],
    });
    journal.record(scope_edit(layer_id(RIPPLES)));
    let minted = SceneLayerId::new();

    // The layer sits in Keyboard after its move; a blend change there
    // minted it a new id.
    journal.remap_layer(ZONE_B, layer_id(RIPPLES), minted);

    assert_eq!(
        journal.undo_edit(),
        Some(&scope_edit(layer_id(RIPPLES))),
        "a layer of the same id in another zone is another layer"
    );
    journal.complete(false);
    match journal.undo_edit() {
        Some(StudioEdit::Members { layer_moves, .. }) => {
            assert_eq!(layer_moves[0].layer_id, minted);
        }
        other => panic!("expected the move, found {other:?}"),
    }

    journal.remap_layer(ZONE_A, layer_id(RIPPLES), minted);
    assert_eq!(journal.redo_edit(), Some(&scope_edit(minted)));
}

#[test]
fn a_replace_keeps_its_place_in_the_stack_under_a_new_id() {
    let layers = |ids: &[&str]| -> Vec<SceneLayer> {
        ids.iter()
            .map(|id| serde_json::from_value(stack_layer(id, None)).expect("a layer"))
            .collect()
    };
    let before = layers(&[RIPPLES, GLOW]);
    let minted = "eeeeeeee-eeee-4eee-8eee-eeeeeeeeeeee";

    assert_eq!(
        replaced_layer_id(&before, &layers(&[RIPPLES, minted]), layer_id(GLOW)),
        Some(layer_id(minted))
    );
    // Stacks that do not line up say nothing about which layer is which.
    assert_eq!(
        replaced_layer_id(&before, &layers(&[minted]), layer_id(GLOW)),
        None
    );
    assert_eq!(
        replaced_layer_id(&before, &layers(&[RIPPLES, minted]), SceneLayerId::new()),
        None
    );
}

#[test]
fn an_undone_target_comes_back_narrower_never_wider() {
    let members: HashSet<&str> = ["keys", "stand"].into_iter().collect();

    // The whole zone is always there to go back to.
    assert_eq!(
        replayable_scope(None, Some(&scoped(&["keys"])), &members),
        Some(None)
    );
    assert_eq!(
        replayable_scope(Some(&scoped(&["stand", "keys"])), None, &members),
        Some(Some(scoped(&["keys", "stand"])))
    );
    // An output that left since is dropped, unless the layer still holds
    // it, which a replace is allowed to carry over.
    assert_eq!(
        replayable_scope(Some(&scoped(&["keys", "mouse"])), None, &members),
        Some(Some(scoped(&["keys"])))
    );
    assert_eq!(
        replayable_scope(
            Some(&scoped(&["keys", "mouse"])),
            Some(&scoped(&["mouse"])),
            &members
        ),
        Some(Some(scoped(&["keys", "mouse"])))
    );
    // Nothing left of it: the step cannot replay, and must not become the
    // whole zone.
    assert_eq!(
        replayable_scope(Some(&scoped(&["mouse"])), None, &members),
        None
    );
}

#[test]
fn undoing_a_change_of_target_names_the_layer() {
    let edit = scope_edit(layer_id(RIPPLES));
    assert_eq!(
        replay_announcement(&edit, false, &zone_names, &whole_stand).as_deref(),
        Some("Undid: changed where Ripples applies")
    );
    assert_eq!(
        replay_announcement(&edit, true, &zone_names, &whole_stand).as_deref(),
        Some("Redid: changed where Ripples applies")
    );
}

#[test]
fn a_recreated_zone_takes_its_carried_layers_records_along() {
    let mut journal = StudioJournal::default();
    journal.record(StudioEdit::Members {
        changes: vec![keys_move(ZONE_A, ZONE_B)],
        layer_moves: vec![ripples_carried()],
    });
    journal.record(scope_edit(layer_id(RIPPLES)));
    let remade = ZoneId::new();

    journal.remap_zone(ZONE_B, remade);

    journal.complete(false);
    match journal.undo_edit() {
        Some(StudioEdit::Members { layer_moves, .. }) => {
            assert_eq!(layer_moves[0].to.zone_id, remade);
            assert_eq!(layer_moves[0].from.zone_id, zone_id(ZONE_A));
        }
        other => panic!("expected the move, found {other:?}"),
    }
    journal.remap_zone(ZONE_A, remade);
    match journal.redo_edit() {
        Some(StudioEdit::LayerScope { zone_id, .. }) => assert_eq!(zone_id, &remade.to_string()),
        other => panic!("expected the change of target, found {other:?}"),
    }
}

// ── A zone made for a device, and the layers that came along ────────────

/// The keyboard went into a zone made for it and took Ripples along.
fn keyboard_zone_created() -> StudioEdit {
    StudioEdit::ZoneCreated {
        zone_id: ZONE_B.to_owned(),
        name: "Keyboard".to_owned(),
        color: None,
        filled: vec![keys_move(ZONE_A, ZONE_B)],
        layer_moves: vec![ripples_carried()],
    }
}

#[test]
fn undoing_a_zones_creation_reverses_the_receipt_of_its_fill() {
    let StudioEdit::ZoneCreated {
        filled,
        layer_moves,
        ..
    } = keyboard_zone_created()
    else {
        unreachable!("the fixture is a creation");
    };
    let scene = carried_scene(&[stack_layer(RIPPLES, Some(&["keys"]))]);

    let request = members_replay_request(&scene, filled.clone(), &layer_moves, false)
        .expect("the keys are still in the zone made for them");

    assert_eq!(request.scene_id, scene.id);
    assert_eq!(request.assignment, None);
    let back = &request.changes[0];
    assert_eq!(
        back.before.as_ref().map(|state| state.zone_id),
        Some(zone_id(ZONE_B))
    );
    assert_eq!(
        back.after.as_ref().map(|state| state.zone_id),
        Some(zone_id(ZONE_A))
    );
    // The layer goes back to the stack position it left, which the
    // daemon's own rule would not do: it lands a carried layer on top.
    assert_eq!(
        request.layer_moves,
        Some(vec![ripples_carried().reversed()])
    );

    // The same rule as any move: a layer replaced since is left to the
    // daemon rather than named in a request it would refuse.
    let scene = carried_scene(&[stack_layer(GLOW, Some(&["keys"]))]);
    let request = members_replay_request(&scene, filled, &layer_moves, false)
        .expect("the keys are still in the zone made for them");
    assert_eq!(request.layer_moves, None);
}

#[test]
fn redoing_a_zones_creation_carries_the_same_layers_into_the_remade_zone() {
    let mut journal = StudioJournal::default();
    journal.record(keyboard_zone_created());
    journal.complete(false);
    let remade = ZoneId::new();

    // The redo made the zone again under a new id.
    journal.remap_zone(ZONE_B, remade);

    match journal.redo_edit() {
        Some(StudioEdit::ZoneCreated {
            zone_id: made,
            filled,
            layer_moves,
            ..
        }) => {
            assert_eq!(made, &remade.to_string());
            assert_eq!(
                filled[0].after.as_ref().map(|state| state.zone_id),
                Some(remade)
            );
            assert_eq!(layer_moves[0].to.zone_id, remade);
            assert_eq!(layer_moves[0].from.zone_id, zone_id(ZONE_A));
        }
        other => panic!("expected the creation, found {other:?}"),
    }
}

// ── A layer's id through replaces made in another zone ──────────────────

fn scope_step(zone: &str, layer: SceneLayerId) -> StudioEdit {
    StudioEdit::LayerScope {
        zone_id: zone.to_owned(),
        layer_id: layer,
        name: "Ripples".to_owned(),
        before: None,
        after: Some(scoped(&["keys"])),
    }
}

fn carried_by(layer: SceneLayerId) -> StudioEdit {
    StudioEdit::Members {
        changes: vec![keys_move(ZONE_A, ZONE_B)],
        layer_moves: vec![LayerMove {
            layer_id: layer,
            ..ripples_carried()
        }],
    }
}

fn scope_step_layer(edit: Option<&StudioEdit>) -> (String, SceneLayerId) {
    match edit {
        Some(StudioEdit::LayerScope {
            zone_id, layer_id, ..
        }) => (zone_id.clone(), *layer_id),
        other => panic!("expected a change of target, found {other:?}"),
    }
}

fn carried_layer(edit: Option<&StudioEdit>) -> SceneLayerId {
    match edit {
        Some(StudioEdit::Members { layer_moves, .. }) => layer_moves[0].layer_id,
        other => panic!("expected the move, found {other:?}"),
    }
}

#[test]
fn a_change_of_target_follows_its_layer_into_the_zone_it_was_carried_to() {
    let [first, second, third, fourth] = [(); 4].map(|()| SceneLayerId::new());
    let mut journal = StudioJournal::default();

    // Ripples is limited to the keys in Case, then the keys move to
    // Keyboard and take it along.
    let retargeted_in_case = journal.record(scope_step(ZONE_A, first));
    let moved = journal.record(carried_by(first));

    // Its target changes again, in Keyboard: the replace mints a new id
    // there, and the step recorded in Case names the same layer.
    journal.remap_layer(ZONE_B, first, second);
    let retargeted_in_keyboard = journal.record(scope_step(ZONE_B, second));
    assert_eq!(carried_layer(journal.edit(moved)), second);
    assert_eq!(
        scope_step_layer(journal.edit(retargeted_in_case)),
        (ZONE_A.to_owned(), second),
        "the step keeps the zone it was made in and follows the layer's id"
    );

    // Undo the second change of target: another replace in Keyboard.
    journal.remap_layer(ZONE_B, second, third);
    journal.settle(
        retargeted_in_keyboard,
        Settled::Unapplied(scope_step(ZONE_B, third)),
    );
    assert_eq!(scope_step_layer(journal.edit(retargeted_in_case)).1, third);

    // Undo the move: the layer is back in Case under the same id.
    journal.settle(moved, Settled::Unapplied(carried_by(third)));
    assert_eq!(
        scope_step_layer(journal.undo_edit()),
        (ZONE_A.to_owned(), third),
        "the first change of target can still find its layer to undo"
    );

    // Undo the first change of target: a replace in Case. The steps on
    // the redo side name the layer where they will find it once redone.
    journal.remap_layer(ZONE_A, third, fourth);
    journal.settle(
        retargeted_in_case,
        Settled::Unapplied(scope_step(ZONE_A, fourth)),
    );
    assert_eq!(carried_layer(journal.edit(moved)), fourth);
    assert_eq!(
        scope_step_layer(journal.edit(retargeted_in_keyboard)),
        (ZONE_B.to_owned(), fourth)
    );
}

#[test]
fn a_replace_leaves_a_layer_of_the_same_id_in_another_zone_alone() {
    // Layer ids are unique within a zone, not across a scene: Case and
    // Keyboard each hold a layer with this id, and neither ever moved.
    let shared = layer_id(RIPPLES);
    let minted = SceneLayerId::new();
    let mut journal = StudioJournal::default();
    let in_case = journal.record(scope_step(ZONE_A, shared));
    let in_keyboard = journal.record(scope_step(ZONE_B, shared));

    journal.remap_layer(ZONE_B, shared, minted);

    assert_eq!(scope_step_layer(journal.edit(in_case)).1, shared);
    assert_eq!(scope_step_layer(journal.edit(in_keyboard)).1, minted);
}

#[test]
fn a_layer_carried_into_a_zone_made_for_it_keeps_its_earlier_steps() {
    let [first, second] = [(); 2].map(|()| SceneLayerId::new());
    let mut journal = StudioJournal::default();
    let retargeted = journal.record(scope_step(ZONE_A, first));
    let made = journal.record(StudioEdit::ZoneCreated {
        zone_id: ZONE_B.to_owned(),
        name: "Keyboard".to_owned(),
        color: None,
        filled: vec![keys_move(ZONE_A, ZONE_B)],
        layer_moves: vec![LayerMove {
            layer_id: first,
            ..ripples_carried()
        }],
    });

    journal.remap_layer(ZONE_B, first, second);

    assert_eq!(scope_step_layer(journal.edit(retargeted)).1, second);
    match journal.edit(made) {
        Some(StudioEdit::ZoneCreated { layer_moves, .. }) => {
            assert_eq!(layer_moves[0].layer_id, second);
        }
        other => panic!("expected the creation, found {other:?}"),
    }
}

// ── A replay the scene can no longer take ───────────────────────────────

#[test]
fn a_destination_slot_that_is_gone_leaves_the_carrying_to_the_daemon() {
    // The keys went from Case to Keyboard and took Ripples along, from the
    // slot above Case's base layer. Case's base layer was deleted since,
    // so the stack Ripples goes back to is empty and has no slot 1.
    let mut scene = carried_scene(&[stack_layer(RIPPLES, Some(&["keys"]))]);
    scene.zones[0].layers.clear();
    let back = vec![keys_move(ZONE_B, ZONE_A)];

    assert_eq!(
        replay_layer_moves(&scene, &back, &[ripples_carried()], false),
        None,
        "a move into a slot the stack no longer has would be refused"
    );

    // With the base layer still there the slot exists, and the receipt is
    // replayed as it is.
    let scene = carried_scene(&[stack_layer(RIPPLES, Some(&["keys"]))]);
    assert!(replay_layer_moves(&scene, &back, &[ripples_carried()], false).is_some());
}

#[test]
fn a_destination_that_holds_the_layers_id_already_is_left_to_the_daemon() {
    // Ids are unique within a zone only, and Case has since gained a layer
    // with the id Ripples carries.
    let mut scene = carried_scene(&[stack_layer(RIPPLES, Some(&["keys"]))]);
    let twin: SceneLayer = serde_json::from_value(stack_layer(RIPPLES, None)).expect("a layer");
    scene.zones[0].layers.push(twin);
    let back = vec![keys_move(ZONE_B, ZONE_A)];

    assert_eq!(
        replay_layer_moves(&scene, &back, &[ripples_carried()], false),
        None
    );
}

#[test]
fn a_scope_the_destination_cannot_hold_is_left_to_the_daemon() {
    // Glow lit the stand and the keys and narrowed to the stand when the
    // keys left. Its undo widens it to the keys again, which only fits
    // while the same undo brings the keys back to Case.
    let mut scene = carried_scene(&[]);
    let glow: SceneLayer =
        serde_json::from_value(stack_layer(GLOW, Some(&["stand"]))).expect("a layer");
    scene.zones[0].layers.push(glow);
    let narrowed = LayerMove {
        layer_id: layer_id(GLOW),
        from: LayerSlot {
            zone_id: zone_id(ZONE_A),
            index: 1,
        },
        to: LayerSlot {
            zone_id: zone_id(ZONE_A),
            index: 1,
        },
        scope_before: scoped(&["keys", "stand"]),
        scope_after: scoped(&["stand"]),
    };

    let back = vec![keys_move(ZONE_B, ZONE_A)];
    assert!(replay_layer_moves(&scene, &back, std::slice::from_ref(&narrowed), false).is_some());

    // The same receipt beside changes that bring no keys back would name
    // an output Case does not hold.
    assert_eq!(replay_layer_moves(&scene, &[], &[narrowed], false), None);
}

#[test]
fn a_refusal_of_the_request_itself_drops_the_step_and_anything_else_keeps_it() {
    let refused = |status: u16| ApiError::Http {
        status,
        code: None,
        message: Some("no".to_owned()),
        details: None,
    };
    // The daemon read the step and will not apply it to the scene as it
    // stands. Asking again gets the same answer, so the step goes.
    for status in [400, 404, 409, 422] {
        assert!(refuses_step(&refused(status)), "HTTP {status}");
    }
    // A stale fence, a daemon in trouble, a lapsed session, or no daemon
    // at all says nothing about the step: it stays for another try.
    for status in [401, 403, 408, 412, 429, 500, 503] {
        assert!(!refuses_step(&refused(status)), "HTTP {status}");
    }
    assert!(!refuses_step(&ApiError::Network("offline".to_owned())));
    assert!(!refuses_step(&ApiError::Parse("garbled".to_owned())));

    assert_eq!(
        dropped_step_notice(false, "layer move index 1 is out of range (HTTP 422)"),
        "This change can no longer be undone, so it was removed from history: layer move index 1 is out of range (HTTP 422)"
    );
    assert_eq!(
        dropped_step_notice(true, "the zone is gone"),
        "This change can no longer be redone, so it was removed from history: the zone is gone"
    );
}
