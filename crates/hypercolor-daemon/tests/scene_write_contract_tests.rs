//! Contract pins for the scene write fence that Studio's live commits
//! lean on (Design 73 D2).
//!
//! A read that serves contents beside a revision must serve the revision
//! those contents were committed at, or a fenced client writes stale
//! fields straight past the fence. A fence from an earlier daemon run must
//! never pass in a later one, so the fence names its run, and every answer
//! names the run it came from so a client knows when to re-anchor.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::body::Body;
use http::{Request, StatusCode};
use hypercolor_core::effect::EffectEntry;
use hypercolor_daemon::api;
use hypercolor_daemon::app_state::{AppState, AppStateBuilder};
use hypercolor_daemon::domain::scene_run::{CLOCK_BASE_CEILING, SceneRun};
use hypercolor_types::api::envelope::{
    IDEMPOTENCY_KEY_HEADER, IDEMPOTENCY_KEY_MISMATCH_CODE, IDEMPOTENCY_REPLAYED_HEADER,
    RUN_EPOCH_HEADER,
};
use hypercolor_types::api::scene::SceneFence;
use hypercolor_types::effect::{
    EffectCategory, EffectId, EffectMetadata, EffectSource, EffectState, PresetTemplate,
};
use hypercolor_types::library::PresetId;
use serde_json::json;
use tower::ServiceExt;

async fn send(app: &axum::Router, request: Request<Body>) -> axum::response::Response {
    app.clone()
        .oneshot(request)
        .await
        .expect("request should succeed")
}

async fn body_json(response: axum::response::Response) -> serde_json::Value {
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body should read");
    serde_json::from_slice(&bytes).expect("response body should be JSON")
}

fn get(uri: impl Into<String>) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri(uri.into())
        .body(Body::empty())
        .expect("request should build")
}

fn patch_zone(zone_id: &str, name: &str, revision: u64) -> Request<Body> {
    Request::builder()
        .method("PATCH")
        .uri(format!("/api/v1/scene/zones/{zone_id}"))
        .header("content-type", "application/json")
        .header(http::header::IF_MATCH, format!("\"{revision}\""))
        .body(Body::from(json!({ "name": name }).to_string()))
        .expect("request should build")
}

fn etag_revision(response: &axum::response::Response) -> u64 {
    response
        .headers()
        .get(http::header::ETAG)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| hypercolor_types::api::scene::SceneFence::parse(value).ok())
        .map(|fence| fence.revision)
        .expect("response should carry the scene revision as ETag")
}

fn primary_zone(document: &serde_json::Value) -> &serde_json::Value {
    document["data"]["zones"]
        .as_array()
        .expect("zones array")
        .iter()
        .find(|zone| zone["role"] == "primary")
        .expect("the live tree always carries a primary zone")
}

/// The name a zone carries when the writer below committed it.
fn name_at(revision: u64) -> String {
    format!("rev-{revision}")
}

/// Reads racing a stream of commits. The split this guards against is a
/// window of a few instructions, so this is the end-to-end check that the
/// served pairs agree; the deterministic proof that a paired read excludes
/// commits sits beside `SceneService` itself.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn scene_reads_pair_contents_with_the_revision_that_committed_them() {
    let state = Arc::new(AppState::new());
    let app = api::build_router(Arc::clone(&state), None);

    let seeded = send(&app, get("/api/v1/scene")).await;
    let mut revision = etag_revision(&seeded);
    let document = body_json(seeded).await;
    let zone_id = primary_zone(&document)["id"]
        .as_str()
        .expect("zone id")
        .to_owned();

    // Name the zone after the revision it lands at, so every later read
    // can check that its contents and its revision came from one commit.
    let named = send(&app, patch_zone(&zone_id, &name_at(revision + 1), revision)).await;
    assert_eq!(named.status(), StatusCode::OK);
    revision = etag_revision(&named);
    let start = revision;

    let writer = {
        let app = app.clone();
        let zone_id = zone_id.clone();
        tokio::spawn(async move {
            for _ in 0..200 {
                let next = revision + 1;
                let response = send(&app, patch_zone(&zone_id, &name_at(next), revision)).await;
                assert_eq!(response.status(), StatusCode::OK);
                assert_eq!(
                    etag_revision(&response),
                    next,
                    "the writer is the only committer, so each fenced rename lands one revision on"
                );
                revision = next;
            }
            revision
        })
    };

    let mut reads = 0_u32;
    while !writer.is_finished() {
        let response = send(&app, get("/api/v1/scene")).await;
        let served = etag_revision(&response);
        let document = body_json(response).await;
        let body_revision = document["data"]["revision"].as_u64().expect("revision");
        assert_eq!(served, body_revision, "ETag and body name one revision");
        assert!(body_revision >= start);
        assert_eq!(
            primary_zone(&document)["name"],
            name_at(body_revision).as_str(),
            "GET /scene paired revision {body_revision} with another commit's contents"
        );

        let response = send(&app, get(format!("/api/v1/scene/zones/{zone_id}"))).await;
        let served = etag_revision(&response);
        let zone = body_json(response).await;
        assert_eq!(
            zone["data"]["name"],
            name_at(served).as_str(),
            "GET /scene/zones/{{zone}} paired revision {served} with another commit's contents"
        );
        reads += 1;
    }

    let last = writer.await.expect("writer task should finish");
    assert_eq!(last, start + 200);
    assert!(reads > 0, "the reader must overlap the writer");
}

// ── Revisions across daemon runs ─────────────────────────────────────────

fn run_epoch(response: &axum::response::Response) -> String {
    response
        .headers()
        .get(RUN_EPOCH_HEADER)
        .and_then(|value| value.to_str().ok())
        .expect("every API response names the daemon run that answered")
        .to_owned()
}

/// One daemon run over `data_dir`, as a restart would build it.
fn daemon_run(data_dir: &Path) -> (Arc<AppState>, axum::Router) {
    let state = Arc::new(AppStateBuilder::new(data_dir.to_path_buf()).build());
    let app = api::build_router(Arc::clone(&state), None);
    (state, app)
}

/// Read the live document's revision, run epoch, and primary zone id.
async fn live_scene(app: &axum::Router) -> (u64, String, String) {
    let response = send(app, get("/api/v1/scene")).await;
    assert_eq!(response.status(), StatusCode::OK);
    let header_epoch = run_epoch(&response);
    let document = body_json(response).await;
    assert_eq!(
        document["data"]["run_epoch"],
        header_epoch.as_str(),
        "the document and its response name one run"
    );
    let revision = document["data"]["revision"].as_u64().expect("revision");
    let zone_id = primary_zone(&document)["id"]
        .as_str()
        .expect("zone id")
        .to_owned();
    (revision, header_epoch, zone_id)
}

#[tokio::test]
async fn a_fence_from_a_previous_daemon_run_never_passes() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let data_dir = tmp.path().join("data");

    let (first_revision, first_epoch, last_revision) = {
        let (_state, app) = daemon_run(&data_dir);
        let (start, epoch, zone_id) = live_scene(&app).await;
        let mut revision = start;
        for step in 0..3 {
            let response = send(
                &app,
                patch_zone(&zone_id, &format!("first-{step}"), revision),
            )
            .await;
            assert_eq!(response.status(), StatusCode::OK);
            revision = etag_revision(&response);
        }
        (start, epoch, revision)
    };

    let (_state, app) = daemon_run(&data_dir);
    let (restarted, epoch, restarted_zone) = live_scene(&app).await;
    assert_ne!(epoch, first_epoch, "every daemon start is a new run");
    assert!(
        restarted > last_revision,
        "the new run starts above every revision the old one handed out \
         ({restarted} <= {last_revision})"
    );

    for stale in [first_revision, last_revision] {
        let response = send(&app, patch_zone(&restarted_zone, "stale", stale)).await;
        assert_eq!(
            response.status(),
            StatusCode::PRECONDITION_FAILED,
            "a fence at {stale} from the previous run must not pass"
        );
        assert_eq!(etag_revision(&response), restarted);
        assert_eq!(
            run_epoch(&response),
            epoch,
            "a refused fence names the run too"
        );
    }

    // The epoch-qualified fence is safe whatever the revisions do: one that
    // names the previous run is stale even at the new run's own revision.
    let numeric_match = SceneFence::new(first_epoch.as_str(), restarted).to_etag();
    let response = send(
        &app,
        patch_zone_fenced(&restarted_zone, "stale", &numeric_match),
    )
    .await;
    assert_eq!(response.status(), StatusCode::PRECONDITION_FAILED);
}

#[test]
fn a_run_starts_at_the_wall_clock_under_a_fresh_epoch() {
    let now = UNIX_EPOCH + Duration::from_hours(500_000);
    let first = SceneRun::from_clock_at(now);
    let second = SceneRun::from_clock_at(now);
    assert_eq!(first.start_revision(), 1_800_000_000 * 1_000_000);
    assert_ne!(
        first.epoch(),
        second.epoch(),
        "two runs at the same instant are still two runs"
    );
    assert_eq!(
        SceneRun::from_clock_at(UNIX_EPOCH).start_revision(),
        0,
        "a clock at the epoch contributes nothing"
    );
}

#[test]
fn the_clock_term_stays_exact_as_a_json_double() {
    // Year 2286: microseconds since the epoch pass 2^53 there.
    let far_future = UNIX_EPOCH + Duration::from_secs(10_000_000_000);
    let run = SceneRun::from_clock_at(far_future);
    assert_eq!(run.start_revision(), CLOCK_BASE_CEILING);
}

#[test]
fn an_ephemeral_run_starts_at_zero_under_its_own_epoch() {
    let first = SceneRun::ephemeral();
    let second = SceneRun::ephemeral();
    assert_eq!(first.start_revision(), 0);
    assert!(!first.epoch().is_empty());
    assert_ne!(first.epoch(), second.epoch());
}

#[tokio::test]
async fn every_api_answer_names_the_run_that_gave_it() {
    let state = Arc::new(AppState::new());
    let app = api::build_router(Arc::clone(&state), None);
    let epoch = state.scene_manager.run_epoch().to_owned();

    let (revision, served, zone_id) = live_scene(&app).await;
    assert_eq!(served, epoch);

    let written = send(&app, patch_zone(&zone_id, "renamed", revision)).await;
    assert_eq!(written.status(), StatusCode::OK);
    assert_eq!(
        run_epoch(&written),
        epoch,
        "zone write replies name the run"
    );

    let refused = send(&app, patch_zone(&zone_id, "stale", revision)).await;
    assert_eq!(refused.status(), StatusCode::PRECONDITION_FAILED);
    assert_eq!(run_epoch(&refused), epoch, "a refused fence names the run");

    let missing = send(&app, get("/api/v1/no-such-route")).await;
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    assert_eq!(run_epoch(&missing), epoch);

    let document = send(&app, get("/api/v1/openapi.json")).await;
    assert_eq!(
        run_epoch(&document),
        epoch,
        "routes merged beside the API name the run too"
    );
}

// ── Idempotency-Key ──────────────────────────────────────────────────────

fn keyed(mut request: Request<Body>, key: &str) -> Request<Body> {
    request.headers_mut().insert(
        IDEMPOTENCY_KEY_HEADER,
        http::HeaderValue::from_str(key).expect("valid key"),
    );
    request
}

fn json_request(method: &str, uri: &str, body: &serde_json::Value) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .expect("request should build")
}

fn fenced(mut request: Request<Body>, revision: u64) -> Request<Body> {
    request.headers_mut().insert(
        http::header::IF_MATCH,
        http::HeaderValue::from_str(&format!("\"{revision}\"")).expect("valid fence"),
    );
    request
}

fn replayed(response: &axum::response::Response) -> bool {
    response
        .headers()
        .get(IDEMPOTENCY_REPLAYED_HEADER)
        .is_some_and(|value| value == "true")
}

fn create_zone(name: &str, revision: u64) -> Request<Body> {
    fenced(
        json_request("POST", "/api/v1/scene/zones", &json!({ "name": name })),
        revision,
    )
}

async fn zone_names(app: &axum::Router) -> Vec<String> {
    let document = body_json(send(app, get("/api/v1/scene")).await).await;
    document["data"]["zones"]
        .as_array()
        .expect("zones")
        .iter()
        .map(|zone| zone["name"].as_str().expect("name").to_owned())
        .collect()
}

fn isolated() -> (Arc<AppState>, axum::Router) {
    let state = Arc::new(AppState::new());
    let app = api::build_router(Arc::clone(&state), None);
    (state, app)
}

#[tokio::test]
async fn a_retried_zone_create_replays_instead_of_creating_twice() {
    let (state, app) = isolated();
    let revision = state.scene_manager.revision();
    let key = "6f9b0b1e-2c4d-4d7e-9a0b-1c2d3e4f5a6b";

    let created = send(&app, keyed(create_zone("Desk", revision), key)).await;
    assert_eq!(created.status(), StatusCode::CREATED);
    assert!(
        !replayed(&created),
        "the first attempt is applied, not replayed"
    );
    let landed = etag_revision(&created);
    let original = body_json(created).await;

    let retried = send(&app, keyed(create_zone("Desk", revision), key)).await;
    assert_eq!(retried.status(), StatusCode::CREATED);
    assert!(replayed(&retried));
    assert_eq!(etag_revision(&retried), landed);
    assert_eq!(
        body_json(retried).await,
        original,
        "the stored answer, byte for byte"
    );

    assert_eq!(
        state.scene_manager.revision(),
        landed,
        "nothing applied twice"
    );
    assert_eq!(
        zone_names(&app)
            .await
            .iter()
            .filter(|name| *name == "Desk")
            .count(),
        1
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_duplicates_apply_once() {
    let (state, app) = isolated();
    let (revision, _, zone_id) = live_scene(&app).await;
    let key = "5d3c1a7e-0000-4000-8000-00000000c0de";

    let (left, right) = tokio::join!(
        send(&app, keyed(patch_zone(&zone_id, "Twice?", revision), key)),
        send(&app, keyed(patch_zone(&zone_id, "Twice?", revision), key)),
    );
    assert_eq!(left.status(), StatusCode::OK);
    assert_eq!(right.status(), StatusCode::OK);
    assert_eq!(
        u8::from(replayed(&left)) + u8::from(replayed(&right)),
        1,
        "one attempt applies and its duplicate replays it"
    );
    assert_eq!(etag_revision(&left), revision + 1);
    assert_eq!(etag_revision(&right), revision + 1);
    assert_eq!(body_json(left).await, body_json(right).await);
    assert_eq!(state.scene_manager.revision(), revision + 1);
}

#[tokio::test]
async fn a_key_reused_for_a_different_request_is_refused() {
    let (state, app) = isolated();
    let (revision, _, zone_id) = live_scene(&app).await;
    let key = "reused-key";

    let first = send(&app, keyed(patch_zone(&zone_id, "First", revision), key)).await;
    assert_eq!(first.status(), StatusCode::OK);
    let landed = etag_revision(&first);

    for different in [
        patch_zone(&zone_id, "Second", revision),
        patch_zone(&zone_id, "First", landed),
    ] {
        let refused = send(&app, keyed(different, key)).await;
        assert_eq!(refused.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let body = body_json(refused).await;
        assert_eq!(body["error"]["code"], IDEMPOTENCY_KEY_MISMATCH_CODE);
    }
    assert_eq!(state.scene_manager.revision(), landed);
    let zone = body_json(send(&app, get(format!("/api/v1/scene/zones/{zone_id}"))).await).await;
    assert_eq!(zone["data"]["name"], "First");
}

#[tokio::test]
async fn a_refused_fence_is_a_definitive_answer_too() {
    let (_state, app) = isolated();
    let (revision, _, zone_id) = live_scene(&app).await;
    let moved = send(&app, patch_zone(&zone_id, "Moved on", revision)).await;
    assert_eq!(moved.status(), StatusCode::OK);

    let key = "stale-fence";
    let refused = send(&app, keyed(patch_zone(&zone_id, "Stale", revision), key)).await;
    assert_eq!(refused.status(), StatusCode::PRECONDITION_FAILED);
    let retried = send(&app, keyed(patch_zone(&zone_id, "Stale", revision), key)).await;
    assert_eq!(retried.status(), StatusCode::PRECONDITION_FAILED);
    assert!(replayed(&retried));
}

#[tokio::test]
async fn a_retried_delete_answers_with_the_original_document() {
    let (state, app) = isolated();
    let revision = state.scene_manager.revision();
    let created = send(&app, create_zone("Doomed", revision)).await;
    assert_eq!(created.status(), StatusCode::CREATED);
    let landed = etag_revision(&created);
    let zone_id = body_json(created).await["data"]["id"]
        .as_str()
        .expect("zone id")
        .to_owned();

    let delete = || {
        fenced(
            Request::builder()
                .method("DELETE")
                .uri(format!("/api/v1/scene/zones/{zone_id}"))
                .body(Body::empty())
                .expect("request should build"),
            landed,
        )
    };
    let key = "lost-delete";
    let deleted = send(&app, keyed(delete(), key)).await;
    assert_eq!(deleted.status(), StatusCode::OK);
    let original = body_json(deleted).await;

    // Without the key this retry would 404 or 412 and leave the client
    // guessing whether its delete landed.
    let retried = send(&app, keyed(delete(), key)).await;
    assert_eq!(retried.status(), StatusCode::OK);
    assert!(replayed(&retried));
    assert_eq!(body_json(retried).await, original);
}

#[tokio::test]
async fn requests_without_a_key_behave_as_before() {
    let (state, app) = isolated();
    let start = state.scene_manager.revision();
    for _ in 0..2 {
        let created = send(
            &app,
            json_request("POST", "/api/v1/scene/zones", &json!({ "name": "Twin" })),
        )
        .await;
        assert_eq!(created.status(), StatusCode::CREATED);
        assert!(!replayed(&created));
    }
    assert_eq!(state.scene_manager.revision(), start + 2);
    assert_eq!(
        zone_names(&app)
            .await
            .iter()
            .filter(|name| *name == "Twin")
            .count(),
        2,
        "unkeyed writes apply every time"
    );
}

#[tokio::test]
async fn a_malformed_key_is_refused_before_anything_applies() {
    let (state, app) = isolated();
    let revision = state.scene_manager.revision();
    let refused = send(&app, keyed(create_zone("Never", revision), "two words")).await;
    assert_eq!(refused.status(), StatusCode::BAD_REQUEST);
    assert_eq!(state.scene_manager.revision(), revision);
}

#[test]
fn every_scene_tree_write_documents_the_key() {
    let document: serde_json::Value = serde_json::from_str(
        &api::openapi::document_json_pretty().expect("OpenAPI document renders"),
    )
    .expect("OpenAPI document parses");
    let mut documented = Vec::new();
    for (path, item) in document["paths"].as_object().expect("paths") {
        for (method, operation) in item.as_object().expect("path item") {
            let declares = operation["parameters"]
                .as_array()
                .is_some_and(|parameters| {
                    parameters.iter().any(|parameter| {
                        parameter["in"] == "header"
                            && parameter["name"].as_str().is_some_and(|name| {
                                name.eq_ignore_ascii_case(IDEMPOTENCY_KEY_HEADER)
                            })
                            && parameter["required"] != true
                    })
                });
            if declares {
                documented.push(format!("{} {path}", method.to_ascii_uppercase()));
            }
        }
    }
    documented.sort();
    let mut expected = vec![
        "PATCH /api/v1/scene",
        "POST /api/v1/scene/clear",
        "POST /api/v1/scene/members/edit",
        "POST /api/v1/scene/zones",
        "PATCH /api/v1/scene/zones/{zone}",
        "DELETE /api/v1/scene/zones/{zone}",
        "PUT /api/v1/scene/zones/{zone}/layout",
        "POST /api/v1/scene/zones/{zone}/members",
        "DELETE /api/v1/scene/zones/{zone}/members/{member}",
        "POST /api/v1/scene/zones/{zone}/layers",
        "PATCH /api/v1/scene/zones/{zone}/layers/order",
        "PUT /api/v1/scene/zones/{zone}/layers/{layer}",
        "DELETE /api/v1/scene/zones/{zone}/layers/{layer}",
    ];
    expected.sort_unstable();
    assert_eq!(documented, expected);
}

#[tokio::test]
async fn an_oversized_keyed_body_is_refused_without_burning_the_key() {
    let (state, app) = isolated();
    let revision = state.scene_manager.revision();
    let key = "big-body";
    let oversized = json!({ "name": "x".repeat(3 * 1024 * 1024) });
    let refused = send(
        &app,
        keyed(
            fenced(
                json_request("POST", "/api/v1/scene/zones", &oversized),
                revision,
            ),
            key,
        ),
    )
    .await;
    assert_eq!(refused.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(state.scene_manager.revision(), revision);

    let created = send(&app, keyed(create_zone("Fits", revision), key)).await;
    assert_eq!(
        created.status(),
        StatusCode::CREATED,
        "a body the daemon never read claims nothing"
    );
    assert!(!replayed(&created));
}

/// Every documented operation, with its path parameters filled, answers a
/// keyed repeat from the store. Most of these answer 404 or 422 for the
/// made-up targets and bodies; a refusal replays like any other answer,
/// which is what proves the router matched each template.
#[tokio::test]
async fn every_documented_route_replays_a_keyed_repeat() {
    let (_state, app) = isolated();
    let document: serde_json::Value = serde_json::from_str(
        &api::openapi::document_json_pretty().expect("OpenAPI document renders"),
    )
    .expect("OpenAPI document parses");
    let mut swept = 0;
    for (path, item) in document["paths"].as_object().expect("paths") {
        for (method, operation) in item.as_object().expect("path item") {
            let declares = operation["parameters"]
                .as_array()
                .is_some_and(|parameters| {
                    parameters
                        .iter()
                        .any(|parameter| parameter["name"] == "Idempotency-Key")
                });
            if !declares {
                continue;
            }
            let uri = path
                .replace("{zone}", "0198c5b6-1111-7000-8000-00000000aaaa")
                .replace("{layer}", "0198c5b6-1111-7000-8000-00000000bbbb")
                .replace("{member}", "no-such-member");
            let key = format!("sweep-{swept}");
            let attempt = || {
                keyed(
                    json_request(&method.to_ascii_uppercase(), &uri, &json!({})),
                    &key,
                )
            };
            let first = send(&app, attempt()).await;
            assert!(
                !replayed(&first),
                "{method} {path}: the first attempt is live"
            );
            let status = first.status();
            let repeat = send(&app, attempt()).await;
            assert!(replayed(&repeat), "{method} {path} did not honor the key");
            assert_eq!(repeat.status(), status, "{method} {path}");
            swept += 1;
        }
    }
    assert_eq!(swept, 13);
}

/// `POST /scene/clear` reads its optional JSON body only with a JSON
/// content type, so the same bytes without one are a different request
/// (a whole-scene clear) and must never replay the zone-only answer.
#[tokio::test]
async fn the_same_bytes_under_another_content_type_are_another_request() {
    let (_state, app) = isolated();
    let (revision, _, zone_id) = live_scene(&app).await;
    let body = json!({ "zone": zone_id }).to_string();
    let clear = |content_type: Option<&str>| {
        let mut builder = Request::builder()
            .method("POST")
            .uri("/api/v1/scene/clear")
            .header(http::header::IF_MATCH, format!("\"{revision}\""));
        if let Some(content_type) = content_type {
            builder = builder.header("content-type", content_type);
        }
        keyed(
            builder
                .body(Body::from(body.clone()))
                .expect("request should build"),
            "clear-key",
        )
    };

    let zone_only = send(&app, clear(Some("application/json"))).await;
    assert_eq!(zone_only.status(), StatusCode::OK);
    let without_type = send(&app, clear(None)).await;
    assert_eq!(without_type.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        body_json(without_type).await["error"]["code"],
        IDEMPOTENCY_KEY_MISMATCH_CODE
    );
    let repeat = send(&app, clear(Some("application/json"))).await;
    assert!(replayed(&repeat), "the original request still replays");
}

// ── Epoch-qualified fences ───────────────────────────────────────────────

/// A run epoch no daemon run in these tests holds: well formed, never current.
const EARLIER_RUN: &str = "0198c5b6-0000-4000-8000-0000000000e1";

fn served_fence(response: &axum::response::Response) -> SceneFence {
    let etag = response
        .headers()
        .get(http::header::ETAG)
        .and_then(|value| value.to_str().ok())
        .expect("scene responses carry their fence as ETag");
    SceneFence::parse(etag).expect("the ETag is a scene fence")
}

fn patch_zone_fenced(zone_id: &str, name: &str, fence: &str) -> Request<Body> {
    Request::builder()
        .method("PATCH")
        .uri(format!("/api/v1/scene/zones/{zone_id}"))
        .header("content-type", "application/json")
        .header(http::header::IF_MATCH, fence)
        .body(Body::from(json!({ "name": name }).to_string()))
        .expect("request should build")
}

#[tokio::test]
async fn the_etag_is_the_fence_and_round_trips_as_if_match() {
    let (state, app) = isolated();
    let read = send(&app, get("/api/v1/scene")).await;
    let fence = served_fence(&read);
    let etag = fence.to_etag();
    let document = body_json(read).await;
    assert_eq!(
        fence.run_epoch.as_deref(),
        Some(state.scene_manager.run_epoch())
    );
    assert_eq!(
        document["data"]["run_epoch"],
        state.scene_manager.run_epoch()
    );
    assert_eq!(document["data"]["revision"], fence.revision);
    let zone_id = primary_zone(&document)["id"]
        .as_str()
        .expect("zone id")
        .to_owned();

    let written = send(&app, patch_zone_fenced(&zone_id, "Fenced", &etag)).await;
    assert_eq!(written.status(), StatusCode::OK);
    assert_eq!(
        served_fence(&written),
        SceneFence::new(state.scene_manager.run_epoch(), fence.revision + 1),
        "the write answers with the fence for its own commit"
    );

    let replayed_fence = send(&app, patch_zone_fenced(&zone_id, "Again", &etag)).await;
    assert_eq!(
        replayed_fence.status(),
        StatusCode::PRECONDITION_FAILED,
        "the consumed fence is stale"
    );
}

#[tokio::test]
async fn a_fence_from_another_run_is_stale_even_when_the_revision_matches() {
    let (state, app) = isolated();
    let (revision, epoch, zone_id) = live_scene(&app).await;
    let foreign = SceneFence::new(EARLIER_RUN, revision).to_etag();

    let refused = send(&app, patch_zone_fenced(&zone_id, "Overwritten", &foreign)).await;
    assert_eq!(refused.status(), StatusCode::PRECONDITION_FAILED);
    assert_eq!(
        served_fence(&refused),
        SceneFence::new(epoch.as_str(), revision),
        "the refusal names the fence to rebase onto"
    );
    let body = body_json(refused).await;
    assert_eq!(body["error"]["details"]["expected"], revision);
    assert_eq!(body["error"]["details"]["current"], revision);
    assert_eq!(body["error"]["details"]["expected_epoch"], EARLIER_RUN);
    assert_eq!(body["error"]["details"]["current_epoch"], epoch.as_str());
    assert_eq!(
        state.scene_manager.revision(),
        revision,
        "nothing committed"
    );
}

#[tokio::test]
async fn a_bare_revision_fence_still_checks_the_revision_within_a_run() {
    let (state, app) = isolated();
    let (revision, epoch, zone_id) = live_scene(&app).await;

    let bare = send(&app, patch_zone(&zone_id, "Bare", revision)).await;
    assert_eq!(bare.status(), StatusCode::OK);
    let stale = send(&app, patch_zone(&zone_id, "Stale", revision)).await;
    assert_eq!(stale.status(), StatusCode::PRECONDITION_FAILED);
    assert_eq!(
        served_fence(&stale),
        SceneFence::new(epoch.as_str(), revision + 1),
        "a bare fence's refusal still names the epoch-qualified fence"
    );
    assert_eq!(state.scene_manager.revision(), revision + 1);
}

#[tokio::test]
async fn a_fence_that_is_not_a_fence_is_malformed() {
    let (state, app) = isolated();
    let (revision, _, zone_id) = live_scene(&app).await;
    let refused = send(&app, patch_zone_fenced(&zone_id, "Never", "\"run:abc\"")).await;
    assert_eq!(refused.status(), StatusCode::BAD_REQUEST);
    assert_eq!(state.scene_manager.revision(), revision);
}

#[tokio::test]
async fn a_stored_scene_read_in_another_run_is_refused_on_write_back() {
    let (_state, app) = isolated();
    let created = send(
        &app,
        json_request("POST", "/api/v1/scenes", &json!({ "name": "Stored" })),
    )
    .await;
    assert_eq!(created.status(), StatusCode::CREATED);
    let scene_id = body_json(created).await["data"]["id"]
        .as_str()
        .expect("scene id")
        .to_owned();
    let read = send(&app, get(format!("/api/v1/scenes/{scene_id}"))).await;
    assert_eq!(read.status(), StatusCode::OK);
    let mut document = body_json(read).await["data"].clone();

    let put = |document: &serde_json::Value| {
        json_request("PUT", &format!("/api/v1/scenes/{scene_id}"), document)
    };
    let mut foreign = document.clone();
    foreign["run_epoch"] = json!(EARLIER_RUN);
    foreign["name"] = json!("Overwritten");
    let refused = send(&app, put(&foreign)).await;
    assert_eq!(
        refused.status(),
        StatusCode::PRECONDITION_FAILED,
        "a body read in another run is stale whatever its revision"
    );

    document["name"] = json!("Renamed");
    let written = send(&app, put(&document)).await;
    assert_eq!(
        written.status(),
        StatusCode::OK,
        "the same body from this run lands"
    );
}

/// An effect with one preset, registered so the effect routes reach their
/// scene commit instead of answering 404.
async fn register_sweep_effect(state: &Arc<AppState>) -> EffectId {
    let id = EffectId::new(uuid::Uuid::now_v7());
    let metadata = EffectMetadata {
        id,
        name: "Sweep".to_owned(),
        author: "test".to_owned(),
        version: "0.1.0".to_owned(),
        description: "Sweep description".to_owned(),
        category: EffectCategory::Ambient,
        tags: Vec::new(),
        controls: Vec::new(),
        presets: vec![PresetTemplate {
            id: PresetId::stable("sweep"),
            name: "Sweep".to_owned(),
            description: None,
            controls: std::collections::HashMap::new(),
        }],
        audio_reactive: false,
        screen_reactive: false,
        input_reactive: false,
        source: EffectSource::Native {
            path: "builtin/sweep".into(),
        },
        license: None,
    };
    let _ = state
        .domains
        .effects
        .register(EffectEntry {
            metadata,
            source_path: "/tmp/sweep.rs".into(),
            modified: SystemTime::now(),
            state: EffectState::Loading,
        })
        .await;
    id
}

fn with_if_match(mut request: Request<Body>, fence: &str) -> Request<Body> {
    request.headers_mut().append(
        http::header::IF_MATCH,
        http::HeaderValue::from_str(fence).expect("valid fence"),
    );
    request
}

/// Every route that documents `If-Match`, sent a well-formed request at
/// the current revision under another run's epoch: each answers 412
/// naming the current fence, and none commits.
#[tokio::test]
async fn no_fenced_route_takes_a_fence_from_another_run() {
    let (state, app) = isolated();
    let effect_id = register_sweep_effect(&state).await;
    let created = send(
        &app,
        json_request("POST", "/api/v1/scenes", &json!({ "name": "Swept" })),
    )
    .await;
    assert_eq!(created.status(), StatusCode::CREATED);
    let scene_id = body_json(created).await["data"]["id"]
        .as_str()
        .expect("scene id")
        .to_owned();
    let stored =
        body_json(send(&app, get(format!("/api/v1/scenes/{scene_id}"))).await).await["data"]
            .clone();
    let document: serde_json::Value = serde_json::from_str(
        &api::openapi::document_json_pretty().expect("OpenAPI document renders"),
    )
    .expect("OpenAPI document parses");

    let mut swept = Vec::new();
    for (path, item) in document["paths"].as_object().expect("paths") {
        for (method, operation) in item.as_object().expect("path item") {
            let declares = operation["parameters"]
                .as_array()
                .is_some_and(|parameters| {
                    parameters
                        .iter()
                        .any(|parameter| parameter["name"] == "If-Match")
                });
            if !declares {
                continue;
            }
            let name = operation["operationId"]
                .as_str()
                .expect("operation id")
                .to_owned();
            let (revision, epoch, zone_id) = live_scene(&app).await;
            let id = if path.starts_with("/api/v1/scenes/") {
                scene_id.clone()
            } else {
                effect_id.to_string()
            };
            let uri = path
                .replace("{id}", &id)
                .replace("{preset}", &PresetId::stable("sweep").to_string())
                .replace("{zone}", &zone_id)
                .replace("{layer}", "0198c5b6-1111-7000-8000-00000000bbbb")
                .replace("{member}", "no-such-member");
            let body = match name.as_str() {
                "update_scene" => stored.clone(),
                "create_live_zone" | "patch_live_zone" => json!({ "name": "Swept" }),
                "put_live_zone_layout" => json!({ "placements": [] }),
                "reorder_live_zone_layers" => json!({ "order": [] }),
                "create_live_zone_layer" | "replace_live_zone_layer" => json!({
                    "source": { "type": "effect", "effect_id": effect_id, "controls": {} }
                }),
                "assign_live_zone_members" => json!({ "device_id": "mock:none" }),
                "edit_live_scene_members" => json!({
                    "scene_id": stored["id"],
                    "assignment": { "zone_id": zone_id, "device_id": "mock:none" }
                }),
                _ => json!({}),
            };
            let foreign = SceneFence::new(EARLIER_RUN, revision).to_etag();
            let answered = send(
                &app,
                with_if_match(
                    json_request(&method.to_ascii_uppercase(), &uri, &body),
                    &foreign,
                ),
            )
            .await;
            assert_eq!(
                answered.status(),
                StatusCode::PRECONDITION_FAILED,
                "{name} took another run's fence"
            );
            assert_eq!(
                served_fence(&answered),
                SceneFence::new(epoch.as_str(), revision),
                "{name} names the fence to rebase onto"
            );
            assert_eq!(
                state.scene_manager.revision(),
                revision,
                "{name} committed under another run's fence"
            );
            swept.push(name);
        }
    }
    assert_eq!(swept.len(), 16, "{swept:?}");
}

#[tokio::test]
async fn a_refusal_without_the_fence_outranks_a_fence_from_another_run() {
    let (state, app) = isolated();
    let stored = send(
        &app,
        json_request("POST", "/api/v1/scenes", &json!({ "name": "Stored" })),
    )
    .await;
    let scene_id = body_json(stored).await["data"]["id"]
        .as_str()
        .expect("scene id")
        .to_owned();
    let document =
        body_json(send(&app, get(format!("/api/v1/scenes/{scene_id}"))).await).await["data"]
            .clone();
    let (revision, _, _) = live_scene(&app).await;
    let foreign = SceneFence::new(EARLIER_RUN, revision).to_etag();

    let unknown_effect = send(
        &app,
        with_if_match(
            json_request("POST", "/api/v1/effects/no-such-effect/apply", &json!({})),
            &foreign,
        ),
    )
    .await;
    assert_eq!(unknown_effect.status(), StatusCode::NOT_FOUND);

    let unknown_scene = send(
        &app,
        with_if_match(
            json_request(
                "PUT",
                "/api/v1/scenes/0198c5b6-1111-7000-8000-00000000cccc",
                &document,
            ),
            &foreign,
        ),
    )
    .await;
    assert_eq!(unknown_scene.status(), StatusCode::NOT_FOUND);
    assert_eq!(state.scene_manager.revision(), revision);
}

#[tokio::test]
async fn if_match_is_one_fence_never_a_list() {
    let (state, app) = isolated();
    let (revision, epoch, zone_id) = live_scene(&app).await;
    let current = SceneFence::new(epoch.as_str(), revision).to_etag();

    let split = with_if_match(
        with_if_match(patch_zone_fenced(&zone_id, "Never", "*"), &current),
        &current,
    );
    assert_eq!(
        split
            .headers()
            .get_all(http::header::IF_MATCH)
            .iter()
            .count(),
        3
    );
    let refused = send(&app, split).await;
    assert_eq!(
        refused.status(),
        StatusCode::BAD_REQUEST,
        "a wildcard line never waves through the lines after it"
    );
    for joined in [
        format!("{current}, {current}"),
        format!("{epoch}:{revision},{epoch}:{revision}"),
        format!("\"{epoch}:{revision},{epoch}:{revision}\""),
    ] {
        let refused = send(&app, patch_zone_fenced(&zone_id, "Never", &joined)).await;
        assert_eq!(refused.status(), StatusCode::BAD_REQUEST, "{joined}");
    }
    for padded in [
        format!("\"{epoch}:0{revision}\""),
        format!("\"{epoch}:+{revision}\""),
    ] {
        let refused = send(&app, patch_zone_fenced(&zone_id, "Never", &padded)).await;
        assert_eq!(refused.status(), StatusCode::BAD_REQUEST, "{padded}");
    }
    assert_eq!(state.scene_manager.revision(), revision);
}
