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
    CLOCK_BASE_CEILING, REVISION_RESERVATION_BLOCK, SCENE_REVISIONS_FILE, SceneRun,
};
use hypercolor_types::api::envelope::RUN_EPOCH_HEADER;
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
}
