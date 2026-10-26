//! The store's bounds and the guard's behavior on a synthetic router, so
//! each case can hold the handler exactly where it needs to.

use std::sync::atomic::{AtomicUsize, Ordering};

use axum::Router;
use axum::routing::post;
use tokio::sync::Notify;
use tower::ServiceExt;

use super::*;

fn fingerprint(body: &str) -> Fingerprint {
    Fingerprint::of(&Method::POST, "/things", None, None, body.as_bytes())
}

fn answered(body: &'static str) -> Outcome {
    Outcome::Answered(StoredResponse {
        status: StatusCode::OK,
        headers: HeaderMap::new(),
        body: Bytes::from_static(body.as_bytes()),
    })
}

fn run(store: &IdempotencyStore, key: &str, body: &'static str) {
    match store.claim(key, fingerprint(body), 0) {
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
        ..Retention::default()
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
        store.claim("a", fingerprint("one"), 0),
        Claim::Execute(_)
    ));
    assert!(matches!(
        store.claim("c", fingerprint("three"), 0),
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
        store.claim("b", fingerprint("abcdef"), 0),
        Claim::Replay(_)
    ));
}

#[tokio::test(start_paused = true)]
async fn answers_expire_after_the_retention_age() {
    let store = IdempotencyStore::with_retention(Retention {
        entries: usize::MAX,
        bytes: usize::MAX,
        age: Duration::from_secs(10),
        ..Retention::default()
    });
    run(&store, "a", "one");
    tokio::time::advance(Duration::from_secs(9)).await;
    assert!(matches!(
        store.claim("a", fingerprint("one"), 0),
        Claim::Replay(_)
    ));
    tokio::time::advance(Duration::from_secs(2)).await;
    let restarted = store.claim("a", fingerprint("one"), 0);
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
    let Claim::Execute(held) = store.claim("held", fingerprint("x"), 0) else {
        panic!("first sighting");
    };
    run(&store, "b", "two");
    run(&store, "c", "three");
    assert!(matches!(
        store.claim("held", fingerprint("x"), 0),
        Claim::Wait(_)
    ));
    held.settle(answered("x"));
    assert!(matches!(
        store.claim("held", fingerprint("x"), 0),
        Claim::Replay(_)
    ));
}

fn pending(entries: usize, bytes: usize) -> Retention {
    Retention {
        pending_entries: entries,
        pending_bytes: bytes,
        ..Retention::default()
    }
}

#[test]
fn new_keys_past_the_pending_bound_are_refused_and_pending_keys_kept() {
    let store = IdempotencyStore::with_retention(pending(1, usize::MAX));
    let Claim::Execute(held) = store.claim("first", fingerprint("x"), 0) else {
        panic!("first sighting");
    };
    assert!(matches!(
        store.claim("second", fingerprint("y"), 0),
        Claim::Busy
    ));
    assert!(
        matches!(store.claim("first", fingerprint("x"), 0), Claim::Wait(_)),
        "a repeat of a pending key still waits for its original"
    );
    assert_eq!(store.pending_len(), 1);

    held.settle(answered("x"));
    assert_eq!(store.pending_len(), 0);
    let Claim::Execute(next) = store.claim("second", fingerprint("y"), 0) else {
        panic!("a refused key claims nothing, so its retry runs once the bound frees");
    };
    next.settle(answered("y"));
}

#[test]
fn pending_request_bytes_are_bounded_too() {
    let store = IdempotencyStore::with_retention(pending(usize::MAX, 10));
    let Claim::Execute(big) = store.claim("big", fingerprint("a"), 6) else {
        panic!("first sighting");
    };
    assert!(matches!(
        store.claim("over", fingerprint("b"), 6),
        Claim::Busy
    ));
    let Claim::Execute(fits) = store.claim("fits", fingerprint("c"), 4) else {
        panic!("a body that fits the remaining budget runs");
    };
    big.settle(answered("a"));
    drop(fits);
    assert_eq!(store.pending_len(), 0);
    assert!(matches!(
        store.claim("over", fingerprint("b"), 6),
        Claim::Execute(_)
    ));
}

#[test]
fn a_key_names_exactly_one_request() {
    let store = IdempotencyStore::default();
    let Claim::Execute(held) = store.claim("k", fingerprint("one"), 0) else {
        panic!("first sighting");
    };
    assert!(matches!(
        store.claim("k", fingerprint("two"), 0),
        Claim::Mismatch
    ));
    held.settle(answered("one"));
    assert!(matches!(
        store.claim("k", fingerprint("two"), 0),
        Claim::Mismatch
    ));
    assert!(matches!(
        store.claim("k", fingerprint("one"), 0),
        Claim::Replay(_)
    ));
}

#[test]
fn the_fingerprint_covers_every_part_a_scene_write_reads() {
    let of = |method: &Method,
              target: &str,
              fence: Option<&HeaderValue>,
              kind: Option<&HeaderValue>,
              body: &[u8]| { Fingerprint::of(method, target, fence, kind, body) };
    let json = HeaderValue::from_static("application/json");
    let base = of(&Method::PUT, "/a", None, Some(&json), b"x");
    let fence = HeaderValue::from_static("\"7\"");
    let empty = HeaderValue::from_static("");
    assert_eq!(base, of(&Method::PUT, "/a", None, Some(&json), b"x"));
    assert_ne!(base, of(&Method::POST, "/a", None, Some(&json), b"x"));
    assert_ne!(base, of(&Method::PUT, "/b", None, Some(&json), b"x"));
    assert_ne!(
        base,
        of(&Method::PUT, "/a", Some(&fence), Some(&json), b"x")
    );
    assert_ne!(base, of(&Method::PUT, "/a", None, Some(&json), b"y"));
    assert_ne!(
        base,
        of(&Method::PUT, "/a", None, None, b"x"),
        "a body without a content type is read differently"
    );
    assert_ne!(
        base,
        of(
            &Method::PUT,
            "/a",
            None,
            Some(&HeaderValue::from_static("text/plain")),
            b"x"
        )
    );
    assert_ne!(
        of(&Method::PUT, "/a", None, None, b"x"),
        of(&Method::PUT, "/a", Some(&empty), None, b"x"),
        "an absent header never hashes like an empty one"
    );
    assert_ne!(
        of(&Method::PUT, "/ab", None, None, b"c"),
        of(&Method::PUT, "/a", None, None, b"bc"),
        "components are length-prefixed"
    );
}

#[test]
fn the_fingerprint_reads_every_if_match_line() {
    let any = HeaderValue::from_static("*");
    let first = HeaderValue::from_static("\"run:7\"");
    let second = HeaderValue::from_static("\"run:8\"");
    let of = |lines: &[&HeaderValue]| {
        Fingerprint::of(&Method::PUT, "/a", lines.iter().copied(), None, b"x")
    };
    assert_eq!(of(&[&any, &first]), of(&[&any, &first]));
    assert_ne!(
        of(&[&any, &first]),
        of(&[&any, &second]),
        "a later line counts"
    );
    assert_ne!(of(&[&any]), of(&[&any, &first]), "an extra line counts");
    assert_ne!(
        of(&[&any, &first]),
        of(&[&first, &any]),
        "line order counts"
    );
    assert_ne!(of(&[]), of(&[&any]));
}

#[test]
fn a_dropped_claim_is_recorded_as_abandoned() {
    let store = IdempotencyStore::default();
    let Claim::Execute(held) = store.claim("k", fingerprint("one"), 0) else {
        panic!("first sighting");
    };
    drop(held);
    let Claim::Replay(outcome) = store.claim("k", fingerprint("one"), 0) else {
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
        store.claim("big", fingerprint("123456"), 0),
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
    let scope = CredentialScope::for_test;
    assert_ne!(
        scoped_key(Some(&scope("credential:alice")), "k"),
        scoped_key(Some(&scope("credential:bob")), "k"),
        "two credentials never share a key"
    );
    assert_ne!(
        scoped_key(Some(&scope("environment:control")), "k"),
        scoped_key(Some(&scope("environment:read")), "k")
    );
    assert_ne!(
        scoped_key(None, "k"),
        scoped_key(Some(&scope("daemon-session")), "k"),
        "an authenticated caller never shares the anonymous scope"
    );
    assert_ne!(
        scoped_key(Some(&scope("a:b")), "c"),
        scoped_key(Some(&scope("a")), "b:c"),
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
    probe_app_with(probe, IdempotencyStore::default())
}

fn probe_app_with(probe: &Arc<Probe>, store: IdempotencyStore) -> Router {
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
        store,
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

#[tokio::test]
async fn a_new_key_past_the_pending_bound_answers_a_retryable_503() {
    let probe = probe(true);
    let app = probe_app_with(
        &probe,
        IdempotencyStore::with_retention(pending(1, usize::MAX)),
    );
    let first = tokio::spawn(app.clone().oneshot(request("/things", Some("first"))));
    probe.entered.notified().await;

    let refused = app
        .clone()
        .oneshot(request("/things", Some("second")))
        .await
        .expect("response");
    assert_eq!(refused.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body: serde_json::Value =
        serde_json::from_str(&text(refused).await).expect("error envelope");
    assert_eq!(body["error"]["code"], "service_unavailable");

    probe.release.notify_one();
    let first = first.await.expect("first task").expect("first response");
    assert_eq!(first.status(), StatusCode::CREATED);

    let retried = tokio::spawn(app.clone().oneshot(request("/things", Some("second"))));
    probe.entered.notified().await;
    probe.release.notify_one();
    let retried = retried.await.expect("retry task").expect("retry response");
    assert_eq!(retried.status(), StatusCode::CREATED);
    assert!(!replayed(&retried), "the refused key was never recorded");
    assert_eq!(probe.applied.load(Ordering::SeqCst), 2);
}

struct OneGrant;

impl crate::api::security::CredentialAuthority for OneGrant {
    fn ceiling(&self) -> crate::api::security::CredentialTier {
        crate::api::security::CredentialTier::Control
    }

    fn authenticate(&self, presented: &str) -> Option<crate::api::security::CredentialGrant> {
        (presented == "hc_grant_token").then(|| {
            crate::api::security::CredentialGrant::new(
                crate::api::security::CredentialTier::Control,
                "studio-tablet",
                tokio_util::sync::CancellationToken::new(),
            )
        })
    }
}

/// Through the real security layer: one key sent under two credentials is
/// two writes, and a repeat under the first credential replays its own.
#[tokio::test]
async fn the_security_layer_scopes_one_key_per_credential() {
    const CONTROL_KEY: &str = "hc_ak_control_scope_test";
    let probe = probe(false);
    let handler = {
        let probe = Arc::clone(&probe);
        move || {
            let probe = Arc::clone(&probe);
            async move {
                let count = probe.applied.fetch_add(1, Ordering::SeqCst) + 1;
                (StatusCode::CREATED, format!("applied {count}")).into_response()
            }
        }
    };
    let layer = IdempotencyLayer {
        store: IdempotencyStore::default(),
        routes: Arc::new(IdempotentRoutes::of(&[(Method::POST, "/things")])),
    };
    let app = Router::new()
        .route("/api/v1/things", post(handler))
        .layer(axum::middleware::from_fn_with_state(layer, guard))
        .layer(axum::middleware::from_fn_with_state(
            crate::api::security::SecurityState::with_keys(Some(CONTROL_KEY), None)
                .with_credential_authority(Arc::new(OneGrant)),
            crate::api::security::enforce_security,
        ));
    let as_credential =
        |token: &str| {
            let mut request = request("/api/v1/things", Some("shared-key"));
            request.headers_mut().insert(
                header::AUTHORIZATION,
                HeaderValue::from_str(&format!("Bearer {token}")).expect("bearer"),
            );
            request.extensions_mut().insert(axum::extract::ConnectInfo(
                std::net::SocketAddr::from(([127, 0, 0, 1], 1042)),
            ));
            request
        };

    let environment = app
        .clone()
        .oneshot(as_credential(CONTROL_KEY))
        .await
        .expect("response");
    assert_eq!(environment.status(), StatusCode::CREATED);
    assert!(!replayed(&environment));
    let granted = app
        .clone()
        .oneshot(as_credential("hc_grant_token"))
        .await
        .expect("response");
    assert_eq!(granted.status(), StatusCode::CREATED);
    assert!(
        !replayed(&granted),
        "another credential's identical key is its own write"
    );
    let again = app
        .clone()
        .oneshot(as_credential(CONTROL_KEY))
        .await
        .expect("response");
    assert!(replayed(&again));
    assert_eq!(text(again).await, "applied 1");
    assert_eq!(probe.applied.load(Ordering::SeqCst), 2);
}
