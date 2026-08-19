use hypercolor_types::source_status::{
    SOURCE_DIAGNOSTICS_DISPLAY_FIELD_MAX_COUNT, SOURCE_DIAGNOSTICS_DISPLAY_KEY_MAX_BYTES,
    SOURCE_DIAGNOSTICS_DISPLAY_LABEL_MAX_BYTES, SOURCE_DIAGNOSTICS_DISPLAY_VALUE_MAX_BYTES,
    SOURCE_DIAGNOSTICS_PAYLOAD_MAX_BYTES, SOURCE_DIAGNOSTICS_SCHEMA_MAX_BYTES,
    SOURCE_DIAGNOSTICS_VERSION_MAX, SourceDiagnosticsDisplayField, SourceDiagnosticsEnvelope,
    SourceDiagnosticsEnvelopeError,
};
use serde_json::json;
use utoipa::PartialSchema;

fn field() -> SourceDiagnosticsDisplayField {
    SourceDiagnosticsDisplayField::new("authorization", "Authorization", "authorized")
}

#[test]
fn unknown_bounded_version_round_trips_opaquely() {
    let envelope = SourceDiagnosticsEnvelope::try_new(
        "platform.input",
        17,
        vec![field()],
        json!({"future": {"shape": true}}),
    )
    .expect("unknown bounded version should remain relayable");
    let json = serde_json::to_string(&envelope).expect("serialize envelope");
    let restored: SourceDiagnosticsEnvelope =
        serde_json::from_str(&json).expect("deserialize envelope");
    assert_eq!(restored, envelope);
}

#[test]
fn malformed_and_oversized_payloads_are_rejected() {
    assert_eq!(
        SourceDiagnosticsEnvelope::try_from_json("platform.input", 1, vec![], "{"),
        Err(SourceDiagnosticsEnvelopeError::MalformedPayload)
    );
    let oversized = "x".repeat(SOURCE_DIAGNOSTICS_PAYLOAD_MAX_BYTES + 1);
    assert_eq!(
        SourceDiagnosticsEnvelope::try_new(
            "platform.input",
            1,
            vec![],
            serde_json::Value::String(oversized),
        ),
        Err(SourceDiagnosticsEnvelopeError::PayloadTooLarge)
    );
}

#[test]
fn display_and_version_bounds_apply_during_deserialization() {
    let display = (0..=SOURCE_DIAGNOSTICS_DISPLAY_FIELD_MAX_COUNT)
        .map(|index| {
            json!({"key": format!("field_{index}"), "label": format!("field {index}"), "value": "ok"})
        })
        .collect::<Vec<_>>();
    let invalid = json!({
        "schema": "platform.input",
        "version": 0,
        "display": display,
        "payload": {},
    });
    assert!(serde_json::from_value::<SourceDiagnosticsEnvelope>(invalid).is_err());
}

#[test]
fn string_bounds_apply_before_values_enter_the_envelope() {
    let cases = [
        format!(
            r#"{{"schema":"{}","version":1,"display":[],"payload":{{}}}}"#,
            "s".repeat(SOURCE_DIAGNOSTICS_SCHEMA_MAX_BYTES + 1)
        ),
        format!(
            r#"{{"schema":"platform.input","version":1,"display":[{{"key":"{}","label":"Label","value":"ok"}}],"payload":{{}}}}"#,
            "k".repeat(SOURCE_DIAGNOSTICS_DISPLAY_KEY_MAX_BYTES + 1)
        ),
        format!(
            r#"{{"schema":"platform.input","version":1,"display":[{{"key":"key","label":"{}","value":"ok"}}],"payload":{{}}}}"#,
            "l".repeat(SOURCE_DIAGNOSTICS_DISPLAY_LABEL_MAX_BYTES + 1)
        ),
        format!(
            r#"{{"schema":"platform.input","version":1,"display":[{{"key":"key","label":"Label","value":"{}"}}],"payload":{{}}}}"#,
            "v".repeat(SOURCE_DIAGNOSTICS_DISPLAY_VALUE_MAX_BYTES + 1)
        ),
    ];

    for case in cases {
        assert!(serde_json::from_str::<SourceDiagnosticsEnvelope>(&case).is_err());
    }
}

#[test]
fn display_collection_rejects_excess_without_retaining_the_tail() {
    let field = r#"{"key":"key","label":"Label","value":"ok"}"#;
    let display = std::iter::repeat_n(field, 10_000)
        .collect::<Vec<_>>()
        .join(",");
    let wire = format!(
        r#"{{"schema":"platform.input","version":1,"display":[{display}],"payload":{{}}}}"#
    );

    let error = serde_json::from_str::<SourceDiagnosticsEnvelope>(&wire)
        .expect_err("oversized display collection must fail");
    assert!(error.to_string().contains("display field count"));
}

#[test]
fn payload_deserialization_stops_at_the_serialized_byte_budget() {
    let payload = std::iter::repeat_n("null", SOURCE_DIAGNOSTICS_PAYLOAD_MAX_BYTES)
        .collect::<Vec<_>>()
        .join(",");
    let wire =
        format!(r#"{{"schema":"platform.input","version":1,"display":[],"payload":[{payload}]}}"#);

    let error = serde_json::from_str::<SourceDiagnosticsEnvelope>(&wire)
        .expect_err("oversized payload collection must fail");
    assert!(error.to_string().contains("payload exceeds its byte bound"));
}

#[test]
fn payload_string_accepts_the_exact_limit_and_rejects_one_byte_more() {
    let exact_payload = "x".repeat(SOURCE_DIAGNOSTICS_PAYLOAD_MAX_BYTES - 2);
    let exact = format!(
        r#"{{"schema":"platform.input","version":1,"display":[],"payload":"{exact_payload}"}}"#
    );
    let envelope: SourceDiagnosticsEnvelope =
        serde_json::from_str(&exact).expect("exact payload byte limit should deserialize");
    assert_eq!(envelope.payload().as_str(), Some(exact_payload.as_str()));

    let oversized_payload = "x".repeat(SOURCE_DIAGNOSTICS_PAYLOAD_MAX_BYTES - 1);
    let oversized = format!(
        r#"{{"schema":"platform.input","version":1,"display":[],"payload":"{oversized_payload}"}}"#
    );
    assert!(serde_json::from_str::<SourceDiagnosticsEnvelope>(&oversized).is_err());
}

#[test]
fn openapi_schema_carries_every_representable_runtime_bound() {
    let envelope_schema =
        serde_json::to_value(<SourceDiagnosticsEnvelope as PartialSchema>::schema())
            .expect("serialize schema");
    assert_eq!(envelope_schema["properties"]["schema"]["minLength"], 1);
    assert_eq!(
        envelope_schema["properties"]["schema"]["maxLength"],
        SOURCE_DIAGNOSTICS_SCHEMA_MAX_BYTES
    );
    assert_eq!(envelope_schema["properties"]["version"]["minimum"], 1);
    assert_eq!(
        envelope_schema["properties"]["version"]["maximum"],
        SOURCE_DIAGNOSTICS_VERSION_MAX
    );
    assert_eq!(
        envelope_schema["properties"]["display"]["maxItems"],
        SOURCE_DIAGNOSTICS_DISPLAY_FIELD_MAX_COUNT
    );
    assert!(
        envelope_schema["properties"]["payload"]["description"]
            .as_str()
            .is_some_and(|description| description.contains("16384"))
    );

    let field_schema =
        serde_json::to_value(<SourceDiagnosticsDisplayField as PartialSchema>::schema())
            .expect("serialize display field schema");
    assert_eq!(field_schema["properties"]["key"]["minLength"], 1);
    assert_eq!(
        field_schema["properties"]["key"]["maxLength"],
        SOURCE_DIAGNOSTICS_DISPLAY_KEY_MAX_BYTES
    );
    assert_eq!(field_schema["properties"]["label"]["minLength"], 1);
    assert_eq!(
        field_schema["properties"]["label"]["maxLength"],
        SOURCE_DIAGNOSTICS_DISPLAY_LABEL_MAX_BYTES
    );
    assert_eq!(
        field_schema["properties"]["value"]["maxLength"],
        SOURCE_DIAGNOSTICS_DISPLAY_VALUE_MAX_BYTES
    );
}
