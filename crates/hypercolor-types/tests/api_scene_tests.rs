//! Contract pins for the live scene tree types (Spec 78 §1, §2.3, §4).
//!
//! These pin the wire grammar before any route exists: request
//! strictness (unknown fields are loud rejections), the closed
//! transition vocabulary, and the document's decode of a
//! representative daemon-shaped payload.

use hypercolor_types::api::output::{OutputPatchRequest, OutputResource};
use hypercolor_types::api::scene::{
    ApplyEffectRequest, ApplyEffectResponse, AssignMembersRequest, ClearSceneRequest,
    CreateLayerRequest, CreateZoneRequest, PatchControlsRequest, PatchZoneRequest,
    ReorderLayersRequest, ReplaceLayerRequest, SceneDocument, ScenePatchRequest, TransitionType,
    ZoneMember,
};
use serde_json::json;

fn representative_document() -> serde_json::Value {
    json!({
        "id": "0198c5b6-1111-7000-8000-000000000001",
        "name": "Desk Wash",
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
    assert_eq!(document.zones.len(), 1);
    let zone = &document.zones[0];
    assert_eq!(zone.members[0].segment.as_deref(), Some("ch1"));
    assert_eq!(zone.layers.len(), 1, "layers are the addressable unit");
}

#[test]
fn zone_members_use_segment_vocabulary_on_the_wire() {
    let member = ZoneMember {
        id: "out-1".into(),
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
        serde_json::from_value::<OutputPatchRequest>(json!({"brightness": 0.5, "powr": "paused"}))
            .is_err(),
    ];
    assert!(
        rejections.iter().all(|rejected| *rejected),
        "a typo'd request field must be a loud rejection, never a silent no-op"
    );
}

#[test]
fn the_transition_vocabulary_is_closed() {
    assert_eq!(
        serde_json::to_value(TransitionType::Cut).expect("serializes"),
        json!("cut")
    );
    assert!(
        serde_json::from_value::<TransitionType>(json!("fade")).is_err(),
        "the request field does not accept aspirational values (Spec 78 §2.3)"
    );
}

#[test]
fn control_patches_carry_values_and_binding_clears() {
    let patch: PatchControlsRequest = serde_json::from_value(json!({
        "values": {"speed": {"float": 0.5}},
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
        "controls": {"speed": {"float": 1.0}},
        "preset_id": "0198c5b6-1111-7000-8000-000000000005",
        "transition": "cut"
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
        output_woken: false,
    };
    let wire = serde_json::to_value(&response).expect("serializes");
    assert_eq!(
        wire["output_woken"], false,
        "a post-commit wake failure rides inside the 200 (Spec 78 §2.3)"
    );
}

#[test]
fn replace_is_the_creation_shape() {
    let request: ReplaceLayerRequest = serde_json::from_value(json!({
        "source": {"type": "effect", "effect_id": "0198c5b6-1111-7000-8000-000000000004", "controls": {}}
    }))
    .expect("creation shape decodes");
    let same: CreateLayerRequest = request;
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
