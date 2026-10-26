//! The store's bounds and the guard's behavior on a synthetic router, so
//! each case can hold the handler exactly where it needs to.

use std::sync::atomic::{AtomicUsize, Ordering};

use axum::Router;
use axum::routing::post;
use tokio::sync::Notify;
use tower::ServiceExt;

use super::*;

fn fingerprint(body: &str) -> Fingerprint {
    Fingerprint::of(&Method::POST, "/things", None, body.as_bytes())
}

fn answered(body: &'static str) -> Outcome {
    Outcome::Answered(StoredResponse {
        status: StatusCode::OK,
        headers: HeaderMap::new(),
        body: Bytes::from_static(body.as_bytes()),
    })
}

fn run(store: &IdempotencyStore, key: &str, body: &'static str) {
    match store.claim(key, fingerprint(body)) {
        Claim::Execute(in_flight) => {
            in_flight.settle(answered(body));
        }
        _ => panic!("{key} should be a first sighting"),
    }
}

fn retention(entries: usize, bytes: usize) -> Retention {
    Retention {
        entries,
        bytes,
        age: IDEMPOTENCY_RETENTION_AGE,
    }
}

#[test]
fn the_oldest_answers_go_first_past_the_entry_bound() {
    let store = IdempotencyStore::with_retention(retention(2, usize::MAX));
    run(&store, "a", "one");
    run(&store, "b", "two");
    run(&store, "c", "three");
    assert_eq!(store.completed_len(), 2);
    assert!(matches!(
        store.claim("a", fingerprint("one")),
        Claim::Execute(_)
    ));
    assert!(matches!(
        store.claim("c", fingerprint("three")),
        Claim::Replay(_)
    ));
}

#[test]
fn the_oldest_answers_go_first_past_the_byte_bound() {
    let store = IdempotencyStore::with_retention(retention(usize::MAX, 10));
    run(&store, "a", "123456");
    run(&store, "b", "abcdef");
    assert_eq!(store.completed_len(), 1);
    assert_eq!(store.stored_bytes(), 6);
    assert!(matches!(
        store.claim("b", fingerprint("abcdef")),
        Claim::Replay(_)
    ));
}

#[tokio::test(start_paused = true)]
async fn answers_expire_after_the_retention_age() {
    let store = IdempotencyStore::with_retention(Retention {
        entries: usize::MAX,
        bytes: usize::MAX,
        age: Duration::from_secs(10),
    });
    run(&store, "a", "one");
    tokio::time::advance(Duration::from_secs(9)).await;
    assert!(matches!(
        store.claim("a", fingerprint("one")),
        Claim::Replay(_)
    ));
    tokio::time::advance(Duration::from_secs(2)).await;
    let restarted = store.claim("a", fingerprint("one"));
    assert!(
        matches!(restarted, Claim::Execute(_)),
        "an expired key starts over"
    );
    assert_eq!(store.completed_len(), 0, "the expired answer is released");
    assert_eq!(store.stored_bytes(), 0);
    drop(restarted);
}

#[test]
fn an_in_flight_request_is_never_evicted() {
    let store = IdempotencyStore::with_retention(retention(1, usize::MAX));
    let Claim::Execute(held) = store.claim("held", fingerprint("x")) else {
        panic!("first sighting");
    };
    run(&store, "b", "two");
    run(&store, "c", "three");
    assert!(matches!(
        store.claim("held", fingerprint("x")),
        Claim::Wait(_)
    ));
    held.settle(answered("x"));
    assert!(matches!(
        store.claim("held", fingerprint("x")),
        Claim::Replay(_)
    ));
}

#[test]
fn a_key_names_exactly_one_request() {
    let store = IdempotencyStore::default();
    let Claim::Execute(held) = store.claim("k", fingerprint("one")) else {
        panic!("first sighting");
    };
    assert!(matches!(
        store.claim("k", fingerprint("two")),
        Claim::Mismatch
    ));
    held.settle(answered("one"));
    assert!(matches!(
        store.claim("k", fingerprint("two")),
        Claim::Mismatch
    ));
    assert!(matches!(
        store.claim("k", fingerprint("one")),
        Claim::Replay(_)
    ));
}

#[test]
fn the_fingerprint_covers_method_target_fence_and_body() {
    let base = Fingerprint::of(&Method::PUT, "/a", None, b"x");
    let fence = HeaderValue::from_static("\"7\"");
    assert_eq!(base, Fingerprint::of(&Method::PUT, "/a", None, b"x"));
    assert_ne!(base, Fingerprint::of(&Method::POST, "/a", None, b"x"));
    assert_ne!(base, Fingerprint::of(&Method::PUT, "/b", None, b"x"));
    assert_ne!(
        base,
        Fingerprint::of(&Method::PUT, "/a", Some(&fence), b"x")
    );
    assert_ne!(base, Fingerprint::of(&Method::PUT, "/a", None, b"y"));
    assert_ne!(
        Fingerprint::of(&Method::PUT, "/ab", None, b"c"),
        Fingerprint::of(&Method::PUT, "/a", None, b"bc"),
        "components are length-prefixed"
    );
}

#[test]
fn a_dropped_claim_is_recorded_as_abandoned() {
    let store = IdempotencyStore::default();
    let Claim::Execute(held) = store.claim("k", fingerprint("one")) else {
        panic!("first sighting");
    };
    drop(held);
    let Claim::Replay(outcome) = store.claim("k", fingerprint("one")) else {
        panic!("an abandoned key still answers");
    };
    assert!(matches!(outcome.as_ref(), Outcome::Abandoned));
}

#[test]
fn keys_are_opaque_visible_ascii() {
    let parse = |raw: &'static str| parse_key(&HeaderValue::from_static(raw)).map(str::to_owned);
    assert_eq!(
        parse("8e03978e-40d5-43e8-bc93-6894a57f9324")
            .ok()
            .as_deref(),
        Some("8e03978e-40d5-43e8-bc93-6894a57f9324")
    );
    assert_eq!(
        parse("\"8e03978e-40d5-43e8-bc93-6894a57f9324\"")
            .ok()
            .as_deref(),
        Some("8e03978e-40d5-43e8-bc93-6894a57f9324"),
        "the draft's quoted structured-field form names the same key"
    );
    for refused in ["", "\"\"", "two words", "a\"b"] {
        assert!(
            matches!(parse(refused), Err(DomainError::Malformed { .. })),
            "{refused:?} is not a key"
        );
    }
    let long =
        HeaderValue::from_str(&"k".repeat(IDEMPOTENCY_KEY_MAX_LEN + 1)).expect("header value");
    assert!(parse_key(&long).is_err());
}

#[test]
fn an_answer_larger_than_the_byte_bound_still_replays() {
    let store = IdempotencyStore::with_retention(retention(usize::MAX, 4));
    run(&store, "big", "123456");
    assert!(matches!(
        store.claim("big", fingerprint("123456")),
        Claim::Replay(_)
    ));
    run(&store, "next", "abcdef");
    assert_eq!(
        store.completed_len(),
        1,
        "the older oversized answer goes once a newer one lands"
    );
}

#[test]
fn keys_are_scoped_per_credential() {
    let grant = |id: &str| {
        CredentialGrant::new(
            crate::api::security::CredentialTier::Control,
            id,
            tokio_util::sync::CancellationToken::new(),
        )
    };
    let alice = grant("alice");
    let bob = grant("bob");
    assert_ne!(
        scoped_key(Some(&alice), "k"),
        scoped_key(Some(&bob), "k"),
        "two credentials never share a key"
    );
    assert_ne!(scoped_key(None, "k"), scoped_key(Some(&alice), "k"));
    assert_ne!(
        scoped_key(Some(&grant("a:b")), "c"),
        scoped_key(Some(&grant("a")), "b:c"),
        "the scope is length-prefixed"
    );
}

struct Probe {
    applied: AtomicUsize,
    entered: Notify,
    release: Notify,
    hold: bool,
}

fn probe_app(probe: &Arc<Probe>) -> Router {
    let handler = {
        let probe = Arc::clone(probe);
        move || {
            let probe = Arc::clone(&probe);
            async move {
                let count = probe.applied.fetch_add(1, Ordering::SeqCst) + 1;
                probe.entered.notify_one();
                if probe.hold {
                    probe.release.notified().await;
                }
                (StatusCode::CREATED, format!("applied {count}")).into_response()
            }
        }
    };
    let layer = IdempotencyLayer {
        store: IdempotencyStore::default(),
        routes: Arc::new(IdempotentRoutes::of(&[(Method::POST, "/things")])),
    };
    Router::new()
        .route("/things", post(handler.clone()))
        .route("/plain", post(handler))
        .layer(axum::middleware::from_fn_with_state(layer, guard))
}

fn probe(hold: bool) -> Arc<Probe> {
    Arc::new(Probe {
        applied: AtomicUsize::new(0),
        entered: Notify::new(),
        release: Notify::new(),
        hold,
    })
}

fn request(path: &str, key: Option<&str>) -> Request {
    let mut builder = axum::http::Request::builder().method("POST").uri(path);
    if let Some(key) = key {
        builder = builder.header(&IDEMPOTENCY_KEY, key);
    }
    builder.body(Body::from("{}")).expect("request builds")
}

async fn text(response: Response) -> String {
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body reads");
    String::from_utf8(bytes.to_vec()).expect("utf-8 body")
}

fn replayed(response: &Response) -> bool {
    response
        .headers()
        .get(&IDEMPOTENCY_REPLAYED)
        .map(HeaderValue::as_bytes)
        == Some(b"true")
}

#[tokio::test]
async fn a_duplicate_waits_for_the_original_and_never_applies_twice() {
    let probe = probe(true);
    let app = probe_app(&probe);

    let first = tokio::spawn(app.clone().oneshot(request("/things", Some("k"))));
    probe.entered.notified().await;
    let second = tokio::spawn(app.clone().oneshot(request("/things", Some("k"))));
    for _ in 0..16 {
        tokio::task::yield_now().await;
    }
    assert!(!second.is_finished(), "the duplicate waits on the original");

    probe.release.notify_one();
    let first = first.await.expect("first task").expect("first response");
    let second = second.await.expect("second task").expect("second response");
    assert_eq!(probe.applied.load(Ordering::SeqCst), 1);
    assert_eq!(first.status(), StatusCode::CREATED);
    assert_eq!(second.status(), StatusCode::CREATED);
    assert!(!replayed(&first));
    assert!(replayed(&second));
    assert_eq!(text(first).await, text(second).await);
}

#[tokio::test]
async fn the_original_finishes_and_is_recorded_when_its_caller_hangs_up() {
    let probe = probe(true);
    let app = probe_app(&probe);

    let first = tokio::spawn(app.clone().oneshot(request("/things", Some("k"))));
    probe.entered.notified().await;
    first.abort();
    let _ = first.await;
    probe.release.notify_one();

    let retry = app
        .clone()
        .oneshot(request("/things", Some("k")))
        .await
        .expect("retry response");
    assert!(replayed(&retry));
    assert_eq!(text(retry).await, "applied 1");
    assert_eq!(probe.applied.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn requests_without_a_key_or_off_the_opted_in_routes_pass_through() {
    let probe = probe(false);
    let app = probe_app(&probe);
    for request in [
        request("/things", None),
        request("/things", None),
        request("/plain", Some("k")),
        request("/plain", Some("k")),
    ] {
        let response = app.clone().oneshot(request).await.expect("response");
        assert_eq!(response.status(), StatusCode::CREATED);
        assert!(!replayed(&response));
    }
    assert_eq!(probe.applied.load(Ordering::SeqCst), 4);
}

#[tokio::test]
async fn a_key_on_a_different_request_is_refused() {
    let probe = probe(false);
    let app = probe_app(&probe);
    let first = app
        .clone()
        .oneshot(request("/things", Some("k")))
        .await
        .expect("response");
    assert_eq!(first.status(), StatusCode::CREATED);

    let mut different = request("/things", Some("k"));
    *different.body_mut() = Body::from("{\"other\":true}");
    let refused = app.clone().oneshot(different).await.expect("response");
    assert_eq!(refused.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body: serde_json::Value =
        serde_json::from_str(&text(refused).await).expect("error envelope");
    assert_eq!(body["error"]["code"], "idempotency_key_mismatch");
    assert_eq!(probe.applied.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_handler_that_dies_leaves_its_key_answering_and_unapplied() {
    let applied = Arc::new(AtomicUsize::new(0));
    let handler = {
        let applied = Arc::clone(&applied);
        move || {
            let applied = Arc::clone(&applied);
            async move {
                let earlier = applied.fetch_add(1, Ordering::SeqCst);
                assert!(earlier > 0, "the first attempt dies mid-write");
                StatusCode::OK.into_response()
            }
        }
    };
    let layer = IdempotencyLayer {
        store: IdempotencyStore::default(),
        routes: Arc::new(IdempotentRoutes::of(&[(Method::POST, "/things")])),
    };
    let app = Router::new()
        .route("/things", post(handler))
        .layer(axum::middleware::from_fn_with_state(layer, guard));

    let first = app
        .clone()
        .oneshot(request("/things", Some("k")))
        .await
        .expect("response");
    assert_eq!(first.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert!(!replayed(&first));

    let retry = app
        .clone()
        .oneshot(request("/things", Some("k")))
        .await
        .expect("response");
    assert_eq!(retry.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert!(
        replayed(&retry),
        "the abandoned key answers instead of applying again"
    );
    assert_eq!(applied.load(Ordering::SeqCst), 1);
}
