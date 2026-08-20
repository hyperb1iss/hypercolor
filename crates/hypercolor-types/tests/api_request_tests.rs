//! Wire fences for the shared REST request contracts.
//!
//! These pin the two properties clients depend on: what a request type
//! emits when optional fields are unset, and what the daemon accepts
//! coming the other way.

use hypercolor_types::api::assets::AssetUpdateRequest;
use hypercolor_types::api::controls::InvokeControlActionRequest;
use hypercolor_types::api::devices::{
    DiscoverRequest, IdentifyAttachmentRequest, IdentifyRequest, UpdateAttachmentsRequest,
};
use hypercolor_types::api::displays::{DisplayFaceScope, DisplayFaceScopeQuery};
use hypercolor_types::api::library::{
    PlaylistItemRequest, PlaylistTargetRequest, SavePlaylistRequest, SavePresetRequest,
};
use hypercolor_types::api::scenes::CreateSceneRequest;
use hypercolor_types::controls::{ControlValue, ControlValueMap};
use hypercolor_types::pairing::PairDeviceRequest;
use serde_json::json;

/// Assert that a payload carrying explicit `null`s for the named fields
/// decodes to the same value as one that omits them entirely.
///
/// Naming these shapes into shared types moved several clients from
/// emitting `"field": null` to omitting the key. This is the fence that
/// keeps the two spellings interchangeable at the daemon.
///
/// Fields are passed as identifiers, not strings, and the closure below
/// binds each one. No type here declares `deny_unknown_fields`, so a
/// stringly-typed field name that no longer matches the struct would let
/// serde ignore the key and the assertion would hold vacuously; naming
/// the field makes a rename or a typo a compile error instead. The JSON
/// key comes from the same identifier, which is exact for these types
/// because none of their fields carries a `serde(rename)`.
macro_rules! assert_null_and_absent_agree {
    ($ty:ty, $base:expr, $($field:ident),+ $(,)?) => {{
        let _bind_every_field = |value: &$ty| { $( let _ = &value.$field; )+ };

        let absent_payload: serde_json::Value = $base;
        let mut null_payload = absent_payload.clone();
        {
            let object = null_payload
                .as_object_mut()
                .expect("fixture payload must be a JSON object");
            $( object.insert(stringify!($field).to_owned(), serde_json::Value::Null); )+
        }
        assert_ne!(
            absent_payload,
            null_payload,
            concat!(stringify!($ty), ": fixture must not already carry the nulls"),
        );

        let absent: $ty = serde_json::from_value(absent_payload)
            .expect(concat!(stringify!($ty), " must decode with the fields absent"));
        let explicit_null: $ty = serde_json::from_value(null_payload)
            .expect(concat!(stringify!($ty), " must decode with explicit nulls"));

        assert_eq!(
            absent,
            explicit_null,
            concat!(stringify!($ty), ": absent and explicit-null must decode alike"),
        );
    }};
}

#[test]
fn absent_and_explicit_null_optional_fields_decode_alike() {
    assert_null_and_absent_agree!(AssetUpdateRequest, json!({}), name, tags);
    assert_null_and_absent_agree!(
        CreateSceneRequest,
        json!({ "name": "movie-night" }),
        description,
        enabled,
        mutation_mode,
    );
    assert_null_and_absent_agree!(
        SavePresetRequest,
        json!({ "name": "warm", "effect": "aurora" }),
        description,
        controls,
        tags,
    );
    assert_null_and_absent_agree!(
        SavePlaylistRequest,
        json!({ "name": "rotation" }),
        description,
        loop_enabled,
        items,
    );
    assert_null_and_absent_agree!(
        PlaylistItemRequest,
        json!({ "target": { "type": "effect", "effect": "aurora" } }),
        duration_ms,
        transition_ms,
    );
}

#[test]
fn absent_and_empty_pairing_values_decode_alike() {
    let absent: PairDeviceRequest = serde_json::from_value(json!({ "activate_after_pair": true }))
        .expect("pairing request decodes without values");
    let empty: PairDeviceRequest =
        serde_json::from_value(json!({ "values": {}, "activate_after_pair": true }))
            .expect("pairing request decodes with an empty values map");

    assert_eq!(absent, empty);
    assert!(absent.values.is_empty());
}

#[test]
fn unset_optional_request_fields_are_absent_not_null() {
    assert_eq!(
        serde_json::to_value(IdentifyRequest::default()).expect("identify request serializes"),
        json!({})
    );
    assert_eq!(
        serde_json::to_value(DiscoverRequest::default()).expect("discover request serializes"),
        json!({})
    );
}

#[test]
fn identify_attachment_flattens_the_base_request() {
    let request = IdentifyAttachmentRequest {
        base: IdentifyRequest {
            duration_ms: Some(2000),
            color: Some("80FFEA".to_owned()),
        },
        binding_index: Some(1),
        instance: None,
    };

    assert_eq!(
        serde_json::to_value(&request).expect("identify attachment serializes"),
        json!({
            "duration_ms": 2000,
            "color": "80FFEA",
            "binding_index": 1,
        })
    );
}

#[test]
fn component_binding_accepts_absent_and_explicit_null_names() {
    let absent: UpdateAttachmentsRequest = serde_json::from_value(json!({
        "bindings": [{
            "slot_id": "slot-1",
            "template_id": "template-1",
            "enabled": true,
            "instances": 1,
            "led_offset": 0,
        }]
    }))
    .expect("bindings without a name decode");

    let explicit_null: UpdateAttachmentsRequest = serde_json::from_value(json!({
        "bindings": [{
            "slot_id": "slot-1",
            "template_id": "template-1",
            "name": null,
            "enabled": true,
            "instances": 1,
            "led_offset": 0,
        }]
    }))
    .expect("bindings with an explicit null name decode");

    assert_eq!(absent, explicit_null);
    assert_eq!(absent.bindings[0].name, None);
    assert!(!absent.validate_only);
}

#[test]
fn attachment_updates_accept_validate_only() {
    let request: UpdateAttachmentsRequest = serde_json::from_value(json!({
        "bindings": [],
        "validate_only": true,
    }))
    .expect("validate-only attachment update decodes");

    assert!(request.validate_only);
}

#[test]
fn control_values_carry_the_driver_kind_tagging() {
    let cases = [
        (ControlValue::Null, json!({ "kind": "null" })),
        (
            ControlValue::Bool(true),
            json!({ "kind": "bool", "value": true }),
        ),
        (
            ControlValue::Integer(7),
            json!({ "kind": "integer", "value": 7 }),
        ),
        (
            ControlValue::Float(12.5),
            json!({ "kind": "float", "value": 12.5 }),
        ),
        (
            ControlValue::String("aurora".to_owned()),
            json!({ "kind": "string", "value": "aurora" }),
        ),
        (
            ControlValue::SecretRef("token".to_owned()),
            json!({ "kind": "secret_ref", "value": "token" }),
        ),
        (
            ControlValue::IpAddress("10.0.0.1".to_owned()),
            json!({ "kind": "ip_address", "value": "10.0.0.1" }),
        ),
        (
            ControlValue::MacAddress("aa:bb:cc:dd:ee:ff".to_owned()),
            json!({ "kind": "mac_address", "value": "aa:bb:cc:dd:ee:ff" }),
        ),
        (
            ControlValue::DurationMs(250),
            json!({ "kind": "duration_ms", "value": 250 }),
        ),
        (
            ControlValue::Enum("ddp".to_owned()),
            json!({ "kind": "enum", "value": "ddp" }),
        ),
        (
            ControlValue::Flags(vec!["a".to_owned(), "b".to_owned()]),
            json!({ "kind": "flags", "value": ["a", "b"] }),
        ),
        (
            ControlValue::ColorRgb([1, 2, 3]),
            json!({ "kind": "color_rgb", "value": [1, 2, 3] }),
        ),
        (
            ControlValue::ColorRgba([1, 2, 3, 4]),
            json!({ "kind": "color_rgba", "value": [1, 2, 3, 4] }),
        ),
    ];

    for (value, wire) in cases {
        assert_eq!(
            serde_json::to_value(&value).expect("control value serializes"),
            wire,
            "wire form for {value:?}"
        );
    }
}

#[test]
fn control_action_input_defaults_to_an_empty_map() {
    let request: InvokeControlActionRequest =
        serde_json::from_value(json!({})).expect("action body without input decodes");
    assert_eq!(request.input, ControlValueMap::new());

    let mut input = ControlValueMap::new();
    input.insert("force".to_owned(), ControlValue::Bool(true));
    assert_eq!(
        serde_json::to_value(InvokeControlActionRequest { input }).expect("action body serializes"),
        json!({ "input": { "force": { "kind": "bool", "value": true } } })
    );
}

#[test]
fn display_face_scope_wire_spelling_matches_its_query_form() {
    for scope in [DisplayFaceScope::Default, DisplayFaceScope::Scene] {
        assert_eq!(
            serde_json::to_value(scope).expect("scope serializes"),
            json!(scope.as_str())
        );
    }

    let query: DisplayFaceScopeQuery =
        serde_json::from_value(json!({})).expect("empty scope query decodes");
    assert_eq!(query.scope, DisplayFaceScope::Default);
}

#[test]
fn playlist_targets_are_internally_tagged() {
    let request = SavePlaylistRequest {
        name: "evening".to_owned(),
        description: None,
        loop_enabled: Some(true),
        items: Some(vec![
            PlaylistItemRequest {
                target: PlaylistTargetRequest::Effect {
                    effect: "aurora".to_owned(),
                },
                duration_ms: Some(30_000),
                transition_ms: None,
            },
            PlaylistItemRequest {
                target: PlaylistTargetRequest::Preset {
                    preset_id: "preset-1".to_owned(),
                },
                duration_ms: None,
                transition_ms: None,
            },
        ]),
    };

    assert_eq!(
        serde_json::to_value(&request).expect("playlist serializes"),
        json!({
            "name": "evening",
            "loop_enabled": true,
            "items": [
                { "target": { "type": "effect", "effect": "aurora" }, "duration_ms": 30000 },
                { "target": { "type": "preset", "preset_id": "preset-1" } },
            ],
        })
    );
}
