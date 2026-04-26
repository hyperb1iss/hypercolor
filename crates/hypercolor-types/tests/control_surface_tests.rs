//! Tests for typed driver and device control surfaces.

use std::collections::BTreeMap;

use hypercolor_types::controls::{
    ActionConfirmation, ActionConfirmationLevel, AppliedControlChange, ApplyControlChangesRequest,
    ApplyControlChangesResponse, ApplyImpact, CONTROL_SURFACE_SCHEMA_VERSION, ControlAccess,
    ControlActionDescriptor, ControlActionStatus, ControlAvailability, ControlAvailabilityExpr,
    ControlAvailabilityState, ControlChange, ControlEnumOption, ControlFieldDescriptor,
    ControlGroupDescriptor, ControlGroupKind, ControlObjectField, ControlOwner, ControlPersistence,
    ControlSurfaceDocument, ControlSurfaceEvent, ControlSurfaceScope, ControlValue,
    ControlValueType, ControlValueValidationError, ControlVisibility, RejectedControlChange,
};
use hypercolor_types::device::DeviceId;

#[test]
fn control_value_type_roundtrips_with_explicit_kind() {
    let value_type = ControlValueType::Enum {
        options: vec![
            ControlEnumOption::new("ddp", "DDP"),
            ControlEnumOption::new("e131", "E1.31"),
        ],
    };

    let json = serde_json::to_value(&value_type).expect("serialize value type");

    assert_eq!(json["kind"], "enum");
    assert_eq!(json["options"][0]["value"], "ddp");

    let roundtrip: ControlValueType = serde_json::from_value(json).expect("deserialize value type");
    assert_eq!(roundtrip, value_type);
}

#[test]
fn control_value_roundtrips_with_explicit_kind_and_value() {
    let value = ControlValue::DurationMs(1_500);

    let json = serde_json::to_value(&value).expect("serialize value");

    assert_eq!(json["kind"], "duration_ms");
    assert_eq!(json["value"], 1_500);

    let roundtrip: ControlValue = serde_json::from_value(json).expect("deserialize value");
    assert_eq!(roundtrip, value);
}

#[test]
fn validation_accepts_matching_scalar_values() {
    let value_type = ControlValueType::Integer {
        min: Some(0),
        max: Some(100),
        step: Some(5),
    };

    value_type
        .validate_value(&ControlValue::Integer(25))
        .expect("valid integer");
}

#[test]
fn validation_rejects_type_mismatch_and_bounds() {
    let value_type = ControlValueType::DurationMs {
        min: Some(100),
        max: Some(1_000),
        step: Some(100),
    };

    let mismatch = value_type
        .validate_value(&ControlValue::String("fast".to_owned()))
        .expect_err("string is not duration");
    assert!(matches!(
        mismatch,
        ControlValueValidationError::TypeMismatch { .. }
    ));

    let too_low = value_type
        .validate_value(&ControlValue::DurationMs(50))
        .expect_err("duration is below minimum");
    assert_eq!(too_low, ControlValueValidationError::BelowMinimum);

    let invalid_step = value_type
        .validate_value(&ControlValue::DurationMs(150))
        .expect_err("duration misses step");
    assert_eq!(invalid_step, ControlValueValidationError::InvalidStep);
}

#[test]
fn validation_rejects_unknown_enum_and_duplicate_flags() {
    let options = vec![
        ControlEnumOption::new("rgb", "RGB"),
        ControlEnumOption::new("grb", "GRB"),
    ];

    let enum_type = ControlValueType::Enum {
        options: options.clone(),
    };
    let enum_error = enum_type
        .validate_value(&ControlValue::Enum("bgr".to_owned()))
        .expect_err("unknown enum option");
    assert_eq!(
        enum_error,
        ControlValueValidationError::UnknownOption("bgr".to_owned())
    );

    let flags_type = ControlValueType::Flags { options };
    let flags_error = flags_type
        .validate_value(&ControlValue::Flags(vec![
            "rgb".to_owned(),
            "rgb".to_owned(),
        ]))
        .expect_err("duplicate flag");
    assert_eq!(
        flags_error,
        ControlValueValidationError::DuplicateOption("rgb".to_owned())
    );
}

#[test]
fn validation_checks_nested_lists_and_objects() {
    let list_type = ControlValueType::List {
        item_type: Box::new(ControlValueType::IpAddress),
        min_items: Some(1),
        max_items: Some(2),
    };

    list_type
        .validate_value(&ControlValue::List(vec![ControlValue::IpAddress(
            "192.168.1.42".to_owned(),
        )]))
        .expect("valid IP list");

    let list_error = list_type
        .validate_value(&ControlValue::List(vec![ControlValue::IpAddress(
            "not-an-ip".to_owned(),
        )]))
        .expect_err("invalid nested IP");
    assert!(matches!(
        list_error,
        ControlValueValidationError::InvalidItem { index: 0, .. }
    ));

    let object_type = ControlValueType::Object {
        fields: vec![
            ControlObjectField {
                id: "width".to_owned(),
                label: "Width".to_owned(),
                value_type: ControlValueType::Integer {
                    min: Some(1),
                    max: Some(64),
                    step: Some(1),
                },
                required: true,
                default_value: Some(ControlValue::Integer(16)),
            },
            ControlObjectField {
                id: "height".to_owned(),
                label: "Height".to_owned(),
                value_type: ControlValueType::Integer {
                    min: Some(1),
                    max: Some(64),
                    step: Some(1),
                },
                required: true,
                default_value: Some(ControlValue::Integer(16)),
            },
        ],
    };

    let mut dimensions = BTreeMap::new();
    dimensions.insert("width".to_owned(), ControlValue::Integer(16));
    dimensions.insert("height".to_owned(), ControlValue::Integer(8));

    object_type
        .validate_value(&ControlValue::Object(dimensions))
        .expect("valid object");
}

#[test]
fn control_surface_document_roundtrips() {
    let device_id = DeviceId::new();
    let mut document = ControlSurfaceDocument::empty(
        format!("device:{device_id}"),
        ControlSurfaceScope::Device {
            device_id,
            driver_id: "wled".to_owned(),
        },
    );
    document.revision = 7;
    document.groups.push(ControlGroupDescriptor {
        id: "output".to_owned(),
        label: "Output".to_owned(),
        description: None,
        kind: ControlGroupKind::Output,
        ordering: 10,
    });
    document.fields.push(ControlFieldDescriptor {
        id: "color_order".to_owned(),
        owner: ControlOwner::Driver {
            driver_id: "wled".to_owned(),
        },
        group_id: Some("output".to_owned()),
        label: "Color Order".to_owned(),
        description: Some("Hardware channel order".to_owned()),
        value_type: ControlValueType::Enum {
            options: vec![ControlEnumOption::new("rgb", "RGB")],
        },
        default_value: Some(ControlValue::Enum("rgb".to_owned())),
        access: ControlAccess::ReadWrite,
        persistence: ControlPersistence::DeviceConfig,
        apply_impact: ApplyImpact::DeviceReconnect,
        visibility: ControlVisibility::Standard,
        availability: ControlAvailabilityExpr::Always,
        ordering: 10,
    });
    document.actions.push(ControlActionDescriptor {
        id: "identify".to_owned(),
        owner: ControlOwner::Host,
        group_id: Some("output".to_owned()),
        label: "Identify".to_owned(),
        description: None,
        input_fields: Vec::new(),
        result_type: None,
        confirmation: Some(ActionConfirmation {
            level: ActionConfirmationLevel::Normal,
            message: "Blink this device".to_owned(),
        }),
        apply_impact: ApplyImpact::Live,
        availability: ControlAvailabilityExpr::Always,
        ordering: 20,
    });
    document.values.insert(
        "color_order".to_owned(),
        ControlValue::Enum("rgb".to_owned()),
    );
    document.availability.insert(
        "color_order".to_owned(),
        ControlAvailability {
            state: ControlAvailabilityState::Available,
            reason: None,
        },
    );

    let json = serde_json::to_string(&document).expect("serialize document");
    let roundtrip: ControlSurfaceDocument =
        serde_json::from_str(&json).expect("deserialize document");

    assert_eq!(roundtrip.schema_version, CONTROL_SURFACE_SCHEMA_VERSION);
    assert_eq!(roundtrip, document);
}

#[test]
fn apply_request_response_and_events_roundtrip() {
    let request = ApplyControlChangesRequest {
        surface_id: "device:abc".to_owned(),
        expected_revision: Some(3),
        changes: vec![ControlChange {
            field_id: "max_fps".to_owned(),
            value: ControlValue::Integer(60),
        }],
        dry_run: false,
    };
    let request_json = serde_json::to_string(&request).expect("serialize request");
    let request_roundtrip: ApplyControlChangesRequest =
        serde_json::from_str(&request_json).expect("deserialize request");
    assert_eq!(request_roundtrip, request);

    let response = ApplyControlChangesResponse {
        surface_id: "device:abc".to_owned(),
        previous_revision: 3,
        revision: 4,
        accepted: vec![AppliedControlChange {
            field_id: "max_fps".to_owned(),
            value: ControlValue::Integer(60),
        }],
        rejected: vec![RejectedControlChange {
            field_id: "color_order".to_owned(),
            attempted_value: ControlValue::Enum("xyz".to_owned()),
            error: hypercolor_types::controls::ControlApplyError::InvalidValue {
                message: "unknown color order".to_owned(),
            },
        }],
        impacts: vec![ApplyImpact::Live],
        values: BTreeMap::from([("max_fps".to_owned(), ControlValue::Integer(60))]),
    };
    let response_json = serde_json::to_string(&response).expect("serialize response");
    let response_roundtrip: ApplyControlChangesResponse =
        serde_json::from_str(&response_json).expect("deserialize response");
    assert_eq!(response_roundtrip, response);

    let event = ControlSurfaceEvent::ActionProgress {
        surface_id: "device:abc".to_owned(),
        action_id: "identify".to_owned(),
        status: ControlActionStatus::Running,
        progress: Some(0.5),
    };
    let event_json = serde_json::to_value(&event).expect("serialize event");
    assert_eq!(event_json["kind"], "action_progress");
    assert_eq!(event_json["status"], "running");
}
