//! Every documented operation refuses malformed input with the envelope.
//!
//! A spelling fence keeps handlers from naming Axum's stock extractors,
//! but a closure handler whose parameter is an inferred constructor
//! pattern (`|axum::Json(value)| ...`) names no type and slips past any
//! such fence. This sweep checks behavior instead, so it does not
//! care how a handler is written. It reads the daemon's own OpenAPI
//! document and, for every operation, sends the malformed request that
//! operation's inputs allow: broken JSON where it takes a JSON body, an
//! undecodable segment where its path has parameters, and an unparseable
//! value for each typed query parameter. Each must answer with the
//! canonical error envelope and the code the error table maps it to.
//!
//! Routes outside the document (the API fallbacks, Swagger, MCP, static
//! files, and extension routes this test does not register) are not
//! swept here.

use std::sync::Arc;

use axum::body::Body;
use http::{Request, StatusCode, header};
use hypercolor_daemon::api;
use hypercolor_daemon::api::local::TrustedLocalApi;
use hypercolor_daemon::app_state::AppState;
use hypercolor_types::api::envelope::ApiErrorBody;
use serde_json::Value;
use tower::ServiceExt;

/// A syntactically valid value for any path parameter: every typed id the
/// API takes is a UUID, and a string parameter accepts it too.
const VALID_SEGMENT: &str = "00000000-0000-0000-0000-000000000000";
/// Percent-decodes to invalid UTF-8, which no path parameter type accepts.
const UNDECODABLE_SEGMENT: &str = "%FF";
/// Fails at the first key whatever the body type, so the probe reaches the
/// syntax error rather than a type's unknown-field or missing-field check.
const BROKEN_JSON: &str = "{,";
const UNPARSEABLE_QUERY_VALUE: &str = "sweep-not-a-value";

struct Harness {
    api: TrustedLocalApi,
    state: Arc<AppState>,
    _data_dir: tempfile::TempDir,
}

fn harness() -> Harness {
    let data_dir = tempfile::tempdir().expect("tempdir should be created");
    let state = Arc::new(AppState::new_with_data_dir(data_dir.path().join("data")));
    Harness {
        api: TrustedLocalApi::new(Arc::clone(&state)),
        state,
        _data_dir: data_dir,
    }
}

/// One malformed request and the refusal it must earn.
struct Probe {
    class: &'static str,
    method: String,
    uri: String,
    content_type: Option<&'static str>,
    body: &'static str,
    status: StatusCode,
    code: &'static str,
    field: Option<String>,
    message_contains: Option<&'static str>,
}

fn fill_path(path: &str, segment: &str) -> String {
    let mut filled = String::with_capacity(path.len());
    let mut rest = path;
    while let Some(open) = rest.find('{') {
        filled.push_str(&rest[..open]);
        filled.push_str(segment);
        let close = rest[open..].find('}').expect("path parameters close") + open;
        rest = &rest[close + 1..];
    }
    filled.push_str(rest);
    filled
}

/// Whether a query parameter's schema rejects an arbitrary word: numbers,
/// booleans, and enumerations do; free-form strings accept anything.
fn is_typed_query_schema(schema: &Value) -> bool {
    if schema.get("$ref").is_some() || schema.get("oneOf").is_some() {
        return true;
    }
    let types: Vec<&str> = match schema.get("type") {
        Some(Value::String(single)) => vec![single.as_str()],
        Some(Value::Array(many)) => many.iter().filter_map(Value::as_str).collect(),
        _ => Vec::new(),
    };
    types
        .iter()
        .any(|kind| matches!(*kind, "boolean" | "integer" | "number"))
}

fn probes(document: &Value) -> Vec<Probe> {
    let mut probes = Vec::new();
    let paths = document["paths"].as_object().expect("document has paths");
    for (path, item) in paths {
        if !path.starts_with("/api/v1/") {
            continue;
        }
        let item = item.as_object().expect("path item is an object");
        for (method, operation) in item {
            if !matches!(method.as_str(), "get" | "post" | "put" | "patch" | "delete") {
                continue;
            }
            let method = method.to_ascii_uppercase();

            let takes_json = operation["requestBody"]["content"]
                .get("application/json")
                .is_some();
            if takes_json {
                probes.push(Probe {
                    class: "json_body",
                    method: method.clone(),
                    uri: fill_path(path, VALID_SEGMENT),
                    content_type: Some("application/json"),
                    body: BROKEN_JSON,
                    status: StatusCode::BAD_REQUEST,
                    code: "malformed_request",
                    field: None,
                    message_contains: Some("JSON"),
                });
            }

            if path.contains('{') {
                probes.push(Probe {
                    class: "path",
                    method: method.clone(),
                    uri: fill_path(path, UNDECODABLE_SEGMENT),
                    content_type: None,
                    body: "",
                    status: StatusCode::BAD_REQUEST,
                    code: "malformed_request",
                    field: None,
                    message_contains: Some("Invalid URL"),
                });
            }

            let parameters = operation["parameters"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            for parameter in parameters {
                if parameter["in"] != "query" || !is_typed_query_schema(&parameter["schema"]) {
                    continue;
                }
                let name = parameter["name"]
                    .as_str()
                    .expect("parameter name")
                    .to_owned();
                probes.push(Probe {
                    class: "query",
                    method: method.clone(),
                    uri: format!(
                        "{}?{name}={UNPARSEABLE_QUERY_VALUE}",
                        fill_path(path, VALID_SEGMENT)
                    ),
                    content_type: None,
                    body: "",
                    status: StatusCode::UNPROCESSABLE_ENTITY,
                    code: "validation_error",
                    field: Some(name),
                    message_contains: None,
                });
            }
        }
    }
    probes
}

/// Send one probe and describe how it missed, if it did.
async fn miss(harness: &Harness, probe: &Probe) -> Option<String> {
    let mut builder = Request::builder()
        .method(probe.method.as_str())
        .uri(probe.uri.as_str());
    if let Some(content_type) = probe.content_type {
        builder = builder.header(header::CONTENT_TYPE, content_type);
    }
    let request = builder
        .body(Body::from(probe.body))
        .expect("probe request builds");
    let label = format!("[{}] {} {}", probe.class, probe.method, probe.uri);
    let response = match harness.api.execute(request).await {
        Ok(response) => response,
        Err(error) => return Some(format!("{label}: not routable in-process: {error}")),
    };
    let status = response.status();
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body reads");
    let text = String::from_utf8_lossy(&bytes);
    if !content_type.starts_with("application/json") {
        return Some(format!(
            "{label}: {status} {content_type:?} body {text:.160}"
        ));
    }
    let envelope: ApiErrorBody = match serde_json::from_slice(&bytes) {
        Ok(envelope) => envelope,
        Err(error) => {
            return Some(format!(
                "{label}: {status} not an envelope ({error}): {text:.160}"
            ));
        }
    };
    if status != probe.status || envelope.error.code != probe.code {
        return Some(format!(
            "{label}: expected {} {}, got {status} {}: {}",
            probe.status.as_u16(),
            probe.code,
            envelope.error.code,
            envelope.error.message
        ));
    }
    if let Some(field) = &probe.field {
        let reported = envelope
            .error
            .details
            .as_ref()
            .and_then(|details| details.get("field"))
            .and_then(Value::as_str);
        if reported != Some(field.as_str()) {
            return Some(format!(
                "{label}: details.field {reported:?}, expected {field:?}"
            ));
        }
    }
    if let Some(fragment) = probe.message_contains
        && !envelope.error.message.contains(fragment)
    {
        return Some(format!(
            "{label}: message lacks {fragment:?}: {}",
            envelope.error.message
        ));
    }
    None
}

#[tokio::test]
async fn every_documented_operation_refuses_malformed_input_with_the_envelope() {
    let harness = harness();
    let response = harness
        .api
        .execute(
            Request::builder()
                .uri("/api/v1/openapi.json")
                .body(Body::empty())
                .expect("document request"),
        )
        .await
        .expect("document is routable");
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("document reads");
    let document: Value = serde_json::from_slice(&bytes).expect("document is JSON");

    let probes = probes(&document);
    let mut misses = Vec::new();
    for probe in &probes {
        if let Some(missed) = miss(&harness, probe).await {
            misses.push(missed);
        }
    }

    let count = |class: &str| probes.iter().filter(|probe| probe.class == class).count();
    let (json_body, path, query) = (count("json_body"), count("path"), count("query"));
    eprintln!("swept {json_body} JSON-body, {path} path, and {query} typed-query probes");
    // Floors sit just under today's catalog, so a sweep that silently
    // stopped reading the document fails rather than passing on nothing.
    assert!(
        json_body >= 40,
        "only {json_body} JSON-body operations swept"
    );
    assert!(path >= 60, "only {path} path-parameter operations swept");
    assert!(query >= 18, "only {query} typed query parameters swept");
    assert!(
        misses.is_empty(),
        "{} of {} probes missed the envelope:\n{}",
        misses.len(),
        probes.len(),
        misses.join("\n")
    );
}

/// The WebSocket route is documented but takes no body, path, or query
/// input; its upgrade extractor is its one way to refuse a request.
#[tokio::test]
async fn a_plain_get_on_the_websocket_route_is_a_malformed_request_envelope() {
    let harness = harness();
    let response = api::build_router(Arc::clone(&harness.state), None)
        .oneshot(
            Request::builder()
                .uri("/api/v1/ws")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("router answers");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body reads");
    let envelope: ApiErrorBody =
        serde_json::from_slice(&bytes).expect("a refused upgrade is the envelope");
    assert_eq!(envelope.error.code, "malformed_request");
}
