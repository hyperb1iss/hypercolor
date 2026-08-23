use hypercolor_ui::api::client::{ApiError, http_error_from_body};
use serde_json::json;

fn envelope(code: &str, details: serde_json::Value) -> serde_json::Value {
    json!({
        "error": { "code": code, "message": "control is bound", "details": details },
        "meta": {
            "api_version": "1.0",
            "request_id": "req_019b1f9a-3f4b-7c8d-a2e1-91b4c0d86a25",
            "timestamp": "2026-08-22T00:00:00.000Z"
        }
    })
}

#[test]
fn canonical_envelope_keeps_the_code_and_details() {
    let error = http_error_from_body(
        409,
        &envelope("control_bound", json!({ "bound": ["speed"] })),
    );

    assert_eq!(error.code(), Some("control_bound"));
    assert_eq!(error.bound_control_keys(), vec!["speed".to_owned()]);
    assert_eq!(error.to_string(), "control is bound (HTTP 409)");
}

#[test]
fn bound_keys_are_empty_for_other_codes() {
    let error = http_error_from_body(409, &envelope("conflict", json!({ "bound": ["speed"] })));

    assert_eq!(error.code(), Some("conflict"));
    assert!(error.bound_control_keys().is_empty());
}

#[test]
fn precondition_details_survive_for_the_stale_rebase_path() {
    let error = http_error_from_body(
        412,
        &envelope(
            "precondition_failed",
            json!({ "current": 7, "expected": 6 }),
        ),
    );

    let ApiError::Http { details, .. } = &error else {
        panic!("expected an HTTP error");
    };
    assert_eq!(
        details.as_ref().and_then(|value| value.pointer("/current")),
        Some(&json!(7))
    );
}

#[test]
fn a_body_outside_the_envelope_degrades_to_the_status() {
    let error = http_error_from_body(500, &json!({ "unexpected": true }));

    assert_eq!(error.code(), None);
    assert_eq!(error.to_string(), "HTTP 500");
    assert!(error.bound_control_keys().is_empty());
}

#[test]
fn a_locally_raised_stale_error_carries_the_daemon_code() {
    let error = ApiError::precondition_failed("scene changed");

    assert_eq!(error.code(), Some("precondition_failed"));
}
