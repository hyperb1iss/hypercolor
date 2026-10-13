//! Extractor rejections leave `/api/v1` as the canonical error envelope.
//!
//! The API extractors refuse malformed requests before a handler runs.
//! These tests pin that every such refusal carries `{ error, meta }` with
//! a stable code, the decoder's full diagnostic, and the offending field
//! path, and that a WebSocket `command` replaying the same request reports
//! the same status and the same error object a REST caller gets.

use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::extract::DefaultBodyLimit;
use axum::extract::ws::Message;
use http::{Request, StatusCode, header};
use hypercolor_daemon::api::extract::{API_BODY_LIMIT_BYTES, ApiJson};
use hypercolor_daemon::api::local::{TrustedLocalApi, TrustedLocalWebSocket};
use hypercolor_daemon::app_state::AppState;
use hypercolor_types::api::scene::SceneDocument;
use hypercolor_types::api::scenes::ReplaceSceneRequest;
use serde_json::{Value, json};
use tokio::time::timeout;
use tower::ServiceExt;

struct Harness {
    api: TrustedLocalApi,
    _data_dir: tempfile::TempDir,
}

fn harness() -> Harness {
    let data_dir = tempfile::tempdir().expect("tempdir should be created");
    let state = Arc::new(AppState::new_with_data_dir(data_dir.path().join("data")));
    Harness {
        api: TrustedLocalApi::new(state),
        _data_dir: data_dir,
    }
}

async fn send(harness: &Harness, request: Request<Body>) -> (StatusCode, Option<String>, Value) {
    let response = harness
        .api
        .execute(request)
        .await
        .expect("trusted request should reach the router");
    let status = response.status();
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body should read");
    let body = serde_json::from_slice(&bytes).unwrap_or_else(|error| {
        panic!(
            "a {status} body must be JSON ({error}): {}",
            String::from_utf8_lossy(&bytes)
        )
    });
    (status, content_type, body)
}

fn request(method: &str, uri: &str, content_type: Option<&str>, body: Body) -> Request<Body> {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(content_type) = content_type {
        builder = builder.header(header::CONTENT_TYPE, content_type);
    }
    builder.body(body).expect("request should build")
}

fn json_request(method: &str, uri: &str, body: &Value) -> Request<Body> {
    request(
        method,
        uri,
        Some("application/json"),
        Body::from(body.to_string()),
    )
}

/// Assert the canonical error envelope and return its `error` object.
fn assert_error_envelope(
    status: StatusCode,
    content_type: Option<&str>,
    body: &Value,
    expected_status: StatusCode,
    expected_code: &str,
) -> Value {
    assert_eq!(status, expected_status, "unexpected status for {body}");
    assert!(
        content_type.is_some_and(|value| value.starts_with("application/json")),
        "rejections must be JSON, got {content_type:?}"
    );
    assert_eq!(body["error"]["code"], expected_code, "body: {body}");
    assert!(
        body["error"]["message"]
            .as_str()
            .is_some_and(|message| !message.is_empty()),
        "every rejection names what went wrong: {body}"
    );
    assert!(
        body["meta"]["request_id"]
            .as_str()
            .is_some_and(|id| id.starts_with("req_")),
        "meta rides on every envelope: {body}"
    );
    body["error"].clone()
}

async fn create_scene(harness: &Harness, name: &str) -> String {
    let (status, _, body) = send(
        harness,
        json_request("POST", "/api/v1/scenes", &json!({ "name": name })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    body["data"]["id"]
        .as_str()
        .expect("scene id should be a string")
        .to_owned()
}

/// A full replacement body for `scene_id` with `key` set to `value`.
async fn replacement_with(harness: &Harness, scene_id: &str, key: &str, value: Value) -> Value {
    let uri = format!("/api/v1/scenes/{scene_id}");
    let (status, _, fetched) = send(harness, request("GET", &uri, None, Body::empty())).await;
    assert_eq!(status, StatusCode::OK);
    let document: SceneDocument =
        serde_json::from_value(fetched["data"].clone()).expect("scene document");
    let mut body =
        serde_json::to_value(ReplaceSceneRequest::from(&document)).expect("replacement body");
    body.as_object_mut()
        .expect("replacement body is an object")
        .insert(key.to_owned(), value);
    body
}

/// Send `body` as a WebSocket `command` and return the `response`.
async fn ws_command(harness: &Harness, method: &str, path: &str, body: &Value) -> Value {
    let mut socket = harness
        .api
        .open_websocket("/api/v1/ws")
        .expect("trusted websocket should open");
    recv_until_type(&mut socket, "hello").await;
    socket
        .send(Message::Text(
            json!({
                "type": "command",
                "id": "cmd-parity",
                "method": method,
                "path": path,
                "body": body,
            })
            .to_string()
            .into(),
        ))
        .await
        .expect("command should send");
    let reply = recv_until_type(&mut socket, "response").await;
    socket.shutdown().await;
    assert_eq!(reply["id"], "cmd-parity");
    reply
}

async fn recv_until_type(socket: &mut TrustedLocalWebSocket, expected: &str) -> Value {
    for _ in 0..16 {
        let message = timeout(Duration::from_secs(5), socket.recv())
            .await
            .expect("timed out waiting for a socket message")
            .expect("socket closed before the expected message");
        let Message::Text(text) = message else {
            continue;
        };
        let message: Value = serde_json::from_str(text.as_str()).expect("socket JSON");
        if message["type"] == expected {
            return message;
        }
    }
    panic!("no {expected} message within 16 attempts");
}

#[tokio::test]
async fn unknown_body_field_is_a_validation_envelope_naming_the_field() {
    let harness = harness();
    let scene_id = create_scene(&harness, "Unknown Field").await;
    let body = replacement_with(&harness, &scene_id, "frobnicate", json!(true)).await;

    let (status, content_type, response) = send(
        &harness,
        json_request("PUT", &format!("/api/v1/scenes/{scene_id}"), &body),
    )
    .await;

    let error = assert_error_envelope(
        status,
        content_type.as_deref(),
        &response,
        StatusCode::UNPROCESSABLE_ENTITY,
        "validation_error",
    );
    let message = error["message"].as_str().expect("message");
    assert!(
        message.contains("frobnicate"),
        "the message names the offending field: {message}"
    );
    assert_eq!(error["details"]["field"], "frobnicate");
}

#[tokio::test]
async fn a_nested_wrong_type_names_its_full_path() {
    let harness = harness();
    let scene_id = create_scene(&harness, "Nested Field").await;
    let mut body = replacement_with(&harness, &scene_id, "enabled", json!(true)).await;
    body["zones"][0]["brightness"] = json!("very bright");

    let (status, content_type, response) = send(
        &harness,
        json_request("PUT", &format!("/api/v1/scenes/{scene_id}"), &body),
    )
    .await;

    let error = assert_error_envelope(
        status,
        content_type.as_deref(),
        &response,
        StatusCode::UNPROCESSABLE_ENTITY,
        "validation_error",
    );
    assert_eq!(error["details"]["field"], "zones[0].brightness");
}

#[tokio::test]
async fn ws_command_reports_the_rest_status_and_error_verbatim() {
    let harness = harness();
    let scene_id = create_scene(&harness, "Socket Parity").await;
    let body = replacement_with(&harness, &scene_id, "frobnicate", json!(true)).await;
    let path = format!("/api/v1/scenes/{scene_id}");

    let (rest_status, _, rest) = send(&harness, json_request("PUT", &path, &body)).await;
    assert_eq!(rest_status, StatusCode::UNPROCESSABLE_ENTITY);

    let reply = ws_command(&harness, "PUT", &path, &body).await;
    assert_eq!(
        reply["status"],
        rest_status.as_u16(),
        "the socket reports the route's real status"
    );
    assert!(reply.get("data").is_none(), "a failure carries no data");
    assert_eq!(
        reply["error"], rest["error"],
        "the socket error object is the REST error object"
    );
}

#[tokio::test]
async fn a_long_diagnostic_survives_whole_over_rest_and_ws() {
    let harness = harness();
    let scene_id = create_scene(&harness, "Long Diagnostic").await;
    // serde quotes the offending value, so a 20,000-character string sent
    // for a boolean produces a diagnostic far past any small buffer.
    let long_value = "x".repeat(20_000);
    let body = replacement_with(&harness, &scene_id, "enabled", json!(long_value)).await;
    let path = format!("/api/v1/scenes/{scene_id}");

    let (status, content_type, rest) = send(&harness, json_request("PUT", &path, &body)).await;
    let error = assert_error_envelope(
        status,
        content_type.as_deref(),
        &rest,
        StatusCode::UNPROCESSABLE_ENTITY,
        "validation_error",
    );
    let message = error["message"].as_str().expect("message");
    assert!(
        message.len() > 16 * 1024,
        "the diagnostic is the long one ({} bytes)",
        message.len()
    );
    assert!(
        message.contains("enabled"),
        "names the field: {message:.120}"
    );
    assert!(
        message.contains(&long_value),
        "the quoted value is not truncated"
    );
    assert_eq!(error["details"]["field"], "enabled");

    let reply = ws_command(&harness, "PUT", &path, &body).await;
    assert_eq!(reply["status"], status.as_u16());
    assert_eq!(
        reply["error"], rest["error"],
        "the socket carries the same long diagnostic"
    );
}

#[tokio::test]
async fn unparseable_json_is_a_malformed_request_envelope() {
    let harness = harness();
    let (status, content_type, body) = send(
        &harness,
        request(
            "POST",
            "/api/v1/scenes",
            Some("application/json"),
            Body::from(r#"{"name": "#),
        ),
    )
    .await;
    assert_error_envelope(
        status,
        content_type.as_deref(),
        &body,
        StatusCode::BAD_REQUEST,
        "malformed_request",
    );
}

#[tokio::test]
async fn a_body_without_a_json_content_type_is_unsupported_media_type() {
    let harness = harness();
    let (status, content_type, body) = send(
        &harness,
        request(
            "POST",
            "/api/v1/scenes",
            None,
            Body::from(r#"{"name":"No Content Type"}"#),
        ),
    )
    .await;
    assert_error_envelope(
        status,
        content_type.as_deref(),
        &body,
        StatusCode::UNSUPPORTED_MEDIA_TYPE,
        "unsupported_media_type",
    );
}

#[tokio::test]
async fn an_oversized_body_is_payload_too_large_without_a_guessed_limit() {
    let harness = harness();
    let oversized = json!({ "name": "x".repeat(2 * 1024 * 1024 + 1) });
    let (status, content_type, body) =
        send(&harness, json_request("POST", "/api/v1/scenes", &oversized)).await;
    let error = assert_error_envelope(
        status,
        content_type.as_deref(),
        &body,
        StatusCode::PAYLOAD_TOO_LARGE,
        "payload_too_large",
    );
    assert!(
        error.get("details").is_none(),
        "the extractor cannot see which limit fired, so it names none: {error}"
    );
}

/// An extension route that opts into `ApiJson` under its own ceiling.
async fn echo(ApiJson(value): ApiJson<Value>) -> axum::response::Response {
    axum::response::IntoResponse::into_response(axum::Json(value))
}

#[tokio::test]
async fn a_route_with_its_own_limit_never_reports_the_router_default() {
    let router = axum::Router::new()
        .route("/echo", axum::routing::post(echo))
        .layer(DefaultBodyLimit::max(1024));

    let small = router
        .clone()
        .oneshot(json_request("POST", "/echo", &json!({ "ok": true })))
        .await
        .expect("router answers");
    assert_eq!(
        small.status(),
        StatusCode::OK,
        "bodies under the limit decode"
    );

    let response = router
        .oneshot(json_request(
            "POST",
            "/echo",
            &json!({ "pad": "x".repeat(2048) }),
        ))
        .await
        .expect("router answers");
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body reads");
    let body: Value = serde_json::from_slice(&bytes).expect("413 is the JSON envelope");
    assert_eq!(body["error"]["code"], "payload_too_large");
    assert!(
        body["error"].get("details").is_none(),
        "no limit is stated, least of all the router's 2 MiB: {body}"
    );
    let message = body["error"]["message"].as_str().expect("message");
    assert!(
        !message.contains(&API_BODY_LIMIT_BYTES.to_string()),
        "the message names no false limit: {message}"
    );
}

#[tokio::test]
async fn an_unparseable_path_segment_is_a_malformed_request_envelope() {
    let harness = harness();
    let (status, content_type, body) = send(
        &harness,
        request("GET", "/api/v1/assets/not-a-uuid", None, Body::empty()),
    )
    .await;
    assert_error_envelope(
        status,
        content_type.as_deref(),
        &body,
        StatusCode::BAD_REQUEST,
        "malformed_request",
    );
}

#[tokio::test]
async fn an_unparseable_query_is_a_validation_envelope_naming_the_key() {
    let harness = harness();
    let (status, content_type, body) = send(
        &harness,
        request("GET", "/api/v1/layouts?limit=lots", None, Body::empty()),
    )
    .await;
    let error = assert_error_envelope(
        status,
        content_type.as_deref(),
        &body,
        StatusCode::UNPROCESSABLE_ENTITY,
        "validation_error",
    );
    let message = error["message"].as_str().expect("message");
    assert!(message.contains("limit"), "names the key: {message}");
    assert_eq!(error["details"]["field"], "limit");
}

#[tokio::test]
async fn success_bodies_keep_their_shape() {
    let harness = harness();
    let scene_id = create_scene(&harness, "Untouched").await;
    let (status, content_type, body) = send(
        &harness,
        request(
            "GET",
            &format!("/api/v1/scenes/{scene_id}"),
            None,
            Body::empty(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(content_type.is_some_and(|value| value.starts_with("application/json")));
    assert_eq!(body["data"]["id"], scene_id);
    assert!(body.get("error").is_none());
}
