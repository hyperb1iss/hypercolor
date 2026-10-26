//! Contract pins for the live scene tree types (Spec 78 §1, §2.3, §4).
//!
//! These pin the wire grammar before any route exists: request
//! strictness (unknown fields are loud rejections), the closed
//! transition vocabulary, and the document's decode of a
//! representative daemon-shaped payload.

use hypercolor_types::api::envelope::RUN_EPOCH_HEADER;
use hypercolor_types::api::output::{OutputPatchRequest, OutputResource};
use hypercolor_types::api::scene::{
    ApplyEffectRequest, ApplyEffectResponse, AssignMembersRequest, ClearSceneRequest,
    CreateLayerRequest, CreateZoneRequest, PatchControlsRequest, PatchZoneRequest,
    ReorderLayersRequest, ReplaceLayerRequest, SceneDocument, ScenePatchRequest, SideEffectOutcome,
    TransitionType, ZoneLayoutRequest, ZoneMember, ZoneMemberId,
};
use hypercolor_types::api::scenes::{ReplaceSceneRequest, SnapshotSceneRequest};
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
fn the_run_epoch_header_is_a_lowercase_wire_name() {
    assert_eq!(RUN_EPOCH_HEADER, "hypercolor-run-epoch");
    assert_eq!(RUN_EPOCH_HEADER, RUN_EPOCH_HEADER.to_ascii_lowercase());
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
    let response = EditMembersResponse {
        document: serde_json::from_value(representative_document()).expect("scene"),
        changes: typed.changes,
    };
    let encoded = serde_json::to_value(&response).expect("serialize receipt");
    assert_eq!(
        serde_json::from_value::<EditMembersResponse>(encoded).expect("read receipt"),
        response
    );
    let mut unknown = request;
    unknown["replace_layers"] = json!(true);
    assert!(serde_json::from_value::<EditMembersRequest>(unknown).is_err());
}
