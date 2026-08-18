//! REST wire matrix: the pinned shapes.
//!
//! Every assertion here describes what the daemon emits. Under Spec 76 §0's
//! lockstep doctrine these pins are intentionality fences, not freezes: a
//! deliberate shape change updates the pin, every in-repo client, and this
//! file in the same PR, while an unintended byte shift still fails CI. A
//! failure here that nobody meant is a wire regression.
//!
//! The human-readable companion is `tests/fixtures/rest_v1/MATRIX.md`; the two
//! are edited together.

use std::sync::Arc;
use std::time::SystemTime;

use axum::body::Body;
use http::{Request, StatusCode};
use hypercolor_core::effect::EffectEntry;
use hypercolor_daemon::api::{self, AppState};
use hypercolor_types::effect::{
    ControlDefinition, ControlKind, ControlType, ControlValue, EffectCategory, EffectId,
    EffectMetadata, EffectSource, EffectState,
};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

// ── Harness ──────────────────────────────────────────────────────────────

fn isolated_state() -> (Arc<AppState>, tempfile::TempDir) {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let data_dir = tempdir.path().join("data");
    std::fs::create_dir_all(&data_dir).expect("temp data dir should be created");
    (Arc::new(AppState::new_with_data_dir(data_dir)), tempdir)
}

fn test_app(state: &Arc<AppState>) -> axum::Router {
    api::build_router(Arc::clone(state), None)
}

async fn send(app: &axum::Router, request: Request<Body>) -> axum::response::Response {
    app.clone()
        .oneshot(request)
        .await
        .expect("request should be served")
}

async fn body_json(response: axum::response::Response) -> Value {
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body should read");
    serde_json::from_slice(&bytes).expect("response body should be JSON")
}

fn empty_request(method: &str, uri: &str) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .body(Body::empty())
        .expect("request should build")
}

fn get(uri: &str) -> Request<Body> {
    empty_request("GET", uri)
}

fn json_request(method: &str, uri: &str, body: &Value) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .expect("request should build")
}

fn with_header(mut request: Request<Body>, name: http::HeaderName, value: &str) -> Request<Body> {
    request.headers_mut().insert(
        name,
        http::HeaderValue::from_str(value).expect("header value should be ASCII"),
    );
    request
}

fn if_match(request: Request<Body>, raw: &str) -> Request<Body> {
    with_header(request, http::header::IF_MATCH, raw)
}

/// Serve the router over a real loopback socket.
///
/// `tower::oneshot` cannot reach any code behind the `WebSocketUpgrade`
/// extractor: without a live hyper connection the extractor rejects with 426
/// before the handler body runs, so an upgrade-path freeze needs real I/O.
async fn spawn_server(state: &Arc<AppState>) -> std::net::SocketAddr {
    let router = test_app(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("ephemeral port should bind");
    let addr = listener
        .local_addr()
        .expect("listener should have an address");
    tokio::spawn(async move {
        let _ = axum::serve(
            listener,
            router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await;
    });
    addr
}

fn etag(response: &axum::response::Response) -> String {
    response
        .headers()
        .get(http::header::ETAG)
        .and_then(|value| value.to_str().ok())
        .expect("response should carry an ETag")
        .to_owned()
}

// ── Shape assertions ─────────────────────────────────────────────────────

/// Assert an object carries exactly these keys, in any order.
///
/// Key-set equality (rather than "contains") is what makes this a freeze: a
/// field appearing on a v1 body is as much a break as one disappearing.
fn assert_keys(value: &Value, expected: &[&str], what: &str) {
    let object = value
        .as_object()
        .unwrap_or_else(|| panic!("{what} should be a JSON object, got {value}"));
    let mut actual: Vec<&str> = object.keys().map(String::as_str).collect();
    actual.sort_unstable();
    let mut expected = expected.to_vec();
    expected.sort_unstable();
    assert_eq!(actual, expected, "{what} key set is frozen");
}

/// The `meta` block carried by both the success and the error envelope.
///
/// `request_id` and `timestamp` vary per response, so they are pinned by
/// grammar rather than value; `api_version` is a frozen literal.
fn assert_meta(meta: &Value) {
    assert_keys(
        meta,
        &["api_version", "request_id", "timestamp"],
        "v1 envelope meta",
    );
    assert_eq!(
        meta["api_version"],
        json!("1.0"),
        "meta.api_version is frozen at the string \"1.0\""
    );

    let request_id = meta["request_id"]
        .as_str()
        .expect("meta.request_id should be a string");
    let uuid = request_id
        .strip_prefix("req_")
        .expect("meta.request_id should carry the frozen `req_` prefix");
    Uuid::parse_str(uuid).expect("meta.request_id should suffix a UUID");

    assert_iso8601_millis(
        meta["timestamp"]
            .as_str()
            .expect("meta.timestamp should be a string"),
    );
}

/// The frozen timestamp grammar: `YYYY-MM-DDTHH:MM:SS.mmmZ`. Always UTC,
/// always exactly three fractional digits, never an offset form.
fn assert_iso8601_millis(timestamp: &str) {
    let shaped = timestamp.len() == 24
        && timestamp
            .as_bytes()
            .iter()
            .enumerate()
            .all(|(index, byte)| match index {
                4 | 7 => *byte == b'-',
                10 => *byte == b'T',
                13 | 16 => *byte == b':',
                19 => *byte == b'.',
                23 => *byte == b'Z',
                _ => byte.is_ascii_digit(),
            });
    assert!(
        shaped,
        "timestamp should match YYYY-MM-DDTHH:MM:SS.mmmZ, got {timestamp}"
    );
}

/// The v1 success envelope: `{ data, meta }`.
fn assert_envelope(body: &Value) {
    assert_keys(body, &["data", "meta"], "v1 success envelope");
    assert_meta(&body["meta"]);
}

/// The canonical error envelope: `{ error: { code, message, details? }, meta }`.
///
/// `details` carries `skip_serializing_if = "Option::is_none"`, so the key is
/// **absent** — not null — on errors with no structured context. Key-set
/// equality against the expected presence is what makes this a fence: a
/// `details` block appearing where none was intended fails as loudly as one
/// disappearing.
fn assert_error_envelope(body: &Value, code: &str, details: DetailsPresence) {
    assert_keys(body, &["error", "meta"], "canonical error envelope");
    assert_meta(&body["meta"]);
    let error = &body["error"];
    let expected: &[&str] = match details {
        DetailsPresence::Absent => &["code", "message"],
        DetailsPresence::Present => &["code", "message", "details"],
    };
    assert_keys(error, expected, "canonical error body");
    assert_eq!(error["code"], json!(code), "error.code is pinned");
    assert!(
        error["message"].is_string(),
        "error.message should be a string, got {}",
        error["message"]
    );
}

/// Whether an error carries structured context, which decides its key set.
#[derive(Clone, Copy)]
enum DetailsPresence {
    Absent,
    Present,
}

/// The canonical 412 body: the error envelope with `precondition_failed` and
/// a `details` object carrying both versions.
///
/// One rendering serves all three version counters (`controls_version`,
/// `zones_revision`, `layers_version`) — the counter's identity lives in the
/// route, not in a bespoke body shape. A client rebases off
/// `error.details.current` (or the `ETag`, which carries the same value).
fn assert_precondition_body(body: &Value, expected: u64, current: u64) {
    assert_error_envelope(body, "precondition_failed", DetailsPresence::Present);
    assert_keys(
        &body["error"]["details"],
        &["expected", "current"],
        "412 details",
    );
    assert_eq!(
        body["error"]["details"]["expected"],
        json!(expected),
        "412 names the version the caller asked for"
    );
    assert_eq!(
        body["error"]["details"]["current"],
        json!(current),
        "412 names the version the resource holds"
    );
}

/// The frozen v1 pagination block.
///
/// Six list endpoints fabricate this: they return every row while reporting
/// `limit: 50, has_more: false`, and they take no query parameters at all.
/// Spec 76 wave 3.3 corrects pagination on canonical routes only, so the lie
/// is contract on v1 and is asserted verbatim.
fn assert_frozen_pagination(pagination: &Value, total: usize) {
    assert_keys(
        pagination,
        &["offset", "limit", "total", "has_more"],
        "v1 pagination block",
    );
    assert_eq!(
        pagination["offset"],
        json!(0),
        "pagination.offset is frozen"
    );
    assert_eq!(pagination["limit"], json!(50), "pagination.limit is frozen");
    assert_eq!(
        pagination["total"],
        json!(total),
        "pagination.total counts every row"
    );
    assert_eq!(
        pagination["has_more"],
        json!(false),
        "pagination.has_more is frozen"
    );
}

/// Prove a legacy path still reaches a v1 handler rather than the router's
/// fallback. Only handlers emit the `meta` block, so its presence is the
/// routing receipt whether or not the call itself succeeded, which lets a route
/// be frozen as *routed* without staging the deep state a success needs.
async fn assert_reached_v1_handler(response: axum::response::Response) -> Value {
    let status = response.status();
    assert_ne!(
        status,
        StatusCode::METHOD_NOT_ALLOWED,
        "the path should still accept this method"
    );
    let json = body_json(response).await;
    assert!(
        json.get("meta").is_some(),
        "a v1 handler always emits meta; got {json}"
    );
    assert_meta(&json["meta"]);
    json
}

/// A frozen list body: `{ items: [...], pagination: {...} }`, where every row
/// ships in `items` regardless of the `limit` the block advertises.
fn assert_frozen_list(data: &Value, expected_items: usize) {
    assert_keys(data, &["items", "pagination"], "v1 frozen list body");
    let items = data["items"].as_array().expect("items should be an array");
    assert_eq!(items.len(), expected_items, "the list returns every row");
    assert_frozen_pagination(&data["pagination"], expected_items);
}

// ── Fixtures ─────────────────────────────────────────────────────────────

fn sample_effect(name: &str) -> EffectMetadata {
    EffectMetadata {
        id: EffectId::new(Uuid::now_v7()),
        name: name.to_owned(),
        author: "test".to_owned(),
        version: "0.1.0".to_owned(),
        description: format!("{name} description"),
        category: EffectCategory::Ambient,
        tags: vec!["test".to_owned()],
        controls: vec![ControlDefinition {
            id: "speed".to_owned(),
            name: "Speed".to_owned(),
            kind: ControlKind::Number,
            control_type: ControlType::Slider,
            default_value: ControlValue::Float(5.0),
            min: Some(0.0),
            max: Some(100.0),
            step: Some(0.5),
            labels: Vec::new(),
            group: Some("General".to_owned()),
            tooltip: None,
            aspect_lock: None,
            preview_source: None,
            binding: None,
        }],
        presets: Vec::new(),
        audio_reactive: false,
        screen_reactive: false,
        input_reactive: false,
        source: EffectSource::Native {
            path: format!("builtin/{name}").into(),
        },
        license: None,
    }
}

async fn register_effect(state: &Arc<AppState>, name: &str) -> EffectMetadata {
    let metadata = sample_effect(name);
    let mut registry = state.effect_registry.write().await;
    let _ = registry.register(EffectEntry {
        metadata: metadata.clone(),
        source_path: format!("/tmp/{name}.html").into(),
        modified: SystemTime::now(),
        state: EffectState::Loading,
    });
    metadata
}

/// The default scene's primary zone id, which every zone and layer route is
/// addressed through.
async fn primary_zone_id(app: &axum::Router) -> String {
    let response = send(app, get("/api/v1/scenes/default/zones")).await;
    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    json["data"]["items"][0]["id"]
        .as_str()
        .expect("the default scene should be born with a primary zone")
        .to_owned()
}

// ── Envelope ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn success_envelope_is_frozen_on_a_representative_get() {
    let (state, _tmp) = isolated_state();
    let app = test_app(&state);

    let response = send(&app, get("/api/v1/scenes")).await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_envelope(&body_json(response).await);
}

#[tokio::test]
async fn success_envelope_is_frozen_on_a_201_created() {
    let (state, _tmp) = isolated_state();
    let app = test_app(&state);

    let response = send(
        &app,
        json_request("POST", "/api/v1/profiles", &json!({ "name": "Compat" })),
    )
    .await;

    assert_eq!(response.status(), StatusCode::CREATED);
    assert_envelope(&body_json(response).await);
}

#[tokio::test]
async fn health_probe_stays_outside_the_envelope() {
    let (state, _tmp) = isolated_state();
    let app = test_app(&state);

    // `/health` is mounted on the outer router, not under `/api/v1`.
    let response = send(&app, get("/health")).await;
    assert_eq!(response.status(), StatusCode::OK);

    let json = body_json(response).await;
    assert_keys(
        &json,
        &["status", "version", "uptime_seconds", "checks"],
        "health probe body",
    );
    assert_eq!(json["status"], json!("healthy"));
    assert!(json["version"].is_string(), "health.version is a string");
    assert!(
        json["uptime_seconds"].is_u64(),
        "health.uptime_seconds is an unsigned integer"
    );
    assert_keys(
        &json["checks"],
        &["render_loop", "device_backends", "event_bus"],
        "health checks block",
    );
    assert_eq!(json["checks"]["render_loop"], json!("idle"));
    assert_eq!(json["checks"]["device_backends"], json!("ok"));
    assert_eq!(json["checks"]["event_bus"], json!("idle"));

    // There is no `/api/v1/health` alias; the probe lives at one path only.
    let nested = send(&app, get("/api/v1/health")).await;
    assert_eq!(nested.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn health_probe_reports_503_when_degraded() {
    let (state, _tmp) = isolated_state();
    state.render_loop.write().await.stop();
    let app = test_app(&state);

    let response = send(&app, get("/health")).await;

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let json = body_json(response).await;
    assert_eq!(json["status"], json!("degraded"));
    assert_eq!(json["checks"]["render_loop"], json!("degraded"));
}

// ── Pagination: the six frozen list endpoints ────────────────────────────

#[tokio::test]
async fn effects_list_freezes_the_fabricated_pagination_block() {
    let (state, _tmp) = isolated_state();
    register_effect(&state, "solid_color").await;
    register_effect(&state, "rainbow").await;
    let app = test_app(&state);

    let response = send(&app, get("/api/v1/effects")).await;

    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_envelope(&json);
    assert_frozen_list(&json["data"], 2);
}

#[tokio::test]
async fn scenes_list_freezes_the_fabricated_pagination_block() {
    let (state, _tmp) = isolated_state();
    let app = test_app(&state);

    // The default scene is ephemeral, and the list filters ephemeral scenes
    // out before counting, so a fresh daemon reports zero rows.
    let empty = send(&app, get("/api/v1/scenes")).await;
    assert_eq!(empty.status(), StatusCode::OK);
    assert_frozen_list(&body_json(empty).await["data"], 0);

    let created = send(
        &app,
        json_request("POST", "/api/v1/scenes", &json!({ "name": "Evening" })),
    )
    .await;
    assert_eq!(created.status(), StatusCode::CREATED);

    let response = send(&app, get("/api/v1/scenes")).await;

    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_envelope(&json);
    assert_frozen_list(&json["data"], 1);
}

#[tokio::test]
async fn profiles_list_freezes_the_fabricated_pagination_block() {
    let (state, _tmp) = isolated_state();
    let app = test_app(&state);

    let created = send(
        &app,
        json_request("POST", "/api/v1/profiles", &json!({ "name": "Evening" })),
    )
    .await;
    assert_eq!(created.status(), StatusCode::CREATED);

    let response = send(&app, get("/api/v1/profiles")).await;

    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_envelope(&json);
    assert_frozen_list(&json["data"], 1);
}

#[tokio::test]
async fn library_favorites_list_freezes_the_fabricated_pagination_block() {
    let (state, _tmp) = isolated_state();
    register_effect(&state, "solid_color").await;
    let app = test_app(&state);

    let added = send(
        &app,
        json_request(
            "POST",
            "/api/v1/library/favorites",
            &json!({ "effect": "solid_color" }),
        ),
    )
    .await;
    assert_eq!(added.status(), StatusCode::OK);

    let response = send(&app, get("/api/v1/library/favorites")).await;

    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_envelope(&json);
    assert_frozen_list(&json["data"], 1);
}

#[tokio::test]
async fn library_presets_list_freezes_the_fabricated_pagination_block() {
    let (state, _tmp) = isolated_state();
    register_effect(&state, "solid_color").await;
    let app = test_app(&state);

    let created = send(
        &app,
        json_request(
            "POST",
            "/api/v1/library/presets",
            &json!({ "name": "Warm Sweep", "effect": "solid_color" }),
        ),
    )
    .await;
    assert_eq!(created.status(), StatusCode::CREATED);

    let response = send(&app, get("/api/v1/library/presets")).await;

    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_envelope(&json);
    assert_frozen_list(&json["data"], 1);
}

#[tokio::test]
async fn library_playlists_list_freezes_the_fabricated_pagination_block() {
    let (state, _tmp) = isolated_state();
    register_effect(&state, "solid_color").await;
    let app = test_app(&state);

    let created = send(
        &app,
        json_request(
            "POST",
            "/api/v1/library/playlists",
            &json!({
                "name": "Night Rotation",
                "items": [{
                    "target": { "type": "effect", "effect": "solid_color" },
                    "duration_ms": 2000
                }]
            }),
        ),
    )
    .await;
    assert_eq!(created.status(), StatusCode::CREATED);

    let response = send(&app, get("/api/v1/library/playlists")).await;

    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_envelope(&json);
    assert_frozen_list(&json["data"], 1);
}

#[tokio::test]
async fn frozen_list_endpoints_ignore_offset_and_limit_query_params() {
    let (state, _tmp) = isolated_state();
    register_effect(&state, "solid_color").await;
    register_effect(&state, "rainbow").await;
    register_effect(&state, "aurora").await;
    let app = test_app(&state);

    // Effects is the one row of the six with a `Query` extractor, and it
    // names only the Spec 78 §2.1 filters. Paging arguments fall outside
    // that set, so they stay silently discarded rather than rejected.
    let response = send(&app, get("/api/v1/effects?offset=2&limit=1")).await;

    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_frozen_list(&json["data"], 3);
}

#[tokio::test]
async fn devices_list_pagination_stays_honest() {
    // The shared `Pagination` type is also used by endpoints that really page.
    // Freezing the honest sites alongside the fabricated ones keeps a later
    // pagination refactor from "fixing" v1 by flattening both into one shape.
    let (state, _tmp) = isolated_state();
    let app = test_app(&state);

    let response = send(&app, get("/api/v1/devices?offset=0&limit=10")).await;

    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_envelope(&json);
    let pagination = &json["data"]["pagination"];
    assert_keys(
        pagination,
        &["offset", "limit", "total", "has_more"],
        "devices pagination block",
    );
    assert_eq!(pagination["offset"], json!(0));
    assert_eq!(pagination["limit"], json!(10), "devices honors ?limit=");
    assert_eq!(pagination["has_more"], json!(false));
}

// ── Legacy paths stay routed and legacy-shaped ───────────────────────────

#[tokio::test]
async fn legacy_effect_apply_path_stays_routed() {
    let (state, _tmp) = isolated_state();
    register_effect(&state, "solid_color").await;
    let app = test_app(&state);

    let response = send(
        &app,
        empty_request("POST", "/api/v1/effects/solid_color/apply"),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_envelope(&json);
    assert_keys(
        &json["data"],
        &[
            "effect",
            "applied_controls",
            "layout",
            "transition",
            "warnings",
        ],
        "legacy /effects/{id}/apply body",
    );
    assert_keys(
        &json["data"]["effect"],
        &["id", "name"],
        "applied effect ref",
    );
    assert_eq!(json["data"]["effect"]["name"], json!("solid_color"));
}

#[tokio::test]
async fn effects_active_controls_keeps_its_unversioned_shape() {
    let (state, _tmp) = isolated_state();
    register_effect(&state, "solid_color").await;
    let app = test_app(&state);
    let applied = send(
        &app,
        empty_request("POST", "/api/v1/effects/solid_color/apply"),
    )
    .await;
    assert_eq!(applied.status(), StatusCode::OK);

    let response = send(
        &app,
        json_request(
            "PATCH",
            "/api/v1/effects/active/controls",
            &json!({ "controls": { "speed": 3.0 } }),
        ),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        response.headers().get(http::header::ETAG).is_none(),
        "the `active` route carries no ETag, unlike its `{{id}}` sibling"
    );
    let json = body_json(response).await;
    assert_envelope(&json);
    assert_keys(
        &json["data"],
        &["effect", "applied", "rejected"],
        "legacy /effects/active/controls body",
    );
    assert_eq!(json["data"]["effect"], json!("solid_color"));
}

#[tokio::test]
async fn effects_active_binding_and_reset_paths_stay_routed() {
    let (state, _tmp) = isolated_state();
    register_effect(&state, "solid_color").await;
    let app = test_app(&state);
    let applied = send(
        &app,
        empty_request("POST", "/api/v1/effects/solid_color/apply"),
    )
    .await;
    assert_eq!(applied.status(), StatusCode::OK);

    // The binding route takes a bare `ControlBinding`, not a wrapper object.
    // Whether this particular sensor validates is not the frozen fact; that the
    // path reaches a v1 handler and answers in the envelope is.
    let binding = send(
        &app,
        json_request(
            "PUT",
            "/api/v1/effects/active/controls/speed/binding",
            &json!({
                "sensor": "cpu.load",
                "sensor_min": 0.0,
                "sensor_max": 100.0,
                "target_min": 0.0,
                "target_max": 100.0
            }),
        ),
    )
    .await;
    assert_reached_v1_handler(binding).await;

    // `reset` accepts an absent body, which is part of its v1 contract.
    let reset = send(&app, empty_request("POST", "/api/v1/effects/active/reset")).await;
    assert_eq!(reset.status(), StatusCode::OK);
    assert_envelope(&body_json(reset).await);
}

/// Route deletions leave nothing behind. A rename or a merge that left
/// an alias or a redirect would still serve the old behavior here, so
/// this asserts the absence rather than the new routes' presence.
///
/// Two waves feed this list: C1b's renames, and wave 78.2's merge of
/// power, brightness, and pause/resume onto one `/output` resource.
///
/// Most retired paths match no route at all and answer 404. The one
/// exception is pinned deliberately: `/effects/current/controls` now
/// falls through to the live `/effects/{id}/controls` sibling with `id`
/// bound to the literal `current`, which fails UUID parsing and answers
/// `400 malformed_request`. That is still a deletion — no handler treats
/// `current` as the active effect any more — and pinning the specific
/// status is what would catch someone re-adding one that does.
#[tokio::test]
async fn renamed_routes_leave_nothing_behind() {
    let (state, _tmp) = isolated_state();
    let app = test_app(&state);
    let zone_id = primary_zone_id(&app).await;

    let retired = [
        (
            json_request(
                "PATCH",
                "/api/v1/effects/current/controls",
                &json!({ "controls": { "speed": 0.5 } }),
            ),
            StatusCode::BAD_REQUEST,
        ),
        (
            json_request(
                "PUT",
                "/api/v1/effects/current/controls/speed/binding",
                &json!({ "sensor": "audio.level" }),
            ),
            StatusCode::NOT_FOUND,
        ),
        (
            empty_request("POST", "/api/v1/effects/current/reset"),
            StatusCode::NOT_FOUND,
        ),
        (
            get(&format!("/api/v1/scenes/default/groups/{zone_id}/layers")),
            StatusCode::NOT_FOUND,
        ),
        (
            json_request(
                "POST",
                "/api/v1/config/get",
                &json!({ "key": "daemon.port" }),
            ),
            StatusCode::NOT_FOUND,
        ),
        (
            json_request(
                "POST",
                "/api/v1/config/set",
                &json!({ "key": "daemon.port", "value": 9420 }),
            ),
            StatusCode::NOT_FOUND,
        ),
        // Spec 78 wave 78.2 merged these four onto `/output` and moved
        // audio devices under `/system`. Same deletion discipline.
        (get("/api/v1/output/power"), StatusCode::NOT_FOUND),
        (
            json_request("PUT", "/api/v1/output/power", &json!({ "state": "paused" })),
            StatusCode::NOT_FOUND,
        ),
        (get("/api/v1/settings/brightness"), StatusCode::NOT_FOUND),
        (
            json_request(
                "PUT",
                "/api/v1/settings/brightness",
                &json!({ "brightness": 42 }),
            ),
            StatusCode::NOT_FOUND,
        ),
        // Pause and resume fall through to the GET-only
        // `/effects/{id}` sibling, so a POST answers 405. Still a
        // deletion: no handler treats `pause` as an output verb.
        (
            empty_request("POST", "/api/v1/effects/pause"),
            StatusCode::METHOD_NOT_ALLOWED,
        ),
        (
            empty_request("POST", "/api/v1/effects/resume"),
            StatusCode::METHOD_NOT_ALLOWED,
        ),
        (get("/api/v1/audio/devices"), StatusCode::NOT_FOUND),
    ];

    for (request, expected) in retired {
        let uri = request.uri().to_string();
        let response = send(&app, request).await;
        assert_eq!(
            response.status(),
            expected,
            "{uri} must be gone, not aliased or redirected"
        );
        // A status alone would pass against a bare framework 404 or an
        // SPA page serving the app shell. The envelope is what proves
        // the API itself answered.
        if expected == StatusCode::NOT_FOUND {
            let body = body_json(response).await;
            assert_error_envelope(&body, "not_found", DetailsPresence::Absent);
            assert_eq!(
                body["error"]["message"],
                format!("route not found: {uri}"),
                "{uri} should name the address the caller asked for"
            );
        }
    }
}

#[tokio::test]
async fn scene_zone_layer_paths_stay_routed() {
    let (state, _tmp) = isolated_state();
    let app = test_app(&state);
    let zone_id = primary_zone_id(&app).await;

    // Layer stacks and zone CRUD address the same object through the same
    // `/zones/` segment.
    let base = format!("/api/v1/scenes/default/zones/{zone_id}/layers");
    let listed = send(&app, get(&base)).await;
    assert_eq!(listed.status(), StatusCode::OK);
    assert_eq!(etag(&listed), "\"0\"");
    let json = body_json(listed).await;
    assert_envelope(&json);
    assert!(
        json["data"]["items"].is_array(),
        "layer stack body carries an items array"
    );
    assert_eq!(json["data"]["layers_version"], json!(0));

    let created = send(
        &app,
        if_match(
            json_request(
                "POST",
                &base,
                &json!({
                    "source": { "type": "color_fill", "rgba": [1.0, 0.0, 0.5, 1.0] },
                    "blend": "alpha",
                    "opacity": 0.75
                }),
            ),
            "\"0\"",
        ),
    )
    .await;
    assert_eq!(created.status(), StatusCode::CREATED);
    assert_eq!(etag(&created), "\"1\"");
    let created_json = body_json(created).await;
    let layer_id = created_json["data"]["items"]
        .as_array()
        .expect("items array")
        .last()
        .and_then(|layer| layer["id"].as_str())
        .expect("created layer should carry an id")
        .to_owned();

    let reordered = send(
        &app,
        json_request(
            "PATCH",
            &format!("{base}/order"),
            &json!({ "layer_ids": [layer_id.clone()] }),
        ),
    )
    .await;
    assert_reached_v1_handler(reordered).await;

    let controls = send(
        &app,
        json_request(
            "PATCH",
            &format!("{base}/{layer_id}/controls"),
            &json!({ "controls": {} }),
        ),
    )
    .await;
    assert_reached_v1_handler(controls).await;

    let deleted = send(&app, empty_request("DELETE", &format!("{base}/{layer_id}"))).await;
    assert_eq!(deleted.status(), StatusCode::OK);
}

#[tokio::test]
async fn config_key_reads_keep_their_key_value_body() {
    let (state, _tmp) = isolated_state();
    let app = test_app(&state);

    let response = send(&app, get("/api/v1/config/keys/daemon.port")).await;

    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_envelope(&json);
    assert_keys(&json["data"], &["key", "value"], "config key read body");
    assert_eq!(json["data"]["key"], json!("daemon.port"));
}

#[tokio::test]
async fn config_key_reads_report_unknown_keys_through_the_error_envelope() {
    let (state, _tmp) = isolated_state();
    let app = test_app(&state);

    let response = send(&app, get("/api/v1/config/keys/nope.not.a.key")).await;

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_error_envelope(
        &body_json(response).await,
        "not_found",
        DetailsPresence::Absent,
    );
}

#[tokio::test]
async fn config_schema_is_served_as_a_list_of_entries() {
    let (state, _tmp) = isolated_state();
    let app = test_app(&state);

    let response = send(&app, get("/api/v1/config/schema")).await;

    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_envelope(&json);
    let entries = json["data"].as_array().expect("schema is a list");
    assert_keys(
        &entries[0],
        &["pattern", "apply", "redaction", "has_validator"],
        "config schema entry",
    );
}

#[tokio::test]
async fn config_key_writes_take_the_value_as_the_body() {
    let (state, _tmp) = isolated_state();
    let app = test_app(&state);

    // Without a live `ConfigManager` this state cannot persist, and that
    // case answers 500 through the standard error envelope rather than a
    // routing failure.
    let response = send(
        &app,
        json_request("PUT", "/api/v1/config/keys/daemon.port", &json!(9420)),
    )
    .await;

    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert_error_envelope(
        &body_json(response).await,
        "internal_error",
        DetailsPresence::Absent,
    );
}

#[tokio::test]
async fn config_key_deletes_reset_one_key() {
    let (state, _tmp) = isolated_state();
    let app = test_app(&state);

    let response = send(
        &app,
        empty_request("DELETE", "/api/v1/config/keys/daemon.port"),
    )
    .await;

    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert_error_envelope(
        &body_json(response).await,
        "internal_error",
        DetailsPresence::Absent,
    );
}

// ── Error shapes ─────────────────────────────────────────────────────────

#[tokio::test]
async fn error_envelope_omits_details_when_the_error_carries_none() {
    let (state, _tmp) = isolated_state();
    let app = test_app(&state);

    let response = send(
        &app,
        get("/api/v1/scenes/00000000-0000-0000-0000-000000000001"),
    )
    .await;

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let json = body_json(response).await;
    assert_error_envelope(&json, "not_found", DetailsPresence::Absent);
    assert_eq!(
        json["error"]["details"],
        Value::Null,
        "serde_json reads an absent key as Null; the key-set assertion above is \
         what proves it was never serialized"
    );
    assert_eq!(
        json["error"]["message"],
        json!("scene not found: 00000000-0000-0000-0000-000000000001"),
        "not-found prose is derived from the resource kind, not hand-written per route"
    );
}

#[tokio::test]
async fn zone_precondition_failure_renders_the_canonical_412() {
    let (state, _tmp) = isolated_state();
    let app = test_app(&state);

    let listed = send(&app, get("/api/v1/scenes/default/zones")).await;
    assert_eq!(listed.status(), StatusCode::OK);
    assert_eq!(etag(&listed), "\"0\"");

    let created = send(
        &app,
        if_match(
            json_request(
                "POST",
                "/api/v1/scenes/default/zones",
                &json!({ "name": "Desk" }),
            ),
            "\"0\"",
        ),
    )
    .await;
    assert_eq!(created.status(), StatusCode::CREATED);
    assert_eq!(etag(&created), "\"1\"");

    // Replaying the now-stale precondition trips the 412.
    let stale = send(
        &app,
        if_match(
            json_request(
                "POST",
                "/api/v1/scenes/default/zones",
                &json!({ "name": "Shelf" }),
            ),
            "\"0\"",
        ),
    )
    .await;

    assert_eq!(stale.status(), StatusCode::PRECONDITION_FAILED);
    assert_eq!(
        etag(&stale),
        "\"1\"",
        "the 412 itself carries the current ETag so clients can rebase"
    );
    assert_precondition_body(&body_json(stale).await, 0, 1);
}

#[tokio::test]
async fn layer_precondition_failure_renders_the_canonical_412() {
    let (state, _tmp) = isolated_state();
    let app = test_app(&state);
    let zone_id = primary_zone_id(&app).await;
    let base = format!("/api/v1/scenes/default/zones/{zone_id}/layers");
    let body = json!({
        "source": { "type": "color_fill", "rgba": [1.0, 0.0, 0.5, 1.0] },
        "blend": "alpha",
        "opacity": 0.75
    });

    let created = send(&app, if_match(json_request("POST", &base, &body), "\"0\"")).await;
    assert_eq!(created.status(), StatusCode::CREATED);

    let stale = send(&app, if_match(json_request("POST", &base, &body), "\"0\"")).await;

    assert_eq!(stale.status(), StatusCode::PRECONDITION_FAILED);
    assert_eq!(etag(&stale), "\"1\"");
    assert_precondition_body(&body_json(stale).await, 0, 1);
}

#[tokio::test]
async fn controls_precondition_failure_renders_the_canonical_412() {
    let (state, _tmp) = isolated_state();
    register_effect(&state, "solid_color").await;
    let app = test_app(&state);
    let applied = send(
        &app,
        empty_request("POST", "/api/v1/effects/solid_color/apply"),
    )
    .await;
    assert_eq!(applied.status(), StatusCode::OK);

    // Applying an effect already advances `controls_version`, so the starting
    // value is a property of the state machine rather than a wire constant.
    // What is frozen is that the ETag and the body always agree on it.
    let active = send(&app, get("/api/v1/effects/active")).await;
    assert_eq!(active.status(), StatusCode::OK);
    let active_etag = etag(&active);
    let active_json = body_json(active).await;
    let effect_id = active_json["data"]["id"]
        .as_str()
        .expect("active effect should carry an id")
        .to_owned();
    let version = active_json["data"]["controls_version"]
        .as_u64()
        .expect("active effect should expose controls_version");
    assert_eq!(
        active_etag,
        format!("\"{version}\""),
        "GET /effects/active mirrors controls_version into the ETag"
    );

    let uri = format!("/api/v1/effects/{effect_id}/controls");
    let patch_body = json!({ "controls": { "speed": 3.0 } });
    let stale_precondition = format!("\"{version}\"");
    let next = version + 1;

    let ok = send(
        &app,
        if_match(
            json_request("PATCH", &uri, &patch_body),
            &stale_precondition,
        ),
    )
    .await;
    assert_eq!(ok.status(), StatusCode::OK);
    assert_eq!(etag(&ok), format!("\"{next}\""));
    assert_eq!(body_json(ok).await["data"]["controls_version"], json!(next));

    let stale = send(
        &app,
        if_match(
            json_request("PATCH", &uri, &patch_body),
            &stale_precondition,
        ),
    )
    .await;

    assert_eq!(stale.status(), StatusCode::PRECONDITION_FAILED);
    assert_eq!(etag(&stale), format!("\"{next}\""));
    assert_precondition_body(&body_json(stale).await, version, next);
}

#[tokio::test]
async fn if_match_parsing_quirks_are_frozen() {
    let (state, _tmp) = isolated_state();
    let app = test_app(&state);
    let uri = "/api/v1/scenes/default/zones";

    // A bare, unquoted integer is accepted alongside the RFC 7232 quoted form.
    let bare = send(
        &app,
        if_match(json_request("POST", uri, &json!({ "name": "Bare" })), "0"),
    )
    .await;
    assert_eq!(bare.status(), StatusCode::CREATED);

    // `*` means "no precondition" rather than "any existing resource".
    let wildcard = send(
        &app,
        if_match(
            json_request("POST", uri, &json!({ "name": "Wildcard" })),
            "*",
        ),
    )
    .await;
    assert_eq!(wildcard.status(), StatusCode::CREATED);

    // A weak validator is rejected outright: `W/` survives the quote trim and
    // then fails the integer parse. Parsing is unchanged; only the rendering
    // is canonical, and an unreadable header value is a 400, not a 422.
    let weak = send(
        &app,
        if_match(
            json_request("POST", uri, &json!({ "name": "Weak" })),
            "W/\"2\"",
        ),
    )
    .await;
    assert_eq!(weak.status(), StatusCode::BAD_REQUEST);
    assert_error_envelope(
        &body_json(weak).await,
        "malformed_request",
        DetailsPresence::Absent,
    );
}

#[tokio::test]
async fn rejected_websocket_origin_serves_the_canonical_forbidden_envelope() {
    let (state, _tmp) = isolated_state();
    let addr = spawn_server(&state).await;

    // `tower::oneshot` cannot reach this handler: the `WebSocketUpgrade`
    // extractor answers first without a live hyper connection, so the origin
    // check needs real I/O to exercise.
    let response = reqwest::Client::new()
        .get(format!("http://{addr}/api/v1/ws"))
        .header(http::header::CONNECTION, "Upgrade")
        .header(http::header::UPGRADE, "websocket")
        .header("sec-websocket-version", "13")
        .header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==")
        .header(http::header::ORIGIN, "https://evil.example")
        .send()
        .await
        .expect("upgrade request should reach the daemon");

    assert_eq!(response.status().as_u16(), StatusCode::FORBIDDEN.as_u16());
    let json: Value = response
        .json()
        .await
        .expect("the origin rejection carries the canonical envelope");
    assert_error_envelope(&json, "forbidden", DetailsPresence::Absent);
}
