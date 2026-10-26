//! Contract pins for the scene write fence that Studio's live commits
//! lean on (Design 73 D2).
//!
//! A read that serves contents beside a revision must serve the revision
//! those contents were committed at, or a fenced client writes stale
//! fields straight past the fence.

use std::sync::Arc;

use axum::body::Body;
use http::{Request, StatusCode};
use hypercolor_daemon::api;
use hypercolor_daemon::app_state::AppState;
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
