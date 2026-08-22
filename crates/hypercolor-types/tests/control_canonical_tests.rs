//! Round-trip and rejection coverage for the canonical control-value
//! algebra (Spec 76 §4.5) and the identity conventions (§4.2).

use std::collections::BTreeMap;
use std::str::FromStr;
use std::time::Duration;

use hypercolor_types::control::{
    ControlDeltaBatch, ControlId, ControlSet, ControlValue, ControlValueInvalid,
    EffectJsonValueError, IpText, MacText, SecretRef, SetRevision, narrow_effect_f32,
};
use hypercolor_types::device::DeviceId;
use hypercolor_types::effect::GradientStop;
use hypercolor_types::identity::{BackendId, IdParseError, LayoutId, OutputRef};
use hypercolor_types::spatial::NormalizedRect;

fn canonical_samples() -> Vec<ControlValue> {
    vec![
        ControlValue::Null,
        ControlValue::Bool(true),
        ControlValue::Int(-42),
        ControlValue::Int(i64::MAX),
        ControlValue::Float(0.25),
        ControlValue::Text("hello".into()),
        ControlValue::SecretRef(SecretRef::new("cred_abc123")),
        ControlValue::ColorRgb(hypercolor_color::Rgb::new(255, 136, 0)),
        ControlValue::ColorRgba(hypercolor_color::Rgba::new(1, 2, 3, 128)),
        ControlValue::Ip(IpText::new("192.168.4.20").expect("valid ip")),
        ControlValue::Ip(IpText::new("::FFFF:1.2.3.4").expect("valid ip")),
        ControlValue::Mac(MacText::new("AA:bb:CC:dd:EE:ff").expect("valid mac")),
        ControlValue::Mac(MacText::new("aa-bb-cc-dd-ee-ff").expect("valid mac")),
        ControlValue::Mac(MacText::new("001122334455").expect("valid mac")),
        ControlValue::Mac(MacText::new("aabb.ccdd.eeff").expect("valid mac")),
        ControlValue::Duration(Duration::from_millis(1500)),
        ControlValue::Enum("rainbow".into()),
        ControlValue::Flags(vec!["b".into(), "a".into(), "b".into()]),
        ControlValue::List(vec![ControlValue::Int(1), ControlValue::Text("two".into())]),
        ControlValue::Map(BTreeMap::from([
            ("port".to_owned(), ControlValue::Int(9420)),
            (
                "host".to_owned(),
                ControlValue::Ip(IpText::new("10.0.0.1").expect("valid ip")),
            ),
        ])),
        ControlValue::Unknown,
        ControlValue::ColorLinear(hypercolor_color::LinearRgba::new(0.1, 0.2, 0.3, 1.0)),
        ControlValue::Gradient(vec![
            GradientStop {
                position: 0.0,
                color: [1.0, 0.0, 0.0, 1.0],
            },
            GradientStop {
                position: 1.0,
                color: [0.0, 0.0, 1.0, 0.5],
            },
        ]),
        ControlValue::Rect(NormalizedRect {
            x: 0.1,
            y: 0.2,
            width: 0.5,
            height: 0.4,
        }),
    ]
}

#[test]
fn canonical_wire_roundtrips_every_variant() {
    for original in canonical_samples() {
        let wire = serde_json::to_value(&original).expect("canonical value serializes");
        assert_eq!(
            wire["kind"],
            original.kind_name(),
            "canonical tag drifted for {original:?}"
        );
        let roundtrip: ControlValue =
            serde_json::from_value(wire).expect("canonical value deserializes");
        assert_eq!(roundtrip, original);
    }

    assert_eq!(
        serde_json::to_value(ControlValue::Duration(Duration::from_millis(1500)))
            .expect("duration serializes"),
        serde_json::json!({"kind": "duration", "value": 1500})
    );
}

#[test]
fn canonical_wire_candidate_preserves_arbitrary_provider_maps() {
    for value in [
        serde_json::json!({"kind": "null"}),
        serde_json::json!({"kind": "float", "value": 0.5}),
        serde_json::json!({"kind": "future_value"}),
        serde_json::json!({"kind": 7, "value": true}),
        serde_json::json!({"kind": "text", "value": "ok", "extra": true}),
    ] {
        assert!(ControlValue::is_canonical_wire_candidate(&value));
    }

    for value in [
        serde_json::json!({"kind": "network", "name": "fixture"}),
        serde_json::json!({"value": true}),
        serde_json::json!("text"),
    ] {
        assert!(!ControlValue::is_canonical_wire_candidate(&value));
    }
}

#[test]
fn canonical_wire_rejects_malformed_envelopes() {
    for value in [
        serde_json::json!({"kind": "null", "value": null}),
        serde_json::json!({"kind": "unknown", "value": 1}),
        serde_json::json!({"kind": "text", "value": "ok", "extra": true}),
        serde_json::json!({"kind": "future_value", "value": 1}),
    ] {
        assert!(
            serde_json::from_value::<ControlValue>(value.clone()).is_err(),
            "malformed canonical envelope was accepted: {value}"
        );
    }
}

#[test]
fn effect_json_admission_uses_checked_scalar_narrowing() {
    assert_eq!(narrow_effect_f32(0.25), Ok(0.25));
    assert_eq!(
        narrow_effect_f32(f64::INFINITY),
        Err(EffectJsonValueError::FloatOutOfRange)
    );
    assert_eq!(
        narrow_effect_f32(f64::from(f32::MAX) * 2.0),
        Err(EffectJsonValueError::FloatOutOfRange)
    );
    assert_eq!(
        ControlValue::try_from_effect_json(&serde_json::json!(i64::from(i32::MAX) + 1)),
        Err(EffectJsonValueError::IntegerOutOfRange)
    );
    assert_eq!(
        ControlValue::try_from_effect_color_json(&serde_json::json!([
            0.0,
            0.5,
            f64::from(f32::MAX) * 2.0,
            1.0
        ])),
        Err(EffectJsonValueError::FloatOutOfRange)
    );
}

#[test]
fn effect_json_admission_rejects_malformed_composites() {
    assert!(ControlValue::try_from_effect_json(&serde_json::json!([0.0, 1.0, 0.0])).is_err());
    assert_eq!(
        ControlValue::try_from_effect_json(&serde_json::json!([0.1, 0.2, 0.3, 0.4])),
        Err(EffectJsonValueError::UnsupportedShape)
    );
    assert_eq!(
        ControlValue::try_from_effect_json(&serde_json::json!({
            "x": 0.1,
            "y": 0.2,
            "width": "wide",
            "height": 0.4
        })),
        Err(EffectJsonValueError::UnsupportedShape)
    );
    assert_eq!(
        ControlValue::try_from_effect_json(&serde_json::json!({
            "x": 0.1,
            "y": 0.2,
            "width": 0.5,
            "height": 0.4,
            "future": true
        })),
        Err(EffectJsonValueError::UnsupportedShape)
    );
}

#[test]
fn effect_gradient_projection_round_trips_through_admission() {
    let raw = serde_json::json!([
        {"pos": 0.0, "color": [0.1, 0.2, 0.3, 1.0]},
        {"pos": 0.5, "color": [0.3, 0.2, 0.1, 0.7]},
        {"pos": 1.0, "color": [0.2, 0.3, 0.1, 1.0]}
    ]);

    let canonical = ControlValue::try_from_effect_json(&raw)
        .expect("valid effect gradient should enter the canonical algebra");
    assert!(matches!(&canonical, ControlValue::Gradient(stops) if stops.len() == 3));
    assert_eq!(
        canonical
            .try_to_effect_json()
            .expect("canonical gradient should return to effect JSON"),
        raw
    );
}

#[test]
fn effect_gradient_admission_rejects_malformed_stops_and_colors() {
    for malformed in [
        serde_json::json!([{"pos": 0.0}, {"pos": 1.0, "color": [0.0, 0.0, 0.0, 1.0]}]),
        serde_json::json!([
            {"pos": 0.0, "color": [1.0, 0.0, 0.0]},
            {"pos": 1.0, "color": [0.0, 0.0, 0.0, 1.0]}
        ]),
        serde_json::json!([
            {"pos": "start", "color": [1.0, 0.0, 0.0, 1.0]},
            {"pos": 1.0, "color": [0.0, 0.0, 0.0, 1.0]}
        ]),
        serde_json::json!([
            {"pos": 0.0, "color": [1.0, "red", 0.0, 1.0]},
            {"pos": 1.0, "color": [0.0, 0.0, 0.0, 1.0]}
        ]),
        serde_json::json!([
            {"pos": 0.0, "color": [1.0, 0.0, 0.0, 1.0], "name": "red"},
            {"pos": 1.0, "color": [0.0, 0.0, 0.0, 1.0]}
        ]),
        serde_json::json!(["red", "blue"]),
    ] {
        assert!(
            ControlValue::try_from_effect_json(&malformed).is_err(),
            "malformed gradient was admitted: {malformed}"
        );
    }
}

#[test]
fn canonical_gradient_validation_covers_count_range_order_and_finiteness() {
    let stop = |position, color| GradientStop { position, color };
    for invalid_count in [
        ControlValue::Gradient(Vec::new()),
        ControlValue::Gradient(vec![stop(0.0, [1.0, 0.0, 0.0, 1.0])]),
        ControlValue::Gradient(
            [0.0, 0.125, 0.25, 0.375, 0.5, 0.625, 0.75, 0.875, 1.0]
                .into_iter()
                .map(|position| stop(position, [1.0, 0.0, 0.0, 1.0]))
                .collect(),
        ),
    ] {
        assert!(matches!(
            invalid_count.validate(),
            Err(ControlValueInvalid::GradientStopCount { .. })
        ));
    }

    let cases = [
        (
            ControlValue::Gradient(vec![
                stop(-0.1, [1.0, 0.0, 0.0, 1.0]),
                stop(1.0, [0.0, 0.0, 1.0, 1.0]),
            ]),
            ControlValueInvalid::GradientPositionOutOfRange,
        ),
        (
            ControlValue::Gradient(vec![
                stop(0.0, [1.0, 0.0, 0.0, 1.0]),
                stop(1.1, [0.0, 0.0, 1.0, 1.0]),
            ]),
            ControlValueInvalid::GradientPositionOutOfRange,
        ),
        (
            ControlValue::Gradient(vec![
                stop(0.75, [1.0, 0.0, 0.0, 1.0]),
                stop(0.25, [0.0, 0.0, 1.0, 1.0]),
            ]),
            ControlValueInvalid::GradientPositionsOutOfOrder,
        ),
        (
            ControlValue::Gradient(vec![
                stop(0.0, [1.1, 0.0, 0.0, 1.0]),
                stop(1.0, [0.0, 0.0, 1.0, 1.0]),
            ]),
            ControlValueInvalid::GradientColorOutOfRange,
        ),
        (
            ControlValue::Gradient(vec![
                stop(0.0, [1.0, 0.0, 0.0, 1.0]),
                stop(1.0, [0.0, -0.1, 1.0, 1.0]),
            ]),
            ControlValueInvalid::GradientColorOutOfRange,
        ),
        (
            ControlValue::Gradient(vec![
                stop(f32::NAN, [1.0, 0.0, 0.0, 1.0]),
                stop(1.0, [0.0, 0.0, 1.0, 1.0]),
            ]),
            ControlValueInvalid::NonFiniteFloat,
        ),
        (
            ControlValue::Gradient(vec![
                stop(0.0, [1.0, 0.0, f32::INFINITY, 1.0]),
                stop(1.0, [0.0, 0.0, 1.0, 1.0]),
            ]),
            ControlValueInvalid::NonFiniteFloat,
        ),
    ];
    for (gradient, expected_source) in cases {
        let error = gradient
            .validate()
            .expect_err("invalid gradient should fail canonical validation");
        assert!(
            matches!(&error, ControlValueInvalid::Nested { source, .. } if **source == expected_source),
            "unexpected gradient validation error: {error:?}"
        );
    }

    assert!(
        ControlValue::Gradient(vec![
            stop(0.5, [1.0, 0.0, 0.0, 1.0]),
            stop(0.5, [0.0, 0.0, 1.0, 1.0]),
        ])
        .validate()
        .is_ok(),
        "equal adjacent positions represent a valid hard edge"
    );
}

#[test]
fn effect_gradient_projection_refuses_invalid_canonical_values() {
    let invalid = ControlValue::Gradient(vec![
        GradientStop {
            position: 0.75,
            color: [1.0, 0.0, 0.0, 1.0],
        },
        GradientStop {
            position: 0.25,
            color: [0.0, 0.0, 1.0, 1.0],
        },
    ]);

    let error = invalid
        .try_to_effect_json()
        .expect_err("descending gradient must not project");
    assert!(matches!(
        &error,
        EffectJsonValueError::InvalidGradient { source }
            if matches!(source.as_ref(), ControlValueInvalid::Nested {
                source,
                ..
            } if **source == ControlValueInvalid::GradientPositionsOutOfOrder)
    ));
}

#[test]
fn effect_json_admission_builds_canonical_composites() {
    let raw_color = serde_json::json!([0.1, 0.2, 0.3, 0.4]);
    let color = ControlValue::try_from_effect_color_json(&raw_color)
        .expect("an explicitly addressed color should enter the canonical algebra");
    assert_eq!(color, ControlValue::linear_color([0.1, 0.2, 0.3, 0.4]));
    assert_eq!(
        color
            .try_to_effect_json()
            .expect("linear color should project without quantization"),
        raw_color
    );
    assert_eq!(
        ControlValue::try_from_effect_json(&serde_json::json!({
            "x": 0.1,
            "y": 0.2,
            "width": 0.5,
            "height": 0.4
        })),
        Ok(ControlValue::Rect(NormalizedRect {
            x: 0.1,
            y: 0.2,
            width: 0.5,
            height: 0.4,
        }))
    );
}

#[test]
fn effect_json_scalar_projection_uses_stable_f32_decimals() {
    for raw in [
        serde_json::json!(0.1),
        serde_json::json!(0.2),
        serde_json::json!(0.3),
    ] {
        let canonical = ControlValue::try_from_effect_json(&raw)
            .expect("common effect scalar should enter the canonical algebra");
        assert_eq!(
            canonical
                .try_to_effect_json()
                .expect("common effect scalar should project"),
            raw
        );
    }
}

#[test]
fn effect_json_projection_rejects_driver_only_variants() {
    let unsupported = [
        ControlValue::Null,
        ControlValue::Unknown,
        ControlValue::SecretRef(SecretRef::new("credential")),
        ControlValue::Ip(IpText::new("192.0.2.1").expect("valid IP")),
        ControlValue::Mac(MacText::new("00:11:22:33:44:55").expect("valid MAC")),
        ControlValue::Duration(Duration::from_millis(10)),
        ControlValue::ColorRgb(hypercolor_color::Rgb::new(1, 2, 3)),
        ControlValue::ColorRgba(hypercolor_color::Rgba::new(1, 2, 3, 4)),
        ControlValue::Flags(vec!["one".to_owned()]),
        ControlValue::List(vec![ControlValue::Int(1)]),
        ControlValue::Map(BTreeMap::from([(
            "key".to_owned(),
            ControlValue::Text("value".to_owned()),
        )])),
    ];

    for value in unsupported {
        assert_eq!(
            value.try_to_effect_json(),
            Err(EffectJsonValueError::UnsupportedShape),
            "driver-only value projected to the effect wire: {value:?}"
        );
    }
}

#[test]
fn effect_json_projection_rejects_width_overflow() {
    assert_eq!(
        ControlValue::Int(i64::from(i32::MAX) + 1).try_to_effect_json(),
        Err(EffectJsonValueError::IntegerOutOfRange)
    );
    assert_eq!(
        ControlValue::Float(f64::NAN).try_to_effect_json(),
        Err(EffectJsonValueError::FloatOutOfRange)
    );
}

#[test]
fn canonical_wire_enforces_validation_on_both_directions() {
    assert!(
        serde_json::to_value(ControlValue::Float(f64::NAN)).is_err(),
        "non-finite values must not serialize as null"
    );
    assert!(
        serde_json::from_value::<ControlValue>(
            serde_json::json!({"kind": "ip", "value": "not-an-ip"})
        )
        .is_err()
    );
    assert!(
        serde_json::from_value::<ControlValue>(
            serde_json::json!({"kind": "mac", "value": "not-a-mac"})
        )
        .is_err()
    );
    assert!(
        serde_json::to_value(ControlValue::Duration(Duration::from_micros(1500))).is_err(),
        "sub-millisecond precision must not truncate on the wire"
    );
}

#[test]
fn ip_and_mac_round_trips_preserve_original_spelling() {
    let ip = IpText::new("::FFFF:1.2.3.4").expect("valid ip");
    assert_eq!(ip.as_str(), "::FFFF:1.2.3.4");
    assert!(ip.addr().is_ipv6());

    let mac = MacText::new("AA:bb:CC:dd:EE:ff").expect("valid mac");
    assert_eq!(mac.as_str(), "AA:bb:CC:dd:EE:ff");
    assert_eq!(mac.octets(), [0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF]);

    // Every established encoding is admitted with spelling preserved —
    // persisted driver settings carry all of these.
    for spelling in ["aa-bb-cc-dd-ee-ff", "001122334455", "aabb.ccdd.eeff"] {
        let mac = MacText::new(spelling).expect("established encoding");
        assert_eq!(mac.as_str(), spelling);
    }
    assert_eq!(
        MacText::new("001122334455").expect("bare").octets(),
        [0x00, 0x11, 0x22, 0x33, 0x44, 0x55]
    );
    assert_eq!(
        MacText::new("aabb.ccdd.eeff").expect("dotted").octets(),
        [0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF]
    );
}

#[test]
fn invalid_ip_and_mac_are_rejected_at_canonicalization() {
    assert_eq!(
        IpText::new("not-an-ip"),
        Err(ControlValueInvalid::InvalidIp)
    );
    for bad_mac in [
        "",
        "aa:bb",
        "aa:bb:cc:dd:ee:gg",
        "aa:bb-cc:dd:ee:ff",
        "0011223344",
        "0011.2233.4455.66",
        "aa bb cc dd ee ff",
        "aaaaaaaaaéa",
    ] {
        assert_eq!(
            MacText::new(bad_mac),
            Err(ControlValueInvalid::InvalidMac),
            "{bad_mac:?} must be rejected"
        );
    }
}

#[test]
fn non_finite_floats_are_rejected_everywhere() {
    assert_eq!(
        ControlValue::Float(f64::NAN).validate(),
        Err(ControlValueInvalid::NonFiniteFloat)
    );
    // Nested values validate recursively, and the error names the path.
    let nested = ControlValue::List(vec![
        ControlValue::Bool(true),
        ControlValue::Float(f64::INFINITY),
    ])
    .validate()
    .expect_err("nested non-finite float must fail");
    assert!(
        matches!(
            &nested,
            ControlValueInvalid::Nested { path, source }
                if path == "[1]" && **source == ControlValueInvalid::NonFiniteFloat
        ),
        "expected a located nested error, got {nested:?}"
    );
}

#[test]
fn sub_millisecond_durations_never_truncate_silently() {
    assert_eq!(
        ControlValue::Duration(Duration::from_micros(1500)).validate(),
        Err(ControlValueInvalid::SubMillisecondDuration)
    );
    assert!(
        ControlValue::Duration(Duration::from_millis(1500))
            .validate()
            .is_ok()
    );
}

#[test]
fn control_set_validates_values_and_orders_identifiers() {
    let set = ControlSet::try_from_entries(
        SetRevision::new(7),
        [
            (ControlId::new("speed"), ControlValue::Float(0.5)),
            (ControlId::new("color"), ControlValue::Text("violet".into())),
        ],
    )
    .expect("finite values form a control set");

    assert_eq!(set.set_revision().get(), 7);
    assert_eq!(
        set.iter()
            .map(|(control_id, _)| control_id.as_str())
            .collect::<Vec<_>>(),
        ["color", "speed"]
    );
    assert_eq!(set.get("speed"), Some(&ControlValue::Float(0.5)));
    assert_eq!(set.len(), 2);
    assert!(!set.is_empty());
}

#[test]
fn control_set_rejects_invalid_values_at_admission() {
    let error = ControlSet::try_from_entries(
        SetRevision::new(3),
        [(ControlId::new("speed"), ControlValue::Float(f64::NAN))],
    )
    .expect_err("non-finite control must be refused");

    assert_eq!(error.control_id.as_str(), "speed");
    assert_eq!(error.source, ControlValueInvalid::NonFiniteFloat);
}

#[test]
fn control_delta_batch_carries_revision_and_resolution_order() {
    let changes = [(ControlId::new("speed"), ControlValue::Float(0.75))];
    let batch = ControlDeltaBatch::new(SetRevision::new(9), 4, &changes);

    assert_eq!(batch.set_revision.get(), 9);
    assert_eq!(batch.resolution_seq, 4);
    assert_eq!(batch.changes, changes);
    assert!(!batch.is_empty());
    assert!(ControlDeltaBatch::new(SetRevision::new(9), 5, &[]).is_empty());
}

// ── identity conventions ───────────────────────────────────────────────────

#[test]
fn backend_id_grammar_forbids_colons_and_uppercase() {
    assert!(BackendId::new("usb").is_ok());
    assert!(BackendId::new("govee-lan").is_ok());
    assert!(BackendId::new("open_rgb2").is_ok());
    for bad in ["", "USB", "usb:0", "usb device", "usb/0"] {
        assert!(BackendId::new(bad).is_err(), "{bad:?} must be rejected");
    }
}

#[test]
fn string_ids_reject_garbage_but_admit_persisted_forms() {
    assert!(LayoutId::new("default").is_ok());
    assert!(LayoutId::new("").is_err());
    assert!(LayoutId::new(" padded ").is_err());
    assert!(LayoutId::new("has\ncontrol").is_err());
    // The migration reader's door: legacy forms load without validation
    // and display verbatim.
    let legacy = LayoutId::from_persisted(" grandfathered ");
    assert_eq!(legacy.as_str(), " grandfathered ");
}

#[test]
fn output_ref_wire_form_roundtrips() {
    let device = DeviceId::new();
    let reference = OutputRef {
        backend: BackendId::new("wled").expect("valid backend id"),
        device,
    };
    let wire = reference.to_string();
    let parsed = OutputRef::from_str(&wire).expect("wire form parses");
    assert_eq!(parsed, reference);

    let json = serde_json::to_string(&reference).expect("serializes");
    let from_json: OutputRef = serde_json::from_str(&json).expect("deserializes");
    assert_eq!(from_json, reference);

    assert!(OutputRef::from_str("no-colon").is_err());
    assert!(OutputRef::from_str("usb:not-a-uuid").is_err());
    let _: IdParseError = OutputRef::from_str("BAD:00000000-0000-0000-0000-000000000000")
        .expect_err("uppercase backend segment is rejected");
}

#[test]
fn device_id_keeps_its_canonical_impl_surface() {
    let id = DeviceId::new();
    let text = id.to_string();
    let parsed = DeviceId::from_str(&text).expect("hyphenated uuid parses");
    assert_eq!(parsed, id);
    assert_eq!(format!("{id:?}"), format!("DeviceId({})", id.as_uuid()));
    let as_uuid: &uuid::Uuid = id.as_ref();
    assert_eq!(*as_uuid, id.as_uuid());
    let json = serde_json::to_string(&id).expect("serializes as bare uuid string");
    assert_eq!(json, format!("\"{}\"", id.as_uuid()));
}
