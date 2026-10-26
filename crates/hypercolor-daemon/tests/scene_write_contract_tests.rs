//! Contract pins for the scene write fence that Studio's live commits
//! lean on (Design 73 D2).
//!
//! A read that serves contents beside a revision must serve the revision
//! those contents were committed at, or a fenced client writes stale
//! fields straight past the fence. A revision from an earlier daemon run
//! must never pass as a fence in a later one, and every answer names the
//! run it came from so a client knows when to re-anchor.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, UNIX_EPOCH};

use axum::body::Body;
use http::{Request, StatusCode};
use hypercolor_daemon::api;
use hypercolor_daemon::app_state::{AppState, AppStateBuilder};
use hypercolor_daemon::domain::scene_run::{
    CLOCK_BASE_CEILING, MAX_TRUSTED_FLOOR, REVISION_RESERVATION_BLOCK, SCENE_REVISIONS_FILE,
    SceneRun,
};
use hypercolor_types::api::envelope::{
    IDEMPOTENCY_KEY_HEADER, IDEMPOTENCY_KEY_MISMATCH_CODE, IDEMPOTENCY_REPLAYED_HEADER,
    RUN_EPOCH_HEADER,
};
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
        .and_then(|value| value.trim_matches('"').parse().ok())
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
}

#[test]
fn a_resumed_run_starts_above_the_previous_reservation_even_with_a_backwards_clock() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let now = UNIX_EPOCH + Duration::from_hours(500_000);

    let first = SceneRun::resume_at(tmp.path(), now);
    let first_ceiling = first
        .reserved_until()
        .expect("a resumed run persists a ceiling");
    assert_eq!(
        first_ceiling,
        first.start_revision() + REVISION_RESERVATION_BLOCK
    );

    // The clock jumped back a decade between runs.
    let second = SceneRun::resume_at(tmp.path(), now - Duration::from_hours(87_600));
    assert!(second.start_revision() >= first_ceiling);
    assert_ne!(second.epoch(), first.epoch());

    // And a clock before the Unix epoch contributes nothing at all.
    let third = SceneRun::resume_at(tmp.path(), UNIX_EPOCH);
    assert!(third.start_revision() >= second.reserved_until().expect("ceiling"));
}

#[test]
fn a_lost_or_corrupt_reservation_falls_back_to_the_clock() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let now = UNIX_EPOCH + Duration::from_hours(500_000);
    let micros = 1_800_000_000 * 1_000_000;

    let fresh = SceneRun::resume_at(tmp.path(), now);
    assert_eq!(
        fresh.start_revision(),
        micros,
        "no reservation yet: start at the clock"
    );

    std::fs::write(tmp.path().join(SCENE_REVISIONS_FILE), b"not json").expect("corrupt");
    let recovered = SceneRun::resume_at(tmp.path(), now + Duration::from_secs(1));
    assert_eq!(recovered.start_revision(), micros + 1_000_000);
    assert!(
        SceneRun::resume_at(tmp.path(), now).start_revision()
            >= recovered.reserved_until().expect("ceiling"),
        "the corrupt file is replaced by a readable reservation"
    );
}

#[test]
fn a_floor_beyond_any_real_ceiling_is_distrusted() {
    let tmp = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        tmp.path().join(SCENE_REVISIONS_FILE),
        json!({ "reserved_until": MAX_TRUSTED_FLOOR }).to_string(),
    )
    .expect("seed a bogus reservation");
    let now = UNIX_EPOCH + Duration::from_hours(500_000);
    let run = SceneRun::resume_at(tmp.path(), now);
    assert_eq!(
        run.start_revision(),
        1_800_000_000 * 1_000_000,
        "a floor no real run could reach falls back to the clock"
    );
    assert!(
        SceneRun::resume_at(tmp.path(), now).start_revision() < MAX_TRUSTED_FLOOR,
        "and the bogus file is replaced"
    );
}

#[test]
fn the_clock_term_stays_exact_as_a_json_double() {
    let tmp = tempfile::tempdir().expect("tempdir");
    // Year 2286: microseconds since the epoch pass 2^53 there.
    let far_future = UNIX_EPOCH + Duration::from_secs(10_000_000_000);
    let run = SceneRun::resume_at(tmp.path(), far_future);
    assert_eq!(run.start_revision(), CLOCK_BASE_CEILING);
}

#[test]
fn an_ephemeral_run_starts_at_zero_under_its_own_epoch() {
    let first = SceneRun::ephemeral();
    let second = SceneRun::ephemeral();
    assert_eq!(first.start_revision(), 0);
    assert_eq!(first.reserved_until(), None);
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
