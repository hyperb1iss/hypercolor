//! A zone's layer stack in Studio's journal: a layer removed and put
//! back where it sat, a stack reordered, and the ids both follow.

use std::collections::HashMap;

use hypercolor_types::api::scene::{LayerMove, LayerSlot, MemberEdit, MemberState, SceneDocument};
use hypercolor_types::control::ControlValue;
use hypercolor_types::effect::EffectId;
use hypercolor_types::layer::{
    BlendMode, LayerAdjust, LayerScope, LayerSource, LayerTransform, SceneLayer, SceneLayerId,
};
use hypercolor_types::scene::{ZoneId, ZoneRole};
use hypercolor_ui::pages::studio::history::{
    AddedLayer, RemovedLayer, Settled, StudioEdit, StudioJournal, recreate_request, reorder_step,
    replay_announcement, replayable_order, restored_order,
};

const CASE: &str = "11111111-1111-4111-8111-111111111111";
const KEYBOARD: &str = "22222222-2222-4222-8222-222222222222";

fn zone_id(id: &str) -> ZoneId {
    id.parse::<uuid::Uuid>().map(ZoneId).expect("zone id")
}

fn ids<const N: usize>() -> [SceneLayerId; N] {
    [(); N].map(|()| SceneLayerId::new())
}

fn scoped(outputs: &[&str]) -> LayerScope {
    LayerScope {
        outputs: outputs.iter().map(|id| (*id).to_owned()).collect(),
    }
}

/// Case holds the stand, Keyboard the keys.
fn scene() -> SceneDocument {
    let member = |id: &str| serde_json::json!({ "id": id, "device_id": format!("usb:{id}"), "segment": "Main", "name": id });
    let placement = |id: &str| {
        serde_json::json!({ "member": id, "position": { "x": 0.5, "y": 0.5 },
        "size": { "x": 0.1, "y": 0.1 }, "topology": { "type": "strip", "count": 15, "direction": "left_to_right" } })
    };
    serde_json::from_value(serde_json::json!({
        "id": "33333333-3333-4333-8333-333333333333", "name": "Desk", "kind": "named",
        "is_default": false, "revision": 9,
        "zones": [
            { "id": CASE, "name": "Case", "role": "primary", "enabled": true, "brightness": 1,
              "members": [member("stand")], "layout": { "placements": [placement("stand")] }, "layers": [] },
            { "id": KEYBOARD, "name": "Keyboard", "role": "custom", "enabled": true, "brightness": 1,
              "members": [member("keys")], "layout": { "placements": [placement("keys")] }, "layers": [] },
        ],
    }))
    .expect("valid scene fixture")
}

/// Ripples: an effect with a control somebody turned, dimmed and paused.
fn ripples(id: SceneLayerId, scope: Option<LayerScope>) -> SceneLayer {
    SceneLayer {
        id,
        name: Some("Ripples".to_owned()),
        source: LayerSource::Effect {
            effect_id: EffectId::new(uuid::Uuid::from_u128(7)),
            controls: HashMap::from([("speed".to_owned(), ControlValue::Float(0.25))]),
            control_bindings: HashMap::new(),
            preset_id: None,
        },
        blend: BlendMode::Screen,
        opacity: 0.4,
        transform: LayerTransform::default(),
        adjust: LayerAdjust::default(),
        bindings: Vec::new(),
        enabled: false,
        scope,
    }
}

fn removed(zone: &str, id: SceneLayerId, index: usize) -> StudioEdit {
    StudioEdit::LayerRemoved(Box::new(RemovedLayer {
        zone_id: zone.to_owned(),
        name: "Ripples".to_owned(),
        index,
        layer: ripples(id, None),
    }))
}

fn reordered(zone: &str, before: &[SceneLayerId], after: &[SceneLayerId]) -> StudioEdit {
    StudioEdit::LayersReordered {
        zone_id: zone.to_owned(),
        name: "Ripples".to_owned(),
        before: before.to_vec(),
        after: after.to_vec(),
    }
}

fn added(zone: &str, id: SceneLayerId) -> StudioEdit {
    StudioEdit::LayersAdded {
        name: "Ripples".to_owned(),
        layers: vec![AddedLayer {
            zone_id: zone.to_owned(),
            layer: ripples(id, None),
            lands_on: vec!["Whole zone".to_owned()],
        }],
    }
}

fn retargeted(zone: &str, id: SceneLayerId) -> StudioEdit {
    StudioEdit::LayerScope {
        zone_id: zone.to_owned(),
        layer_id: id,
        name: "Ripples".to_owned(),
        before: None,
        after: Some(scoped(&["keys"])),
    }
}

/// The keys went from Case to Keyboard and took `layer` along.
fn carried(layer: SceneLayerId) -> StudioEdit {
    let scene = scene();
    let output = hypercolor_ui::api::zone_outputs(&scene.zones[1]).remove(0);
    let state = |zone: &str| MemberState {
        zone_id: zone_id(zone),
        output: output.clone(),
        index: 0,
    };
    let slot = |zone: &str| LayerSlot {
        zone_id: zone_id(zone),
        index: 0,
    };
    StudioEdit::Members {
        changes: vec![MemberEdit {
            before: Some(state(CASE)),
            after: Some(state(KEYBOARD)),
        }],
        layer_moves: vec![LayerMove {
            layer_id: layer,
            from: slot(CASE),
            to: slot(KEYBOARD),
            scope_before: scoped(&["keys"]),
            scope_after: scoped(&["keys"]),
        }],
    }
}

fn removed_layer(edit: Option<&StudioEdit>) -> (String, usize, SceneLayerId) {
    match edit {
        Some(StudioEdit::LayerRemoved(removed)) => {
            (removed.zone_id.clone(), removed.index, removed.layer.id)
        }
        other => panic!("expected a removal, found {other:?}"),
    }
}

// ── A removed layer ─────────────────────────────────────────────────────

#[test]
fn a_removed_layer_is_made_again_as_it_stood() {
    let scene = scene();
    let layer = ripples(SceneLayerId::new(), Some(scoped(&["keys"])));

    let request = recreate_request(&layer, &scene.zones[1]).expect("the keys are still there");

    assert_eq!(request.name.as_deref(), Some("Ripples"));
    assert_eq!(
        request.source, layer.source,
        "its controls come back with it"
    );
    assert_eq!(request.blend, Some(BlendMode::Screen));
    assert_eq!(request.opacity, Some(0.4));
    assert_eq!(request.enabled, Some(false));
    assert_eq!(request.scope, Some(scoped(&["keys"])));

    // A whole-zone layer comes back on the whole zone, whatever it holds.
    let whole = recreate_request(&ripples(SceneLayerId::new(), None), &scene.zones[0])
        .expect("a zone always takes a whole-zone layer");
    assert_eq!(whole.scope, None);

    // Its outputs all left: there is nothing to put it back on, and it
    // must not come back on the whole zone instead.
    assert_eq!(recreate_request(&layer, &scene.zones[0]), None);
}

#[test]
fn a_restored_layer_goes_back_to_the_place_it_held() {
    let [base, glow, top, made] = ids();
    // The daemon makes a layer on top of its stack.
    let stack = [base, glow, top, made];

    assert_eq!(
        restored_order(&stack, made, 1),
        Some(vec![base, made, glow, top])
    );
    assert_eq!(
        restored_order(&stack, made, 0),
        Some(vec![made, base, glow, top])
    );
    // It was the top layer, and is again: nothing to move.
    assert_eq!(restored_order(&stack, made, 3), None);
    // The stack is shorter than it was: the layer stays on top.
    assert_eq!(restored_order(&stack, made, 9), None);
    assert_eq!(restored_order(&[made], made, 4), None);
    // Not the stack the create answered with.
    assert_eq!(restored_order(&[base, glow], made, 0), None);
}

#[test]
fn a_removal_is_one_step_that_undoes_and_redoes() {
    let [old, minted] = ids();
    let mut journal = StudioJournal::default();
    let place = journal.record(removed(CASE, old, 1));

    let (id, edit) = journal.replayed(false).expect("one step to undo");
    assert_eq!(id, place);
    assert_eq!(edit, &removed(CASE, old, 1));

    // The undo made the layer again under a new id; the step waits on the
    // redo side, naming the layer it would take out.
    journal.settle(place, Settled::Unapplied(removed(CASE, minted, 1)));
    assert!(journal.undo_edit().is_none(), "one step, taken back whole");
    assert_eq!(
        removed_layer(journal.redo_edit()),
        (CASE.to_owned(), 1, minted)
    );

    journal.settle(place, Settled::Applied(removed(CASE, minted, 1)));
    assert_eq!(
        removed_layer(journal.undo_edit()),
        (CASE.to_owned(), 1, minted)
    );
    assert!(journal.redo_edit().is_none());
}

#[test]
fn an_undone_removal_takes_every_step_that_names_its_layer_to_the_new_id() {
    let [ripples_id, glow, minted] = ids();
    let mut journal = StudioJournal::default();
    let add = journal.record(added(CASE, ripples_id));
    let target = journal.record(retargeted(CASE, ripples_id));
    let order = journal.record(reordered(CASE, &[ripples_id, glow], &[glow, ripples_id]));
    let removal = journal.record(removed(CASE, ripples_id, 1));

    // Undoing the removal makes the layer again, and the daemon gives it
    // a new id. Every step before it named the old one.
    journal.remap_layer(CASE, ripples_id, minted);
    journal.settle(removal, Settled::Unapplied(removed(CASE, minted, 1)));

    assert_eq!(
        journal.edit(order),
        Some(&reordered(CASE, &[minted, glow], &[glow, minted]))
    );
    assert_eq!(journal.edit(target), Some(&retargeted(CASE, minted)));
    assert_eq!(journal.edit(add), Some(&added(CASE, minted)));
    assert_eq!(
        removed_layer(journal.redo_edit()),
        (CASE.to_owned(), 1, minted)
    );
}

#[test]
fn a_removal_waiting_to_be_redone_follows_its_layer() {
    let [made, remade, other] = ids();
    let mut journal = StudioJournal::default();
    journal.record(added(CASE, made));
    let removal = journal.record(removed(CASE, made, 0));
    journal.complete(false);
    journal.complete(false);

    // Redoing the add makes the layer again under a new id, and the
    // removal waiting behind it has to find that layer.
    journal.remap_layer(CASE, made, remade);
    journal.complete(true);
    assert_eq!(
        removed_layer(journal.redo_edit()),
        (CASE.to_owned(), 0, remade)
    );

    // Once the removal is applied its layer is gone: a layer that takes
    // an id is some other layer, and the record keeps what it removed.
    journal.complete(true);
    journal.remap_layer(CASE, remade, other);
    assert_eq!(
        removed_layer(journal.edit(removal)),
        (CASE.to_owned(), 0, remade)
    );
}

// ── A reordered stack ───────────────────────────────────────────────────

#[test]
fn a_reorder_replays_only_onto_the_layers_it_put_in_order() {
    let [base, glow, top, stranger] = ids();
    let before = [base, glow, top];

    // The same layers, in whatever order they stand now: the recorded
    // order goes back whole.
    assert_eq!(
        replayable_order(&[glow, base, top], &before),
        Some(before.to_vec())
    );
    assert_eq!(
        replayable_order(&[top, glow, base], &before),
        Some(before.to_vec())
    );
    // A layer joined, left, or was traded for another since: no order of
    // the recorded layers describes this stack.
    assert_eq!(
        replayable_order(&[glow, base, top, stranger], &before),
        None
    );
    assert_eq!(replayable_order(&[glow, base], &before), None);
    assert_eq!(replayable_order(&[glow, base, stranger], &before), None);
    assert_eq!(replayable_order(&[], &before), None);
}

#[test]
fn a_reorder_moves_a_layer_one_place_from_where_it_sits_now() {
    let [base, glow, top, face] = ids();

    assert_eq!(
        reorder_step(&[base, glow, top], &[base, glow, top], base, 1),
        Some(vec![glow, base, top])
    );
    // A second press, asked for while the panel still showed the stack
    // as it was before the first: the layer moves on, not back.
    assert_eq!(
        reorder_step(&[glow, base, top], &[base, glow, top], base, 1),
        Some(vec![glow, top, base])
    );
    // A layer outside the group keeps its slot: a screen's face between
    // two canvas layers is not what the canvas layers trade places with.
    assert_eq!(
        reorder_step(&[base, face, top], &[base, top], top, -1),
        Some(vec![top, face, base])
    );
    // Nowhere to go, and nothing to move.
    assert_eq!(reorder_step(&[base, glow], &[base, glow], glow, 1), None);
    assert_eq!(reorder_step(&[base, glow], &[base, glow], base, -1), None);
    assert_eq!(reorder_step(&[base, glow], &[base, glow], top, 1), None);
    assert_eq!(reorder_step(&[base, glow], &[glow], glow, 1), None);
}

#[test]
fn a_reorder_follows_its_layers_through_a_replace() {
    let [ripples_id, glow, minted] = ids();
    let mut journal = StudioJournal::default();
    let order = journal.record(reordered(CASE, &[ripples_id, glow], &[glow, ripples_id]));

    // A layer of the same id in another zone is another layer.
    journal.remap_layer(KEYBOARD, ripples_id, minted);
    assert_eq!(
        journal.edit(order),
        Some(&reordered(CASE, &[ripples_id, glow], &[glow, ripples_id]))
    );

    // A blend change replaced the layer in its own zone.
    journal.remap_layer(CASE, ripples_id, minted);
    assert_eq!(
        journal.edit(order),
        Some(&reordered(CASE, &[minted, glow], &[glow, minted]))
    );
}

#[test]
fn a_reorder_follows_a_layer_replaced_in_the_zone_it_was_carried_to() {
    let [ripples_id, glow, minted] = ids();
    let mut journal = StudioJournal::default();
    let order = journal.record(reordered(CASE, &[ripples_id, glow], &[glow, ripples_id]));
    journal.record(carried(ripples_id));

    // The layer sits in Keyboard now, and was replaced there. Undoing the
    // move brings that layer back to Case, where the reorder must know it.
    journal.remap_layer(KEYBOARD, ripples_id, minted);

    assert_eq!(
        journal.edit(order),
        Some(&reordered(CASE, &[minted, glow], &[glow, minted]))
    );
}

// ── Zones made again, and what the steps are called ─────────────────────

#[test]
fn a_recreated_zone_takes_its_layer_steps_along() {
    let [ripples_id, glow] = ids();
    let mut journal = StudioJournal::default();
    let order = journal.record(reordered(
        KEYBOARD,
        &[ripples_id, glow],
        &[glow, ripples_id],
    ));
    let removal = journal.record(removed(KEYBOARD, ripples_id, 1));
    let elsewhere = journal.record(removed(CASE, glow, 0));
    let remade = ZoneId::new();

    journal.remap_zone(KEYBOARD, remade);

    assert_eq!(
        journal.edit(order),
        Some(&reordered(
            &remade.to_string(),
            &[ripples_id, glow],
            &[glow, ripples_id]
        ))
    );
    assert_eq!(
        removed_layer(journal.edit(removal)),
        (remade.to_string(), 1, ripples_id)
    );
    assert_eq!(
        removed_layer(journal.edit(elsewhere)),
        (CASE.to_owned(), 0, glow)
    );
}

#[test]
fn layer_steps_are_announced_in_plain_words() {
    let zone = |_: &str| Some(("Case".to_owned(), ZoneRole::Primary));
    let device = |_: &str| None;
    let [ripples_id, glow] = ids();
    let say = |edit: &StudioEdit, redo| replay_announcement(edit, redo, &zone, &device);

    let removal = removed(CASE, ripples_id, 0);
    assert_eq!(
        say(&removal, false).as_deref(),
        Some("Undid: removing Ripples")
    );
    assert_eq!(
        say(&removal, true).as_deref(),
        Some("Redid: removing Ripples")
    );

    let order = reordered(CASE, &[ripples_id, glow], &[glow, ripples_id]);
    assert_eq!(
        say(&order, false).as_deref(),
        Some("Undid: moving Ripples in the layer order")
    );
    assert_eq!(
        say(&order, true).as_deref(),
        Some("Redid: moving Ripples in the layer order")
    );

    assert_eq!(
        say(&added(CASE, ripples_id), false).as_deref(),
        Some("Undid: adding Ripples")
    );
}
