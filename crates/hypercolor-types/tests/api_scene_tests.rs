//! Contract pins for the live scene tree types (Spec 78 §1, §2.3, §4).
//!
//! These pin the wire grammar before any route exists: request
//! strictness (unknown fields are loud rejections), the closed
//! transition vocabulary, and the document's decode of a
//! representative daemon-shaped payload.

use hypercolor_types::api::envelope::{
    IDEMPOTENCY_KEY_HEADER, IDEMPOTENCY_KEY_MISMATCH_CODE, IDEMPOTENCY_REPLAYED_HEADER,
    RUN_EPOCH_HEADER,
};
use hypercolor_types::api::output::{OutputPatchRequest, OutputResource};
use hypercolor_types::api::scene::{
    ApplyEffectRequest, ApplyEffectResponse, AssignMembersRequest, ClearSceneRequest,
    CreateLayerRequest, CreateZoneRequest, PatchControlsRequest, PatchZoneRequest,
    ReorderLayersRequest, ReplaceLayerRequest, SceneDocument, SceneFence, ScenePatchRequest,
    SideEffectOutcome, TransitionType, ZoneLayoutRequest, ZoneMember, ZoneMemberId,
};
use hypercolor_types::api::scenes::{ReplaceSceneRequest, SnapshotSceneRequest};
use hypercolor_types::layer::LayerScope;
use hypercolor_types::scene::{ColorInterpolation, EasingFunction, ScenePriority};
use serde_json::json;

fn representative_document() -> serde_json::Value {
    json!({
        "id": "0198c5b6-1111-7000-8000-000000000001",
        "name": "Desk Wash",
        "kind": "named",
        "is_default": false,
        "unassigned_behavior": "off",
        "layout_id": null,
        "revision": 41,
        "zones": [{
            "id": "0198c5b6-1111-7000-8000-000000000002",
            "name": "Primary",
            "role": "primary",
            "enabled": true,
            "brightness": 1.0,
            "color": null,
            "display_target": null,
            "members": [{
                "id": "out-desk-1",
                "device_id": "usb:controller-1",
                "segment": "ch1",
                "name": "Desk Strip"
            }],
            "layers": [{
                "id": "0198c5b6-1111-7000-8000-000000000003",
                "source": {
                    "type": "effect",
                    "effect_id": "0198c5b6-1111-7000-8000-000000000004",
                    "controls": {}
                },
                "blend": "replace",
                "opacity": 1.0
            }]
        }]
    })
}

#[test]
fn the_document_decodes_a_daemon_shaped_payload() {
    let document: SceneDocument =
        serde_json::from_value(representative_document()).expect("document decodes");
    assert_eq!(document.revision, 41);
    assert_eq!(document.transition.duration_ms, 1000);
    assert_eq!(document.transition.easing, EasingFunction::Linear);
    assert_eq!(
        document.transition.color_interpolation,
        ColorInterpolation::Oklab
    );
    assert_eq!(document.priority, ScenePriority::USER);
    assert!(document.enabled);
    assert_eq!(document.zones.len(), 1);
    let zone = &document.zones[0];
    assert_eq!(zone.members[0].segment.as_deref(), Some("ch1"));
    assert_eq!(zone.layers.len(), 1, "layers are the addressable unit");
    assert_eq!(
        document.run_epoch, "",
        "a daemon that predates run epochs still decodes, with an empty epoch"
    );
}

#[test]
fn the_document_names_the_run_its_revision_belongs_to() {
    let mut payload = representative_document();
    payload["run_epoch"] = json!("0b6f2b57-6e0c-4d7c-9a51-7c1d1f0e2a10");
    let document: SceneDocument = serde_json::from_value(payload).expect("document decodes");
    assert_eq!(document.run_epoch, "0b6f2b57-6e0c-4d7c-9a51-7c1d1f0e2a10");

    let wire = serde_json::to_value(&document).expect("document encodes");
    assert_eq!(wire["run_epoch"], "0b6f2b57-6e0c-4d7c-9a51-7c1d1f0e2a10");
    assert_eq!(wire["revision"], 41);
}

#[test]
fn a_scene_read_is_still_its_own_replacement_body() {
    let mut payload = representative_document();
    payload["run_epoch"] = json!("0b6f2b57-6e0c-4d7c-9a51-7c1d1f0e2a10");
    let document: SceneDocument = serde_json::from_value(payload).expect("document decodes");

    let echoed: ReplaceSceneRequest =
        serde_json::from_value(serde_json::to_value(&document).expect("document encodes"))
            .expect("a read written back as it arrived is a valid replacement");
    assert_eq!(
        echoed.run_epoch.as_deref(),
        Some("0b6f2b57-6e0c-4d7c-9a51-7c1d1f0e2a10")
    );
    assert_eq!(ReplaceSceneRequest::from(&document), echoed);
}

#[test]
fn the_write_contract_headers_are_lowercase_wire_names() {
    assert_eq!(RUN_EPOCH_HEADER, "hypercolor-run-epoch");
    assert_eq!(IDEMPOTENCY_KEY_HEADER, "idempotency-key");
    assert_eq!(IDEMPOTENCY_REPLAYED_HEADER, "idempotency-replayed");
    for name in [
        RUN_EPOCH_HEADER,
        IDEMPOTENCY_KEY_HEADER,
        IDEMPOTENCY_REPLAYED_HEADER,
    ] {
        assert_eq!(name, name.to_ascii_lowercase());
    }
    assert_eq!(IDEMPOTENCY_KEY_MISMATCH_CODE, "idempotency_key_mismatch");
}

#[test]
fn zone_members_use_segment_vocabulary_on_the_wire() {
    let member = ZoneMember {
        id: ZoneMemberId("out-1".into()),
        device_id: "usb:controller-1".into(),
        segment: None,
        name: "Strip".into(),
    };
    let wire = serde_json::to_value(&member).expect("serializes");
    assert!(wire.get("segment").is_some(), "field is named segment");
    assert!(
        wire.get("zone_name").is_none(),
        "device regions are segments, never zones (Spec 78 §5.1)"
    );
}

#[test]
fn every_request_type_rejects_unknown_fields() {
    let rejections = [
        serde_json::from_value::<ScenePatchRequest>(json!({"nmae": "x"})).is_err(),
        serde_json::from_value::<ClearSceneRequest>(json!({"zoen": null})).is_err(),
        serde_json::from_value::<CreateZoneRequest>(json!({"name": "a", "roel": "primary"}))
            .is_err(),
        serde_json::from_value::<PatchZoneRequest>(json!({"brightnes": 0.5})).is_err(),
        serde_json::from_value::<AssignMembersRequest>(json!({
            "device_id": "usb:c1", "segment": ["ch1"]
        }))
        .is_err(),
        serde_json::from_value::<ReorderLayersRequest>(json!({"orders": []})).is_err(),
        serde_json::from_value::<PatchControlsRequest>(json!({"value": {}})).is_err(),
        serde_json::from_value::<ApplyEffectRequest>(json!({"zone_id": null})).is_err(),
        serde_json::from_value::<SnapshotSceneRequest>(json!({
            "name": "Desk", "profile_id": "retired"
        }))
        .is_err(),
        serde_json::from_value::<OutputPatchRequest>(json!({"brightness": 0.5, "powr": "paused"}))
            .is_err(),
    ];
    assert!(
        rejections.iter().all(|rejected| *rejected),
        "a typo'd request field must be a loud rejection, never a silent no-op"
    );
}

#[test]
fn the_transition_vocabulary_is_closed_and_tagged() {
    assert_eq!(
        serde_json::to_value(TransitionType::Cut).expect("serializes"),
        json!({"type": "cut"}),
        "transitions are internally tagged objects (Spec 78 §2.3)"
    );
    assert!(
        serde_json::from_value::<TransitionType>(json!({"type": "fade"})).is_err(),
        "the request field does not accept aspirational values"
    );
    assert!(
        serde_json::from_value::<TransitionType>(json!("cut")).is_err(),
        "the bare-string spelling is not the wire form"
    );
}

#[test]
fn control_patches_carry_values_and_binding_clears() {
    let patch: PatchControlsRequest = serde_json::from_value(json!({
        "values": {"speed": {"kind": "float", "value": 0.5}},
        "clear_bindings": ["speed"]
    }))
    .expect("full form decodes");
    assert_eq!(patch.values.len(), 1);
    assert_eq!(patch.clear_bindings, ["speed"]);

    let minimal: PatchControlsRequest =
        serde_json::from_value(json!({})).expect("both fields default");
    assert!(minimal.values.is_empty() && minimal.clear_bindings.is_empty());

    let wire = serde_json::to_value(&minimal).expect("serializes");
    assert!(
        wire.get("clear_bindings").is_none(),
        "an empty clear list stays off the wire"
    );
}

#[test]
fn apply_takes_the_minimal_and_full_forms() {
    let minimal: ApplyEffectRequest = serde_json::from_value(json!({})).expect("all optional");
    assert!(minimal.zone.is_none() && minimal.transition.is_none());

    let full: ApplyEffectRequest = serde_json::from_value(json!({
        "zone": "0198c5b6-1111-7000-8000-000000000002",
        "controls": {"speed": {"kind": "float", "value": 1.0}},
        "preset_id": "0198c5b6-1111-7000-8000-000000000005",
        "transition": {"type": "cut"}
    }))
    .expect("full form decodes");
    assert_eq!(full.transition, Some(TransitionType::Cut));
}

#[test]
fn apply_response_reports_the_power_outcome_inside_success() {
    let document: SceneDocument =
        serde_json::from_value(representative_document()).expect("document decodes");
    let response = ApplyEffectResponse {
        zone: document.zones[0].clone(),
        transition: TransitionType::Cut,
        output: SideEffectOutcome::failed("output backend unavailable"),
    };
    let wire = serde_json::to_value(&response).expect("serializes");
    assert_eq!(
        wire["output"]["applied"], false,
        "a post-commit wake failure rides inside the 200 (Spec 78 §2.3)"
    );
    assert_eq!(
        wire["output"]["message"], "output backend unavailable",
        "the failure carries its reason"
    );
    let ok = SideEffectOutcome::applied();
    let ok_wire = serde_json::to_value(&ok).expect("serializes");
    assert!(
        ok_wire.get("message").is_none(),
        "a landed side effect carries no message"
    );
}

#[test]
fn replace_is_the_creation_shape() {
    let request: ReplaceLayerRequest = serde_json::from_value(json!({
        "source": {"type": "effect", "effect_id": "0198c5b6-1111-7000-8000-000000000004", "controls": {}}
    }))
    .expect("creation shape decodes");
    let replacement_wire = serde_json::to_value(&request).expect("replacement serializes");
    let same: CreateLayerRequest = request.into();
    let creation_wire = serde_json::to_value(&same).expect("creation serializes");
    assert_eq!(replacement_wire, creation_wire, "request shapes stay equal");
    assert!(
        same.name.is_none(),
        "replacement is creation (Spec 78 §1.4)"
    );
}

#[test]
fn a_created_layer_takes_an_optional_scope() {
    let source = json!({"type": "color_fill", "rgba": [1.0, 0.0, 0.0, 1.0]});
    let whole_zone: CreateLayerRequest =
        serde_json::from_value(json!({"source": source})).expect("no scope decodes");
    assert_eq!(whole_zone.scope, None, "omitted means the whole zone");
    assert!(
        serde_json::to_value(&whole_zone)
            .expect("serializes")
            .get("scope")
            .is_none(),
        "a whole-zone create stays off the wire"
    );

    let scoped: CreateLayerRequest = serde_json::from_value(json!({
        "source": source,
        "scope": {"outputs": ["out-keys", "out-logo"]}
    }))
    .expect("a scope decodes");
    assert_eq!(
        scoped.scope,
        Some(LayerScope {
            outputs: vec!["out-keys".to_owned(), "out-logo".to_owned()],
        })
    );
    assert_eq!(
        serde_json::to_value(&scoped).expect("serializes")["scope"],
        json!({"outputs": ["out-keys", "out-logo"]})
    );
}

#[test]
fn a_layer_replace_scope_is_tri_state() {
    let source = json!({"type": "color_fill", "rgba": [1.0, 0.0, 0.0, 1.0]});
    let keep: ReplaceLayerRequest =
        serde_json::from_value(json!({"source": source})).expect("absent decodes");
    assert_eq!(keep.scope, None, "missing keeps the replaced layer's scope");
    assert!(
        serde_json::to_value(&keep)
            .expect("serializes")
            .get("scope")
            .is_none(),
        "a request that says nothing about scope sends nothing"
    );

    let clear: ReplaceLayerRequest =
        serde_json::from_value(json!({"source": source, "scope": null})).expect("null decodes");
    assert_eq!(
        clear.scope,
        Some(None),
        "explicit null clears to whole zone"
    );
    assert_eq!(
        serde_json::to_value(&clear).expect("serializes")["scope"],
        serde_json::Value::Null,
        "a clear travels as an explicit null"
    );

    let set: ReplaceLayerRequest = serde_json::from_value(json!({
        "source": source,
        "scope": {"outputs": ["out-keys"]}
    }))
    .expect("value decodes");
    let scope = LayerScope {
        outputs: vec!["out-keys".to_owned()],
    };
    assert_eq!(set.scope, Some(Some(scope.clone())));

    assert_eq!(CreateLayerRequest::from(keep).scope, None);
    assert_eq!(CreateLayerRequest::from(clear).scope, None);
    assert_eq!(CreateLayerRequest::from(set).scope, Some(scope));
}

#[test]
fn a_scene_replacement_layer_scope_is_tri_state_and_echoes_a_read() {
    let mut payload = representative_document();
    payload["run_epoch"] = json!("0b6f2b57-6e0c-4d7c-9a51-7c1d1f0e2a10");
    payload["zones"][0]["layers"][0]["scope"] = json!({"outputs": ["out-desk-1"]});
    let document: SceneDocument = serde_json::from_value(payload).expect("document decodes");

    let echoed: ReplaceSceneRequest =
        serde_json::from_value(serde_json::to_value(&document).expect("document encodes"))
            .expect("a scoped read written back is a valid replacement");
    let scope = LayerScope {
        outputs: vec!["out-desk-1".to_owned()],
    };
    assert_eq!(echoed.zones[0].layers[0].scope, Some(Some(scope)));
    assert_eq!(ReplaceSceneRequest::from(&document), echoed);

    let mut wire = serde_json::to_value(&echoed).expect("replacement encodes");
    wire["zones"][0]["layers"][0]["scope"] = serde_json::Value::Null;
    let cleared: ReplaceSceneRequest = serde_json::from_value(wire.clone()).expect("null decodes");
    assert_eq!(cleared.zones[0].layers[0].scope, Some(None));

    wire["zones"][0]["layers"][0]
        .as_object_mut()
        .expect("layer object")
        .remove("scope");
    let kept: ReplaceSceneRequest = serde_json::from_value(wire).expect("absent decodes");
    assert_eq!(
        kept.zones[0].layers[0].scope, None,
        "a client that drops the field keeps the stored scope"
    );
}

#[test]
fn the_output_resource_patches_either_or_both_fields() {
    let resource: OutputResource =
        serde_json::from_value(json!({"power": "running", "brightness": 0.8}))
            .expect("resource decodes");
    assert!((resource.brightness - 0.8).abs() < f32::EPSILON);

    let power_only: OutputPatchRequest =
        serde_json::from_value(json!({"power": "paused"})).expect("power alone");
    assert!(power_only.brightness.is_none());
    let brightness_only: OutputPatchRequest =
        serde_json::from_value(json!({"brightness": 0.25})).expect("brightness alone");
    assert!(brightness_only.power.is_none());
}

#[test]
fn zone_color_patches_are_tri_state() {
    let clear: PatchZoneRequest =
        serde_json::from_value(json!({"color": null})).expect("null decodes");
    assert_eq!(clear.color, Some(None), "explicit null clears");
    let set: PatchZoneRequest =
        serde_json::from_value(json!({"color": "#aa00ff"})).expect("value decodes");
    assert_eq!(set.color, Some(Some("#aa00ff".to_owned())));
    let leave: PatchZoneRequest = serde_json::from_value(json!({})).expect("absent decodes");
    assert_eq!(leave.color, None, "missing leaves the color");
}

#[test]
fn zone_patches_carry_an_optional_move_index() {
    let moved: PatchZoneRequest =
        serde_json::from_value(json!({"index": 2})).expect("index decodes");
    assert_eq!(moved.index, Some(2));
    let leave: PatchZoneRequest = serde_json::from_value(json!({})).expect("absent decodes");
    assert_eq!(leave.index, None, "missing leaves the order");
    let encoded = serde_json::to_value(PatchZoneRequest::default()).expect("encodes");
    assert_eq!(encoded, json!({}), "an unset index stays off the wire");
}

#[test]
fn the_zone_layout_contract_speaks_placements_only() {
    let request: ZoneLayoutRequest = serde_json::from_value(json!({
        "placements": [{
            "member": "out-desk-1",
            "position": {"x": 0.5, "y": 0.5},
            "size": {"x": 1.0, "y": 0.1},
            "topology": {"type": "strip", "count": 60, "direction": "left_to_right"}
        }]
    }))
    .expect("compact layout decodes");
    assert!(
        (request.placements[0].scale - 1.0).abs() < f32::EPSILON,
        "scale defaults to 1.0"
    );

    let wire = serde_json::to_value(&request).expect("serializes");
    let text = wire.to_string();
    assert!(
        !text.contains("zone_name") && !text.contains("device_id"),
        "no device-internal vocabulary on the layout wire (Spec 78 §5.1)"
    );
}

#[test]
fn membership_edit_contract_round_trips_offline_restore_receipts() {
    use hypercolor_types::api::scene::{EditMembersRequest, EditMembersResponse};
    let state = json!({
        "zone_id": "0198c5b6-1111-7000-8000-000000000002",
        "index": 1,
        "output": {
            "id": "out-desk-1", "device_id": "usb:controller-1", "name": "Desk Strip",
            "zone_name": "ch1", "position": {"x": 0.25, "y": 0.5},
            "size": {"x": 0.2, "y": 0.3}, "rotation": 0.0,
            "orientation": null, "topology": {"type": "strip", "count": 4, "direction": "left_to_right"},
            "sampling_mode": null, "edge_behavior": null, "shape": null, "shape_preset": null,
            "led_mapping": [3, 2, 1, 0], "brightness": 0.7
        }
    });
    let request = json!({
        "scene_id": "0198c5b6-1111-7000-8000-000000000001",
        "changes": [{"before": null, "after": state}]
    });
    let typed: EditMembersRequest =
        serde_json::from_value(request.clone()).expect("restore request");
    let after = typed.changes[0].after.as_ref().expect("restored state");
    assert_eq!(after.index, 1);
    assert_eq!(after.output.led_mapping, Some(vec![3, 2, 1, 0]));
    assert_eq!(after.output.brightness, Some(0.7));
    assert_eq!(
        typed.layer_moves, None,
        "a request without layer moves leaves the carry to the daemon"
    );
    assert_eq!(
        serde_json::to_value(&typed).expect("serialize request")["layer_moves"],
        serde_json::Value::Null,
        "an omitted layer_moves stays omitted"
    );
    let response = EditMembersResponse {
        document: serde_json::from_value(representative_document()).expect("scene"),
        changes: typed.changes,
        layer_moves: Vec::new(),
    };
    let encoded = serde_json::to_value(&response).expect("serialize receipt");
    assert_eq!(encoded["layer_moves"], json!([]));
    assert_eq!(
        serde_json::from_value::<EditMembersResponse>(encoded).expect("read receipt"),
        response
    );
    let mut unknown = request;
    unknown["replace_layers"] = json!(true);
    assert!(serde_json::from_value::<EditMembersRequest>(unknown).is_err());
}

#[test]
fn a_membership_receipt_carries_layer_moves_and_replays_them_reversed() {
    use hypercolor_types::api::scene::{
        EditMembersRequest, EditMembersResponse, LayerMove, LayerSlot,
    };
    use hypercolor_types::layer::LayerScope;

    let receipt = json!({
        "document": representative_document(),
        "changes": [],
        "layer_moves": [{
            "layer_id": "0198c5b6-1111-7000-8000-00000000000a",
            "from": {"zone_id": "0198c5b6-1111-7000-8000-000000000002", "index": 1},
            "to": {"zone_id": "0198c5b6-1111-7000-8000-000000000003", "index": 0},
            "scope_before": {"outputs": ["out-desk-1", "out-desk-2"]},
            "scope_after": {"outputs": ["out-desk-1", "out-desk-2"]}
        }]
    });
    let response: EditMembersResponse =
        serde_json::from_value(receipt.clone()).expect("read receipt");
    assert_eq!(
        serde_json::to_value(&response).expect("serialize receipt")["layer_moves"],
        receipt["layer_moves"]
    );
    let carried = &response.layer_moves[0];
    assert_eq!(carried.from.index, 1);
    assert_eq!(
        carried.scope_after,
        LayerScope {
            outputs: vec!["out-desk-1".to_owned(), "out-desk-2".to_owned()],
        }
    );

    let reversed = carried.reversed();
    assert_eq!(
        reversed,
        LayerMove {
            layer_id: carried.layer_id,
            from: LayerSlot {
                zone_id: carried.to.zone_id,
                index: 0,
            },
            to: LayerSlot {
                zone_id: carried.from.zone_id,
                index: 1,
            },
            scope_before: carried.scope_after.clone(),
            scope_after: carried.scope_before.clone(),
        }
    );
    assert_eq!(&reversed.reversed(), carried);

    let replay = json!({
        "scene_id": "0198c5b6-1111-7000-8000-000000000001",
        "changes": [],
        "layer_moves": [serde_json::to_value(&reversed).expect("serialize move")]
    });
    let typed: EditMembersRequest = serde_json::from_value(replay).expect("replay request");
    assert_eq!(typed.layer_moves, Some(vec![reversed]));

    // A receipt written before layer moves existed still reads.
    let mut older = receipt;
    older
        .as_object_mut()
        .expect("receipt is an object")
        .remove("layer_moves");
    assert!(
        serde_json::from_value::<EditMembersResponse>(older)
            .expect("read older receipt")
            .layer_moves
            .is_empty()
    );
}

#[test]
fn a_scene_fence_round_trips_as_an_entity_tag() {
    let fence = SceneFence::new(
        "0b6f2b57-6e0c-4d7c-9a51-7c1d1f0e2a10",
        1_798_012_345_678_901,
    );
    let etag = fence.to_etag();
    assert_eq!(
        etag,
        "\"0b6f2b57-6e0c-4d7c-9a51-7c1d1f0e2a10:1798012345678901\""
    );
    assert_eq!(SceneFence::parse(&etag), Ok(fence.clone()));
    assert_eq!(
        SceneFence::parse("0b6f2b57-6e0c-4d7c-9a51-7c1d1f0e2a10:1798012345678901"),
        Ok(fence),
        "unquoted is the same fence"
    );
}

#[test]
fn a_bare_revision_is_still_a_fence_without_a_run() {
    assert_eq!(
        SceneFence::parse("\"42\""),
        Ok(SceneFence::revision_only(42))
    );
    assert_eq!(SceneFence::parse(" 42 "), Ok(SceneFence::revision_only(42)));
    assert_eq!(SceneFence::parse("0"), Ok(SceneFence::revision_only(0)));
    assert_eq!(SceneFence::revision_only(42).to_etag(), "\"42\"");
    assert_eq!(SceneFence::from(42), SceneFence::revision_only(42));
}

#[test]
fn a_run_epoch_is_the_uuid_the_daemon_serves() {
    let run = "0b6f2b57-6e0c-4d7c-9a51-7c1d1f0e2a10";
    assert_eq!(
        SceneFence::parse(&format!("\"{run}:7\"")),
        Ok(SceneFence::new(run, 7))
    );
    for invalid in [
        format!("\"{run}:1,{run}:2\""),
        format!("{run}:1,{run}:2"),
        format!("\"{}:7\"", run.to_ascii_uppercase()),
        format!("\"{{{run}}}:7\""),
        format!("\"{}:7\"", run.replace('-', "")),
        format!("\"x{run}:7\""),
        "\"run:7\"".to_owned(),
        "\"host:run:7\"".to_owned(),
    ] {
        assert!(SceneFence::parse(&invalid).is_err(), "{invalid:?} parsed");
    }
}

#[test]
fn anything_else_is_not_a_scene_fence() {
    for invalid in [
        "",
        "\"\"",
        "*",
        "W/\"42\"",
        ":42",
        "run:",
        "run:abc",
        "run epoch:42",
        "-1",
        "+42",
        "042",
        "run:042",
        "run:+42",
        "run:18446744073709551616",
        "18446744073709551616",
        "\"run\"x:42\"",
        "\"run:42\", \"run:43\"",
        "\"42",
    ] {
        assert!(SceneFence::parse(invalid).is_err(), "{invalid:?} parsed");
    }
}
