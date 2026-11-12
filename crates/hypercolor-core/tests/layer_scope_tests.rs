//! How a layer's output scope behaves in the scene manager: what a
//! request may introduce, what follows an output between zones, and what
//! the stack-wide gestures leave alone.

use std::collections::HashMap;

use hypercolor_core::scene::{
    LayerMutationError, OutputMove, OutputPlacement, SceneManager, plan_layer_carry,
};
use hypercolor_types::api::scene::{LayerMove, LayerSlot};
use hypercolor_types::effect::{
    ControlBinding, EffectCategory, EffectId, EffectMetadata, EffectSource,
};
use hypercolor_types::layer::{LayerScope, LayerSource, SceneLayer, SceneLayerId};
use hypercolor_types::scene::{Scene, SceneId, Zone, ZoneId};
use hypercolor_types::spatial::{
    EdgeBehavior, LedTopology, NormalizedPosition, Output, SamplingMode, SpatialLayout,
    StripDirection,
};
use uuid::Uuid;

const SCENE: SceneId = SceneId::DEFAULT;
const KEYS: &str = "usb:keyboard-keys";
const LOGO: &str = "usb:keyboard-logo";
const STRIP: &str = "usb:hub-channel-1";
const SCREEN: &str = "usb:pump-lcd-display";

fn effect(name: &str) -> EffectMetadata {
    EffectMetadata {
        id: EffectId::new(Uuid::now_v7()),
        name: name.to_owned(),
        author: "test".to_owned(),
        version: "0.1.0".to_owned(),
        description: format!("{name} description"),
        category: EffectCategory::Ambient,
        tags: Vec::new(),
        controls: Vec::new(),
        presets: Vec::new(),
        audio_reactive: false,
        screen_reactive: false,
        input_reactive: false,
        source: EffectSource::Native {
            path: format!("builtin/{name}").into(),
        },
        license: None,
    }
}

fn output(id: &str) -> Output {
    let (device, segment) = match id {
        KEYS => ("usb:keyboard", "Keys"),
        LOGO => ("usb:keyboard", "Logo"),
        SCREEN => ("usb:pump-lcd", "Display"),
        _ => ("usb:hub", "Channel 1"),
    };
    Output {
        id: id.to_owned(),
        name: id.to_owned(),
        device_id: device.to_owned(),
        zone_name: Some(segment.to_owned()),
        position: NormalizedPosition::new(0.5, 0.5),
        size: NormalizedPosition::new(0.2, 0.2),
        rotation: 0.0,
        scale: 1.0,
        display_order: 0,
        orientation: None,
        topology: LedTopology::Strip {
            count: 4,
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

fn layout(outputs: &[&str]) -> SpatialLayout {
    SpatialLayout {
        id: "default".to_owned(),
        name: "Default Layout".to_owned(),
        description: None,
        canvas_width: 320,
        canvas_height: 200,
        zones: outputs.iter().map(|id| output(id)).collect(),
        default_sampling_mode: SamplingMode::Bilinear,
        default_edge_behavior: EdgeBehavior::Clamp,
        version: 1,
    }
}

/// A scope as the manager stores it: sorted and unique.
fn scope(outputs: &[&str]) -> LayerScope {
    LayerScope {
        outputs: outputs.iter().map(|id| (*id).to_owned()).collect(),
    }
    .normalized()
}

fn scoped_layer(outputs: &[&str]) -> SceneLayer {
    let mut layer = SceneLayer::from_effect(
        SceneLayerId::new(),
        EffectId::new(Uuid::now_v7()),
        HashMap::new(),
        HashMap::new(),
        None,
    );
    layer.scope = Some(scope(outputs));
    layer
}

/// The default scene with a primary zone ("Main") holding every output and
/// running one base effect, and an empty custom zone ("Desk").
struct Rig {
    manager: SceneManager,
    main: ZoneId,
    desk: ZoneId,
    base: SceneLayerId,
}

fn rig() -> Rig {
    let mut manager = SceneManager::with_default();
    let main = manager
        .upsert_primary_zone(
            &effect("Bubbles"),
            HashMap::new(),
            None,
            layout(&[KEYS, LOGO, STRIP, SCREEN]),
        )
        .expect("primary zone");
    let (main, base) = (main.id, main.layers[0].id);
    let desk = manager
        .create_zone(&SCENE, "Desk".to_owned(), None, (320, 200))
        .expect("custom zone");
    Rig {
        manager,
        main,
        desk,
        base,
    }
}

impl Rig {
    fn scene(&self) -> &Scene {
        self.manager.get(&SCENE).expect("default scene")
    }

    fn zone(&self, zone_id: ZoneId) -> &Zone {
        self.scene()
            .zones
            .iter()
            .find(|zone| zone.id == zone_id)
            .expect("zone")
    }

    fn stack(&self, zone_id: ZoneId) -> Vec<SceneLayerId> {
        self.zone(zone_id)
            .layers
            .iter()
            .map(|layer| layer.id)
            .collect()
    }

    fn layer(&self, zone_id: ZoneId, layer_id: SceneLayerId) -> &SceneLayer {
        self.zone(zone_id)
            .layers
            .iter()
            .find(|layer| layer.id == layer_id)
            .expect("layer")
    }

    fn members(&self, zone_id: ZoneId) -> Vec<&str> {
        self.zone(zone_id)
            .layout
            .zones
            .iter()
            .map(|output| output.id.as_str())
            .collect()
    }

    /// Add a scoped layer to a zone through the create path.
    fn add(&mut self, zone_id: ZoneId, outputs: &[&str]) -> SceneLayerId {
        let layer = scoped_layer(outputs);
        let layer_id = layer.id;
        self.manager
            .insert_zone_layer(SCENE, zone_id, layer, None, None)
            .expect("scoped layer");
        layer_id
    }

    /// Move outputs between zones in one edit, as the member-edit
    /// transaction does.
    fn move_outputs(&mut self, zone_id: ZoneId, outputs: &[&str]) -> Vec<LayerMove> {
        self.manager
            .assign_outputs_to_zone(
                &SCENE,
                zone_id,
                outputs.iter().map(|id| output(id)).collect(),
                OutputPlacement::Preserve,
            )
            .expect("assign outputs")
    }
}

// ── R1: a request validates the scope ids it introduces ─────────────────

#[test]
fn creating_a_layer_scoped_to_a_non_member_is_refused() {
    let mut rig = rig();
    let layer = scoped_layer(&[KEYS, "usb:not-in-this-zone"]);
    let layer_id = layer.id;
    let before = rig.scene().clone();

    let refusal = rig
        .manager
        .insert_zone_layer(SCENE, rig.main, layer, None, None)
        .map(|_| ())
        .expect_err("a scope may only name outputs of its zone");

    assert_eq!(
        refusal,
        LayerMutationError::ScopeOutputMissing {
            layer_id,
            output: "usb:not-in-this-zone".to_owned(),
        }
    );
    assert_eq!(rig.scene(), &before, "a refused create changes nothing");
}

#[test]
fn creating_a_layer_scoped_to_members_stores_a_normalized_scope() {
    let mut rig = rig();

    let layer_id = rig.add(rig.main, &[LOGO, KEYS, LOGO]);

    assert_eq!(
        rig.layer(rig.main, layer_id).scope,
        Some(scope(&[KEYS, LOGO]))
    );
    assert_eq!(rig.stack(rig.main), vec![rig.base, layer_id]);
}

#[test]
fn a_scope_id_owned_by_another_zone_is_not_a_member() {
    let mut rig = rig();
    rig.move_outputs(rig.desk, &[STRIP]);

    let refusal = rig
        .manager
        .insert_zone_layer(SCENE, rig.main, scoped_layer(&[STRIP]), None, None)
        .map(|_| ())
        .expect_err("the strip belongs to the other zone");

    assert!(matches!(
        refusal,
        LayerMutationError::ScopeOutputMissing { output, .. } if output == STRIP
    ));
}

#[test]
fn replacing_a_layer_refuses_a_newly_named_non_member() {
    let mut rig = rig();
    let layer_id = rig.add(rig.main, &[KEYS]);
    let before = rig.scene().clone();
    let mut replacement = rig.layer(rig.main, layer_id).clone();
    replacement.id = SceneLayerId::new();
    replacement.scope = Some(scope(&[KEYS, "usb:not-in-this-zone"]));

    let refusal = rig
        .manager
        .replace_zone_layer(SCENE, rig.main, layer_id, replacement, None)
        .map(|_| ())
        .expect_err("a replace may only introduce outputs of its zone");

    assert!(matches!(
        refusal,
        LayerMutationError::ScopeOutputMissing { output, .. } if output == "usb:not-in-this-zone"
    ));
    assert_eq!(rig.scene(), &before, "a refused replace changes nothing");
}

#[test]
fn replacing_a_layer_carries_a_dormant_scope_id() {
    let mut rig = rig();
    let layer_id = rig.add(rig.main, &[KEYS, LOGO]);
    rig.manager
        .unassign_output(&SCENE, LOGO)
        .expect("unassign the logo");
    let mut replacement = rig.layer(rig.main, layer_id).clone();
    replacement.id = SceneLayerId::new();
    replacement.opacity = 0.5;
    let replacement_id = replacement.id;

    rig.manager
        .replace_zone_layer(SCENE, rig.main, layer_id, replacement, None)
        .expect("an id the replaced layer already held needs no membership");

    let replaced = rig.layer(rig.main, replacement_id);
    assert_eq!(replaced.scope, Some(scope(&[KEYS, LOGO])));
    assert!((replaced.opacity - 0.5).abs() < f32::EPSILON);
    assert_eq!(
        rig.stack(rig.main),
        vec![rig.base, replacement_id],
        "a replacement keeps the replaced layer's slot"
    );
}

#[test]
fn replacing_a_layer_may_add_a_member_beside_a_dormant_id() {
    let mut rig = rig();
    let layer_id = rig.add(rig.main, &[LOGO]);
    rig.manager
        .unassign_output(&SCENE, LOGO)
        .expect("unassign the logo");
    let mut replacement = rig.layer(rig.main, layer_id).clone();
    replacement.scope = Some(scope(&[STRIP, LOGO]));

    rig.manager
        .replace_zone_layer(SCENE, rig.main, layer_id, replacement, None)
        .expect("the strip is a member and the logo is carried");

    assert_eq!(
        rig.layer(rig.main, layer_id).scope,
        Some(scope(&[LOGO, STRIP]))
    );
}

#[test]
fn replacing_a_layer_can_clear_or_keep_its_scope() {
    let mut rig = rig();
    let layer_id = rig.add(rig.main, &[KEYS]);
    let mut whole_zone = rig.layer(rig.main, layer_id).clone();
    whole_zone.scope = None;

    rig.manager
        .replace_zone_layer(SCENE, rig.main, layer_id, whole_zone, None)
        .expect("clearing a scope introduces nothing");

    assert_eq!(rig.layer(rig.main, layer_id).scope, None);
}

#[test]
fn replacing_an_unknown_layer_is_refused() {
    let mut rig = rig();
    let missing = SceneLayerId::new();

    let refusal = rig
        .manager
        .replace_zone_layer(SCENE, rig.main, missing, scoped_layer(&[KEYS]), None)
        .map(|_| ())
        .expect_err("there is nothing to replace");

    assert_eq!(
        refusal,
        LayerMutationError::LayerMissing { layer_id: missing }
    );
}

// ── R2: stored documents are tolerant ────────────────────────────────────

#[test]
fn a_stored_scene_keeps_a_scope_id_no_zone_holds() {
    let mut rig = rig();
    let mut scene = rig.scene().clone();
    let mut foreign = scoped_layer(&["usb:from-another-rig"]);
    foreign.name = Some("Ripples".to_owned());
    scene.zones[0].layers.push(foreign.clone());

    rig.manager
        .update(scene)
        .expect("a document's scope ids are not checked against membership");

    let stored = rig.layer(rig.main, foreign.id);
    assert_eq!(stored.scope, foreign.scope);
    assert!(rig.zone(rig.main).layer_is_dormant(stored));
}

// ── R4: layers follow their outputs between zones ────────────────────────

#[test]
fn a_two_output_device_moved_in_one_edit_carries_its_layer() {
    let mut rig = rig();
    let keyboard = rig.add(rig.main, &[KEYS, LOGO]);
    let ambient = rig
        .manager
        .apply_effect_to_zone(rig.desk, &effect("Aurora"), HashMap::new(), None)
        .expect("desk base")
        .layers[0]
        .id;

    let moves = rig.move_outputs(rig.desk, &[KEYS, LOGO]);

    assert_eq!(
        moves,
        vec![LayerMove {
            layer_id: keyboard,
            from: LayerSlot {
                zone_id: rig.main,
                index: 1,
            },
            to: LayerSlot {
                zone_id: rig.desk,
                index: 1,
            },
            scope_before: scope(&[KEYS, LOGO]),
            scope_after: scope(&[KEYS, LOGO]),
        }]
    );
    assert_eq!(rig.stack(rig.main), vec![rig.base]);
    assert_eq!(
        rig.stack(rig.desk),
        vec![ambient, keyboard],
        "a carried layer lands on top of the destination stack"
    );
    assert_eq!(
        rig.layer(rig.desk, keyboard).scope,
        Some(scope(&[KEYS, LOGO]))
    );
    assert_eq!(rig.members(rig.desk), vec![KEYS, LOGO]);
}

#[test]
fn a_carried_layer_undoes_exactly_through_its_reversed_moves() {
    let mut rig = rig();
    let keyboard = rig.add(rig.main, &[KEYS, LOGO]);
    let strip = rig.add(rig.main, &[STRIP]);
    let narrowed = rig.add(rig.main, &[KEYS, STRIP]);
    let before = rig.scene().zones.clone();

    let moves = rig.move_outputs(rig.desk, &[KEYS, LOGO]);
    assert_eq!(moves.len(), 2, "one layer moves and one is narrowed");
    assert_eq!(rig.stack(rig.main), vec![rig.base, strip, narrowed]);
    assert_eq!(rig.layer(rig.main, narrowed).scope, Some(scope(&[STRIP])));
    assert_eq!(rig.stack(rig.desk), vec![keyboard]);

    // Undo, the way the member-edit transaction replays a receipt: lift the
    // outputs, put them back at their old slots, then run the receipt's
    // moves reversed. Asking the rule instead would carry the keyboard
    // layer to the top of the stack and leave the narrowed scope narrow.
    let original_layout = before
        .iter()
        .find(|zone| zone.id == rig.main)
        .expect("main")
        .layout
        .clone();
    for moved in [KEYS, LOGO] {
        rig.manager
            .unassign_output(&SCENE, moved)
            .expect("lift the output");
    }
    for moved in [KEYS, LOGO] {
        let carried = rig
            .manager
            .assign_output_to_zone(&SCENE, rig.main, output(moved), OutputPlacement::Preserve)
            .expect("put the output back");
        assert!(
            carried.is_empty(),
            "an output no zone holds carries nothing"
        );
    }
    rig.manager
        .update_zone_layout(&SCENE, rig.main, original_layout)
        .expect("restore the output order");
    let reversed = moves.iter().map(LayerMove::reversed).collect::<Vec<_>>();
    rig.manager
        .apply_layer_moves(SCENE, &reversed)
        .expect("the reversed moves fit the stacks");

    let after = &rig.scene().zones;
    for (restored, original) in after.iter().zip(&before) {
        assert_eq!(restored.layers, original.layers, "{}", original.name);
        assert_eq!(
            restored
                .layout
                .zones
                .iter()
                .map(|output| &output.id)
                .collect::<Vec<_>>(),
            original
                .layout
                .zones
                .iter()
                .map(|output| &output.id)
                .collect::<Vec<_>>()
        );
    }
    assert_eq!(
        rig.stack(rig.main),
        vec![rig.base, keyboard, strip, narrowed]
    );
    assert_eq!(
        rig.layer(rig.main, narrowed).scope,
        Some(scope(&[KEYS, STRIP]))
    );
}

#[test]
fn moving_one_of_two_scoped_outputs_narrows_the_scope_and_keeps_the_layer() {
    let mut rig = rig();
    let keyboard = rig.add(rig.main, &[KEYS, LOGO]);

    let moves = rig.move_outputs(rig.desk, &[LOGO]);

    assert_eq!(
        moves,
        vec![LayerMove {
            layer_id: keyboard,
            from: LayerSlot {
                zone_id: rig.main,
                index: 1,
            },
            to: LayerSlot {
                zone_id: rig.main,
                index: 1,
            },
            scope_before: scope(&[KEYS, LOGO]),
            scope_after: scope(&[KEYS]),
        }]
    );
    assert_eq!(rig.stack(rig.main), vec![rig.base, keyboard]);
    assert!(rig.stack(rig.desk).is_empty());
}

#[test]
fn the_carry_rule_runs_per_edit_not_per_output() {
    let mut split = rig();
    let layer = split.add(split.main, &[KEYS, LOGO]);

    // Two edits, one output each: the first narrows the scope, so only the
    // second output's id is left to carry.
    split.move_outputs(split.desk, &[KEYS]);
    split.move_outputs(split.desk, &[LOGO]);

    assert_eq!(split.layer(split.desk, layer).scope, Some(scope(&[LOGO])));

    // One edit, both outputs: the scope arrives whole.
    let mut together = rig();
    let layer = together.add(together.main, &[KEYS, LOGO]);
    together.move_outputs(together.desk, &[KEYS, LOGO]);

    assert_eq!(
        together.layer(together.desk, layer).scope,
        Some(scope(&[KEYS, LOGO]))
    );
}

#[test]
fn a_single_output_assignment_carries_a_layer_scoped_to_it() {
    let mut rig = rig();
    let strip = rig.add(rig.main, &[STRIP]);

    let moves = rig
        .manager
        .assign_output_to_zone(&SCENE, rig.desk, output(STRIP), OutputPlacement::AutoGrid)
        .expect("assign the strip");

    assert_eq!(moves.len(), 1);
    assert_eq!(rig.stack(rig.desk), vec![strip]);
    assert_eq!(rig.stack(rig.main), vec![rig.base]);
}

#[test]
fn unscoped_and_unrelated_layers_stay_where_they_are() {
    let mut rig = rig();
    let strip = rig.add(rig.main, &[STRIP]);
    let before = rig.zone(rig.main).layers.clone();

    let moves = rig.move_outputs(rig.desk, &[KEYS, LOGO]);

    assert!(moves.is_empty());
    assert_eq!(rig.zone(rig.main).layers, before);
    assert_eq!(rig.stack(rig.main), vec![rig.base, strip]);
}

#[test]
fn a_dormant_id_travels_with_a_carried_layer() {
    let mut rig = rig();
    let layer = rig.add(rig.main, &[KEYS, LOGO]);
    rig.manager
        .unassign_output(&SCENE, LOGO)
        .expect("unassign the logo");

    rig.move_outputs(rig.desk, &[KEYS]);

    assert_eq!(
        rig.layer(rig.desk, layer).scope,
        Some(scope(&[KEYS, LOGO])),
        "every live id moved, so the layer moves with its scope intact"
    );
}

#[test]
fn a_wholly_dormant_layer_is_not_carried_by_an_unrelated_move() {
    let mut rig = rig();
    let layer = rig.add(rig.main, &[LOGO]);
    rig.manager
        .unassign_output(&SCENE, LOGO)
        .expect("unassign the logo");

    let moves = rig.move_outputs(rig.desk, &[KEYS, STRIP]);

    assert!(moves.is_empty());
    assert_eq!(rig.stack(rig.main), vec![rig.base, layer]);
}

#[test]
fn carried_layers_keep_their_order_on_top_of_the_destination() {
    let mut rig = rig();
    let first = rig.add(rig.main, &[KEYS]);
    let between = rig.add(rig.main, &[STRIP]);
    let second = rig.add(rig.main, &[KEYS, LOGO]);
    let resident = rig
        .manager
        .apply_effect_to_zone(rig.desk, &effect("Aurora"), HashMap::new(), None)
        .expect("desk base")
        .layers[0]
        .id;

    let moves = rig.move_outputs(rig.desk, &[KEYS, LOGO]);

    assert_eq!(rig.stack(rig.desk), vec![resident, first, second]);
    assert_eq!(rig.stack(rig.main), vec![rig.base, between]);
    assert_eq!(
        moves
            .iter()
            .map(|moved| (moved.from.index, moved.to.index))
            .collect::<Vec<_>>(),
        vec![(1, 1), (3, 2)]
    );
}

#[test]
fn a_layer_whose_outputs_split_across_zones_stays_and_goes_dormant() {
    let mut rig = rig();
    let layer = rig.add(rig.main, &[KEYS, STRIP]);
    let shelf = rig
        .manager
        .create_zone(&SCENE, "Shelf".to_owned(), None, (320, 200))
        .expect("second custom zone");
    let moves = [
        OutputMove {
            output_id: KEYS.to_owned(),
            from: rig.main,
            to: rig.desk,
        },
        OutputMove {
            output_id: STRIP.to_owned(),
            from: rig.main,
            to: shelf,
        },
    ];

    let plan = plan_layer_carry(rig.scene(), &moves);

    assert!(
        plan.is_empty(),
        "no one zone holds what the layer named, and a scope is never emptied: {plan:?}"
    );
    assert_eq!(
        rig.layer(rig.main, layer).scope,
        Some(scope(&[KEYS, STRIP]))
    );
}

#[test]
fn the_carry_plan_ignores_moves_that_name_no_real_transfer() {
    let rig = rig();
    let phantom = [
        OutputMove {
            output_id: KEYS.to_owned(),
            from: rig.main,
            to: rig.main,
        },
        OutputMove {
            output_id: "usb:not-in-this-zone".to_owned(),
            from: rig.main,
            to: rig.desk,
        },
        OutputMove {
            output_id: LOGO.to_owned(),
            from: rig.main,
            to: ZoneId::new(),
        },
    ];

    assert!(plan_layer_carry(rig.scene(), &phantom).is_empty());
}

#[test]
fn replayed_layer_moves_are_checked_against_the_stacks() {
    let mut rig = rig();
    let keyboard = rig.add(rig.main, &[KEYS, LOGO]);
    let moves = rig.move_outputs(rig.desk, &[KEYS, LOGO]);
    let landed = rig.scene().clone();
    let carried = &moves[0];

    // The layer is now in the destination, so the forward move is stale.
    assert_eq!(
        rig.manager.apply_layer_moves(SCENE, &moves),
        Err(LayerMutationError::LayerMoveStale { layer_id: keyboard })
    );

    let mut wrong_scope = carried.reversed();
    wrong_scope.scope_before = scope(&[KEYS]);
    assert_eq!(
        rig.manager.apply_layer_moves(SCENE, &[wrong_scope]),
        Err(LayerMutationError::LayerMoveStale { layer_id: keyboard })
    );

    // Reversing without moving the outputs back would name outputs the
    // source zone no longer holds, which is fine: nothing is introduced.
    // Widening to an output the destination does not hold is not.
    let mut widened = carried.reversed();
    widened.scope_after = scope(&[KEYS, LOGO, "usb:not-in-this-zone"]);
    assert!(matches!(
        rig.manager.apply_layer_moves(SCENE, &[widened]),
        Err(LayerMutationError::ScopeOutputMissing { output, .. })
            if output == "usb:not-in-this-zone"
    ));

    let mut emptied = carried.reversed();
    emptied.scope_after = scope(&[]);
    assert!(matches!(
        rig.manager.apply_layer_moves(SCENE, &[emptied]),
        Err(LayerMutationError::InvalidLayer { .. })
    ));

    let mut past_the_end = carried.reversed();
    past_the_end.to.index = 7;
    assert!(matches!(
        rig.manager.apply_layer_moves(SCENE, &[past_the_end]),
        Err(LayerMutationError::InvalidIndex { index: 7, .. })
    ));

    let mut unknown_zone = carried.reversed();
    unknown_zone.to.zone_id = ZoneId::new();
    assert_eq!(
        rig.manager.apply_layer_moves(SCENE, &[unknown_zone]),
        Err(LayerMutationError::ZoneMissing)
    );

    assert_eq!(
        rig.manager
            .apply_layer_moves(SCENE, &[carried.reversed(), carried.reversed()]),
        Err(LayerMutationError::DuplicateLayer { layer_id: keyboard })
    );

    assert_eq!(rig.scene(), &landed, "a refused replay changes nothing");
}

#[test]
fn a_replayed_move_cannot_land_on_a_zone_that_already_uses_the_layer_id() {
    let mut rig = rig();
    let keyboard = rig.add(rig.main, &[KEYS]);
    let mut twin = scoped_layer(&[]);
    twin.id = keyboard;
    twin.scope = None;
    let mut scene = rig.scene().clone();
    scene
        .zones
        .iter_mut()
        .find(|zone| zone.id == rig.desk)
        .expect("desk")
        .layers
        .push(twin);
    rig.manager.update(scene).expect("store the twin");

    // The rule leaves such a layer where it is rather than collide.
    assert!(rig.move_outputs(rig.desk, &[KEYS]).is_empty());
    assert_eq!(rig.stack(rig.main), vec![rig.base, keyboard]);

    let forced = LayerMove {
        layer_id: keyboard,
        from: LayerSlot {
            zone_id: rig.main,
            index: 1,
        },
        to: LayerSlot {
            zone_id: rig.desk,
            index: 1,
        },
        scope_before: scope(&[KEYS]),
        scope_after: scope(&[KEYS]),
    };
    assert_eq!(
        rig.manager.apply_layer_moves(SCENE, &[forced]),
        Err(LayerMutationError::DuplicateLayer { layer_id: keyboard })
    );
}

/// A third zone holding the strip and the screen, with a scoped layer
/// that shares its id with one in the primary zone. Scene validation
/// accepts the shared id: layer ids are unique per zone.
fn rig_with_a_twin(main_scope: &[&str], shelf_scope: &[&str]) -> (Rig, ZoneId, SceneLayerId) {
    let mut rig = rig();
    let shared = rig.add(rig.main, main_scope);
    let shelf = rig
        .manager
        .create_zone(&SCENE, "Shelf".to_owned(), None, (320, 200))
        .expect("third zone");
    rig.move_outputs(shelf, &[STRIP, SCREEN]);
    let mut twin = scoped_layer(shelf_scope);
    twin.id = shared;
    twin.name = Some("Twin".to_owned());
    let mut scene = rig.scene().clone();
    scene
        .zones
        .iter_mut()
        .find(|zone| zone.id == shelf)
        .expect("shelf")
        .layers
        .push(twin);
    rig.manager.update(scene).expect("store the twin");
    (rig, shelf, shared)
}

fn scene_layer_count(rig: &Rig) -> usize {
    rig.scene().zones.iter().map(|zone| zone.layers.len()).sum()
}

#[test]
fn two_layers_sharing_an_id_are_never_carried_into_one_zone() {
    let (mut rig, shelf, shared) = rig_with_a_twin(&[KEYS], &[STRIP]);
    let layers_before = scene_layer_count(&rig);

    // One edit takes each twin's only output to the same third zone.
    let moves = rig.move_outputs(rig.desk, &[KEYS, STRIP]);

    assert_eq!(
        moves.len(),
        1,
        "the second twin stays rather than land on the first: {moves:?}"
    );
    assert_eq!(moves[0].from.zone_id, rig.main);
    assert_eq!(scene_layer_count(&rig), layers_before, "no layer is lost");
    assert_eq!(rig.stack(rig.desk), vec![shared]);
    let carried = rig.layer(rig.desk, shared);
    assert_eq!(carried.scope, Some(scope(&[KEYS])));
    assert_eq!(
        carried.name, None,
        "the primary zone's layer is the one carried"
    );
    let stayed = rig.layer(shelf, shared);
    assert_eq!(stayed.name.as_deref(), Some("Twin"));
    assert_eq!(
        stayed.scope,
        Some(scope(&[STRIP])),
        "the twin keeps its scope and goes dormant"
    );
    assert!(rig.zone(shelf).layer_is_dormant(stayed));
}

#[test]
fn two_layers_sharing_an_id_are_narrowed_independently() {
    let (mut rig, shelf, shared) = rig_with_a_twin(&[KEYS, LOGO], &[STRIP, SCREEN]);
    let layers_before = scene_layer_count(&rig);

    // One edit takes one output from each twin, so both stay and narrow.
    let moves = rig.move_outputs(rig.desk, &[KEYS, STRIP]);

    assert_eq!(moves.len(), 2);
    assert_eq!(scene_layer_count(&rig), layers_before, "no layer is lost");
    assert!(rig.stack(rig.desk).is_empty());
    assert_eq!(rig.layer(rig.main, shared).scope, Some(scope(&[LOGO])));
    assert_eq!(rig.layer(rig.main, shared).name, None);
    assert_eq!(rig.layer(shelf, shared).scope, Some(scope(&[SCREEN])));
    assert_eq!(rig.layer(shelf, shared).name.as_deref(), Some("Twin"));

    // The same moves replay in reverse, each finding its own layer.
    for moved in [KEYS, STRIP] {
        rig.manager
            .unassign_output(&SCENE, moved)
            .expect("lift the output");
    }
    rig.manager
        .assign_output_to_zone(&SCENE, rig.main, output(KEYS), OutputPlacement::Preserve)
        .expect("return the keys");
    rig.manager
        .assign_output_to_zone(&SCENE, shelf, output(STRIP), OutputPlacement::Preserve)
        .expect("return the strip");
    let reversed = moves.iter().map(LayerMove::reversed).collect::<Vec<_>>();
    rig.manager
        .apply_layer_moves(SCENE, &reversed)
        .expect("moves that share an id are told apart by their zone");
    assert_eq!(
        rig.layer(rig.main, shared).scope,
        Some(scope(&[KEYS, LOGO]))
    );
    assert_eq!(
        rig.layer(shelf, shared).scope,
        Some(scope(&[SCREEN, STRIP]))
    );
}

#[test]
fn replayed_moves_may_not_land_one_id_twice_in_a_zone() {
    let (mut rig, shelf, shared) = rig_with_a_twin(&[KEYS], &[STRIP]);
    let before = rig.scene().clone();
    let landing = |from: ZoneId, outputs: &[&str], index| LayerMove {
        layer_id: shared,
        from: LayerSlot {
            zone_id: from,
            index: 1,
        },
        to: LayerSlot {
            zone_id: rig.desk,
            index,
        },
        scope_before: scope(outputs),
        scope_after: scope(outputs),
    };
    let both = [
        landing(rig.main, &[KEYS], 0),
        LayerMove {
            from: LayerSlot {
                zone_id: shelf,
                index: 0,
            },
            ..landing(shelf, &[STRIP], 1)
        },
    ];

    assert_eq!(
        rig.manager.apply_layer_moves(SCENE, &both),
        Err(LayerMutationError::DuplicateLayer { layer_id: shared })
    );
    assert_eq!(rig.scene(), &before, "a refused replay changes nothing");
}

#[test]
fn the_stop_gesture_names_the_first_effect_it_removes() {
    use hypercolor_core::scene::first_lighting_effect;

    let mut rig = rig();
    let base_effect = rig.zone(rig.main).effect_ids().next().expect("base effect");
    assert_eq!(first_lighting_effect(rig.zone(rig.main)), Some(base_effect));

    rig.manager.clear_zone_effect(rig.main).expect("clear");
    rig.add(rig.main, &[SCREEN]);
    assert_eq!(
        first_lighting_effect(rig.zone(rig.main)),
        None,
        "a face that survives the clear is not what the gesture stops"
    );

    let keyboard = rig.add(rig.main, &[KEYS]);
    let LayerSource::Effect { effect_id, .. } = rig.layer(rig.main, keyboard).source else {
        panic!("the keyboard layer runs an effect");
    };
    assert_eq!(first_lighting_effect(rig.zone(rig.main)), Some(effect_id));
}

// ── R5: leaving every zone keeps the scope ───────────────────────────────

#[test]
fn unassigning_a_scoped_output_keeps_the_scope_and_readding_revives_it() {
    let mut rig = rig();
    let layer = rig.add(rig.main, &[STRIP]);

    rig.manager
        .unassign_output(&SCENE, STRIP)
        .expect("unassign the strip");

    let dormant = rig.layer(rig.main, layer);
    assert_eq!(
        dormant.scope,
        Some(scope(&[STRIP])),
        "a scope is never widened"
    );
    assert!(rig.zone(rig.main).layer_is_dormant(dormant));

    rig.manager
        .assign_output_to_zone(&SCENE, rig.main, output(STRIP), OutputPlacement::AutoGrid)
        .expect("assign the strip again");

    assert!(
        !rig.zone(rig.main)
            .layer_is_dormant(rig.layer(rig.main, layer))
    );
}

#[test]
fn a_followed_layout_that_drops_an_output_keeps_the_scope() {
    let mut manager = SceneManager::with_default();
    let main = manager
        .upsert_primary_zone(
            &effect("Bubbles"),
            HashMap::new(),
            None,
            layout(&[KEYS, STRIP]),
        )
        .expect("primary zone")
        .id;
    let layer = scoped_layer(&[STRIP]);
    let layer_id = layer.id;
    manager
        .insert_zone_layer(SCENE, main, layer, None, None)
        .expect("scoped layer");

    assert!(manager.sync_primary_zone_layout(&layout(&[KEYS])));

    let zone = &manager.get(&SCENE).expect("default scene").zones[0];
    let kept = zone
        .layers
        .iter()
        .find(|layer| layer.id == layer_id)
        .expect("the layer outlives its output");
    assert_eq!(kept.scope, Some(scope(&[STRIP])));
    assert!(zone.layer_is_dormant(kept));
}

// ── R6: a deleted zone takes its layers ──────────────────────────────────

#[test]
fn deleting_a_zone_removes_its_scoped_layers_and_frees_its_outputs() {
    let mut rig = rig();
    let layer = rig.add(rig.main, &[KEYS, LOGO]);
    rig.move_outputs(rig.desk, &[KEYS, LOGO]);
    assert_eq!(rig.stack(rig.desk), vec![layer]);

    rig.manager
        .delete_zone(&SCENE, rig.desk)
        .expect("delete the custom zone");

    let scene = rig.scene();
    assert!(scene.zones.iter().all(|zone| zone.id != rig.desk));
    assert!(
        scene
            .zones
            .iter()
            .flat_map(|zone| &zone.layers)
            .all(|kept| kept.id != layer),
        "the layer goes with its zone"
    );
    assert!(
        scene
            .zones
            .iter()
            .flat_map(|zone| &zone.layout.zones)
            .all(|output| output.id != KEYS && output.id != LOGO),
        "the zone's outputs become unassigned"
    );
}

// ── Apply keeps scoped layers ────────────────────────────────────────────

#[test]
fn applying_an_effect_replaces_whole_zone_layers_and_keeps_scoped_ones() {
    let mut rig = rig();
    let keyboard = rig.add(rig.main, &[KEYS, LOGO]);
    let mut overlay = SceneLayer::from_effect(
        SceneLayerId::new(),
        EffectId::new(Uuid::now_v7()),
        HashMap::new(),
        HashMap::new(),
        None,
    );
    overlay.name = Some("Whole-zone overlay".to_owned());
    let overlay_id = overlay.id;
    rig.manager
        .insert_zone_layer(SCENE, rig.main, overlay, None, None)
        .expect("unscoped overlay");
    let face = rig.add(rig.main, &[SCREEN]);
    let aurora = effect("Aurora");

    let zone = rig
        .manager
        .upsert_primary_zone(
            &aurora,
            HashMap::new(),
            None,
            layout(&[KEYS, LOGO, STRIP, SCREEN]),
        )
        .expect("apply")
        .clone();

    assert_eq!(zone.layers.len(), 3);
    assert!(matches!(
        zone.layers[0].source,
        LayerSource::Effect { effect_id, .. } if effect_id == aurora.id
    ));
    assert_eq!(zone.layers[0].scope, None);
    assert_ne!(
        zone.layers[0].id, rig.base,
        "apply mints a fresh base layer"
    );
    assert_eq!(
        zone.layers[1..]
            .iter()
            .map(|layer| layer.id)
            .collect::<Vec<_>>(),
        vec![keyboard, face],
        "scoped layers stay above the new base in their order"
    );
    assert!(zone.layers.iter().all(|layer| layer.id != overlay_id));
}

#[test]
fn applying_to_a_zone_with_no_scoped_layer_leaves_one_layer() {
    let mut rig = rig();
    let aurora = effect("Aurora");

    let zone = rig
        .manager
        .apply_effect_to_zone(rig.main, &aurora, HashMap::new(), None)
        .expect("apply")
        .clone();

    assert_eq!(zone.layers.len(), 1);
    assert_eq!(zone.effect_ids().collect::<Vec<_>>(), vec![aurora.id]);
}

#[test]
fn applying_after_a_scoped_output_left_succeeds_and_keeps_the_dormant_layer() {
    let mut rig = rig();
    let layer = rig.add(rig.main, &[STRIP]);
    rig.manager
        .unassign_output(&SCENE, STRIP)
        .expect("unassign the strip");

    let zone = rig
        .manager
        .apply_effect_to_zone(rig.main, &effect("Aurora"), HashMap::new(), None)
        .expect("apply does not re-check a kept layer's scope")
        .clone();

    assert_eq!(zone.layers.len(), 2);
    assert_eq!(zone.layers[1].id, layer);
    assert_eq!(zone.layers[1].scope, Some(scope(&[STRIP])));
    assert!(zone.layer_is_dormant(&zone.layers[1]));
}

#[test]
fn reapplying_an_effect_carries_the_base_layer_bindings_not_a_scoped_layers() {
    let mut rig = rig();
    let bubbles = effect("Bubbles");
    rig.manager
        .apply_effect_to_zone(rig.main, &bubbles, HashMap::new(), None)
        .expect("base");
    let binding = ControlBinding {
        sensor: "cpu".to_owned(),
        sensor_min: 0.0,
        sensor_max: 100.0,
        target_min: 0.0,
        target_max: 1.0,
        deadband: 0.0,
        smoothing: 0.0,
    };
    let mut base = rig.zone(rig.main).layers[0].clone();
    if let LayerSource::Effect {
        control_bindings, ..
    } = &mut base.source
    {
        control_bindings.insert("speed".to_owned(), binding.clone());
    }
    let base_id = base.id;
    rig.manager
        .replace_zone_layer(SCENE, rig.main, base_id, base, None)
        .expect("bind the base");
    // A scoped layer running the same effect, with a binding of its own.
    let mut scoped = SceneLayer::from_effect(
        SceneLayerId::new(),
        bubbles.id,
        HashMap::new(),
        HashMap::from([("density".to_owned(), binding.clone())]),
        None,
    );
    scoped.scope = Some(scope(&[KEYS]));
    rig.manager
        .insert_zone_layer(SCENE, rig.main, scoped, Some(0), None)
        .expect("scoped layer beneath the base");

    let zone = rig
        .manager
        .apply_effect_to_zone(rig.main, &bubbles, HashMap::new(), None)
        .expect("re-apply")
        .clone();

    let LayerSource::Effect {
        control_bindings, ..
    } = &zone.layers[0].source
    else {
        panic!("the base is an effect layer");
    };
    assert_eq!(
        control_bindings.keys().collect::<Vec<_>>(),
        vec!["speed"],
        "the re-applied base keeps its own bindings"
    );
}

#[test]
fn replacing_the_base_layer_keeps_scoped_layers() {
    let mut rig = rig();
    let keyboard = rig.add(rig.main, &[KEYS]);
    let color = SceneLayer {
        source: LayerSource::ColorFill {
            rgba: [1.0, 0.0, 0.5, 1.0],
        },
        ..SceneLayer::from_effect(
            SceneLayerId::new(),
            EffectId::new(Uuid::now_v7()),
            HashMap::new(),
            HashMap::new(),
            None,
        )
    };
    let color_id = color.id;

    rig.manager
        .replace_zone_base_layer(SCENE, rig.main, color)
        .expect("a solid color replaces the base");

    assert_eq!(rig.stack(rig.main), vec![color_id, keyboard]);
}

// ── Clear removes lighting and keeps faces ───────────────────────────────

#[test]
fn clearing_a_zone_removes_scoped_lighting_and_keeps_screen_only_layers() {
    let mut rig = rig();
    let keyboard = rig.add(rig.main, &[KEYS, LOGO]);
    let face = rig.add(rig.main, &[SCREEN]);
    let mixed = rig.add(rig.main, &[SCREEN, STRIP]);

    let zone = rig
        .manager
        .clear_zone_effect(rig.main)
        .expect("clear")
        .clone();

    assert_eq!(
        zone.layers.iter().map(|layer| layer.id).collect::<Vec<_>>(),
        vec![face],
        "the base, the keyboard layer, and a layer that also lights LEDs all go"
    );
    assert!(zone.layers.iter().all(|layer| layer.id != keyboard));
    assert!(zone.layers.iter().all(|layer| layer.id != mixed));
}

#[test]
fn clearing_removes_a_screen_layer_that_also_names_a_dormant_output() {
    let mut rig = rig();
    rig.add(rig.main, &[SCREEN, STRIP]);
    rig.manager
        .unassign_output(&SCENE, STRIP)
        .expect("unassign the strip");

    rig.manager.clear_zone_effect(rig.main).expect("clear");

    assert!(
        rig.stack(rig.main).is_empty(),
        "a scope that names a strip is lighting, and re-adding the strip must not light it"
    );
    rig.manager
        .assign_output_to_zone(&SCENE, rig.main, output(STRIP), OutputPlacement::AutoGrid)
        .expect("assign the strip again");
    assert!(rig.stack(rig.main).is_empty());
}

#[test]
fn clearing_removes_a_face_whose_screen_has_left_the_zone() {
    let mut rig = rig();
    rig.add(rig.main, &[SCREEN]);
    rig.manager
        .unassign_output(&SCENE, SCREEN)
        .expect("unassign the screen");

    rig.manager.clear_zone_effect(rig.main).expect("clear");

    assert!(
        rig.stack(rig.main).is_empty(),
        "an id that names no current output cannot be shown to be a screen; \
         losing such a face to a clear is the accepted cost of one rule"
    );
}

#[test]
fn clearing_keeps_a_layer_only_when_every_scope_id_is_a_live_screen() {
    let mut rig = rig();
    let mut second_screen = output(SCREEN);
    second_screen.id = "usb:fan-lcd-display".to_owned();
    second_screen.device_id = "usb:fan-lcd".to_owned();
    rig.manager
        .assign_output_to_zone(&SCENE, rig.main, second_screen, OutputPlacement::AutoGrid)
        .expect("a second screen");
    let both = rig.add(rig.main, &[SCREEN, "usb:fan-lcd-display"]);
    let one = rig.add(rig.main, &[SCREEN]);
    rig.add(rig.main, &[SCREEN, KEYS]);

    rig.manager.clear_zone_effect(rig.main).expect("clear");

    assert_eq!(rig.stack(rig.main), vec![both, one]);
}

#[test]
fn clearing_a_zone_with_nothing_to_remove_leaves_its_layer_version_alone() {
    let mut rig = rig();
    rig.manager.clear_zone_effect(rig.main).expect("clear");
    let face = rig.add(rig.main, &[SCREEN]);
    let version = rig.zone(rig.main).layers_version;

    rig.manager
        .clear_zone_effect(rig.main)
        .expect("clear again");

    assert_eq!(rig.stack(rig.main), vec![face]);
    assert_eq!(rig.zone(rig.main).layers_version, version);
}
