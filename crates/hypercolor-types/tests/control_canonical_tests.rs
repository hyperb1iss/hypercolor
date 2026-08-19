//! Round-trip and rejection coverage for the canonical control-value
//! algebra (Spec 76 §4.5) and the identity conventions (§4.2).
//!
//! The load-bearing property: every driver-wire and effect-wire value
//! round-trips through canonical BYTE-IDENTICALLY on its own wire —
//! serialize the original, serialize the reprojection, compare JSON.

use std::collections::BTreeMap;
use std::str::FromStr;
use std::time::Duration;

use hypercolor_types::control::{
    ControlValue, ControlValueInvalid, DriverProjectionError, EffectProjectionError, IpText,
    MacText, SecretRef,
};
use hypercolor_types::controls as driver;
use hypercolor_types::device::DeviceId;
use hypercolor_types::effect::{self, GradientStop};
use hypercolor_types::identity::{BackendId, IdParseError, LayoutId, OutputRef};
use hypercolor_types::viewport::ViewportRect;

fn driver_samples() -> Vec<driver::ControlValue> {
    vec![
        driver::ControlValue::Null,
        driver::ControlValue::Bool(true),
        driver::ControlValue::Integer(-42),
        driver::ControlValue::Integer(i64::MAX),
        driver::ControlValue::Float(0.25),
        driver::ControlValue::String("hello".into()),
        driver::ControlValue::SecretRef("cred_abc123".into()),
        driver::ControlValue::ColorRgb([255, 136, 0]),
        driver::ControlValue::ColorRgba([1, 2, 3, 128]),
        driver::ControlValue::IpAddress("192.168.4.20".into()),
        driver::ControlValue::IpAddress("::FFFF:1.2.3.4".into()),
        driver::ControlValue::MacAddress("AA:bb:CC:dd:EE:ff".into()),
        driver::ControlValue::MacAddress("aa-bb-cc-dd-ee-ff".into()),
        driver::ControlValue::MacAddress("001122334455".into()),
        driver::ControlValue::MacAddress("aabb.ccdd.eeff".into()),
        driver::ControlValue::DurationMs(1500),
        driver::ControlValue::Enum("rainbow".into()),
        driver::ControlValue::Flags(vec!["b".into(), "a".into(), "b".into()]),
        driver::ControlValue::List(vec![
            driver::ControlValue::Integer(1),
            driver::ControlValue::String("two".into()),
        ]),
        driver::ControlValue::Object(BTreeMap::from([
            ("port".to_owned(), driver::ControlValue::Integer(9420)),
            (
                "host".to_owned(),
                driver::ControlValue::IpAddress("10.0.0.1".into()),
            ),
        ])),
        driver::ControlValue::Unknown,
    ]
}

fn effect_samples() -> Vec<effect::ControlValue> {
    vec![
        effect::ControlValue::Float(0.75),
        effect::ControlValue::Integer(-7),
        effect::ControlValue::Boolean(false),
        effect::ControlValue::Color([0.1, 0.2, 0.3, 1.0]),
        effect::ControlValue::Gradient(vec![
            GradientStop {
                position: 0.0,
                color: [1.0, 0.0, 0.0, 1.0],
            },
            GradientStop {
                position: 1.0,
                color: [0.0, 0.0, 1.0, 0.5],
            },
        ]),
        effect::ControlValue::Enum("wave".into()),
        effect::ControlValue::Text("hello".into()),
        effect::ControlValue::Rect(ViewportRect::new(0.1, 0.2, 0.5, 0.4)),
    ]
}

#[test]
fn every_driver_value_roundtrips_byte_identically() {
    for original in driver_samples() {
        let original_json = serde_json::to_value(&original).expect("driver value serializes");
        let canonical =
            ControlValue::try_from(original.clone()).expect("driver value canonicalizes");
        let back = canonical
            .to_driver_wire()
            .expect("canonical projects back to the driver wire");
        let back_json = serde_json::to_value(&back).expect("projection serializes");
        assert_eq!(
            original_json, back_json,
            "driver wire drifted through canonical for {original:?}"
        );
    }
}

#[test]
fn every_effect_value_roundtrips_byte_identically() {
    for original in effect_samples() {
        let original_json = serde_json::to_value(&original).expect("effect value serializes");
        let canonical =
            ControlValue::try_from(original.clone()).expect("effect value canonicalizes");
        let back = canonical
            .to_effect_wire()
            .expect("canonical projects back to the effect wire");
        let back_json = serde_json::to_value(&back).expect("projection serializes");
        assert_eq!(
            original_json, back_json,
            "effect wire drifted through canonical for {original:?}"
        );
    }
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
        ControlValue::try_from(driver::ControlValue::IpAddress("not-an-ip".into())),
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
            ControlValue::try_from(driver::ControlValue::MacAddress(bad_mac.into())),
            Err(ControlValueInvalid::InvalidMac),
            "{bad_mac:?} must be rejected"
        );
    }
}

#[test]
fn non_finite_floats_are_rejected_everywhere() {
    assert_eq!(
        ControlValue::try_from(driver::ControlValue::Float(f64::NAN)),
        Err(ControlValueInvalid::NonFiniteFloat)
    );
    assert_eq!(
        ControlValue::try_from(effect::ControlValue::Float(f32::INFINITY)),
        Err(ControlValueInvalid::NonFiniteFloat)
    );
    assert_eq!(
        ControlValue::try_from(effect::ControlValue::Color([0.0, f32::NAN, 0.0, 1.0])),
        Err(ControlValueInvalid::NonFiniteFloat)
    );
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
    use hypercolor_types::control::DriverProjectionError as DriverError;
    assert_eq!(
        ControlValue::Duration(Duration::from_micros(1500)).to_driver_wire(),
        Err(DriverError::SubMillisecondDuration)
    );
    // Whole milliseconds project cleanly.
    assert_eq!(
        ControlValue::Duration(Duration::from_millis(1500)).to_driver_wire(),
        Ok(driver::ControlValue::DurationMs(1500))
    );
}

#[test]
fn effect_projection_rejects_driver_only_variants() {
    let driver_only = [
        ControlValue::Null,
        ControlValue::SecretRef(SecretRef::new("cred_x")),
        ControlValue::Ip(IpText::new("127.0.0.1").expect("valid")),
        ControlValue::Mac(MacText::new("00:11:22:33:44:55").expect("valid")),
        ControlValue::Duration(Duration::from_millis(5)),
        ControlValue::ColorRgb(hypercolor_color::Rgb::new(1, 2, 3)),
        ControlValue::ColorRgba(hypercolor_color::Rgba::new(1, 2, 3, 4)),
        ControlValue::Flags(vec!["a".into()]),
        ControlValue::List(vec![]),
        ControlValue::Map(BTreeMap::new()),
        ControlValue::Unknown,
    ];
    for value in driver_only {
        assert!(
            matches!(
                value.to_effect_wire(),
                Err(EffectProjectionError::DriverOnly(_))
            ),
            "{} must not reach the effect wire",
            value.kind_name()
        );
    }
}

#[test]
fn driver_projection_rejects_effect_only_variants() {
    let effect_only = [
        ControlValue::ColorLinear(hypercolor_color::LinearRgba::new(0.1, 0.2, 0.3, 1.0)),
        ControlValue::Gradient(vec![]),
        ControlValue::Rect(hypercolor_types::spatial::NormalizedRect {
            x: 0.0,
            y: 0.0,
            width: 1.0,
            height: 1.0,
        }),
    ];
    for value in effect_only {
        assert!(
            matches!(
                value.to_driver_wire(),
                Err(DriverProjectionError::EffectOnly(_))
            ),
            "{} must not reach the driver wire",
            value.kind_name()
        );
    }
}

#[test]
fn width_narrowing_is_range_checked() {
    assert!(matches!(
        ControlValue::Int(i64::from(i32::MAX) + 1).to_effect_wire(),
        Err(EffectProjectionError::IntOutOfRange(_))
    ));
    assert!(matches!(
        ControlValue::Float(f64::MAX).to_effect_wire(),
        Err(EffectProjectionError::FloatOverflow(_))
    ));
    assert!(matches!(
        ControlValue::Duration(Duration::from_secs(u64::MAX)).to_driver_wire(),
        Err(DriverProjectionError::DurationOverflow)
    ));
    // In-range values narrow cleanly.
    assert_eq!(
        ControlValue::Int(42).to_effect_wire(),
        Ok(effect::ControlValue::Integer(42))
    );
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
