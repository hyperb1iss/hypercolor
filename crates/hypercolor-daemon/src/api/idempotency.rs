//! `Idempotency-Key` for scene-tree writes, following the IETF httpapi
//! idempotency-key draft.
//!
//! A client that lost a reply cannot tell whether its write committed, and
//! a read only proves "not committed yet", never "will not commit". The key
//! closes that gap: a retry with the same key never applies twice, it gets
//! the original answer.
//!
//! - The first request with a key runs normally, on a task of its own so
//!   that a dropped connection cannot cancel it between commit and record,
//!   and its status, headers, and body are stored once it answers.
//! - A repeat with the same key and the same request fingerprint is not
//!   applied again. It gets the stored answer with `Idempotency-Replayed:
//!   true`; a repeat that arrives while the original is still running waits
//!   for it and gets the same.
//! - The same key on a different request (method, path, `If-Match`, or
//!   body) answers `422 idempotency_key_mismatch`.
//! - A request without the header costs one header lookup and is otherwise
//!   untouched, so the CLI, Home Assistant, and MCP behave as before.
//!
//! Every completed answer is kept, refusals included: a `412` for a stale
//! fence is as definitive as a `200`, and only a stored answer guarantees a
//! retry cannot apply what the original did not.
//!
//! Which routes honor the key is declared in the OpenAPI document: an
//! operation that documents the optional `Idempotency-Key` header gets this
//! behavior, so the published contract and the enforced one cannot drift.
//! Keys live in memory for one daemon run, bounded by
//! [`IDEMPOTENCY_RETENTION_ENTRIES`], [`IDEMPOTENCY_RETENTION_BYTES`], and
//! [`IDEMPOTENCY_RETENTION_AGE`]; the run epoch on every response tells a
//! client when its keys stopped meaning anything.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use axum::body::{Body, Bytes};
use axum::extract::{MatchedPath, Request, State};
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use hypercolor_types::api::envelope::{IDEMPOTENCY_KEY_HEADER, IDEMPOTENCY_REPLAYED_HEADER};
use sha2::{Digest, Sha256};
use tokio::sync::watch;
use tokio::time::Instant;
use tracing::Instrument;

use crate::domain::DomainError;

/// The request header naming an idempotent attempt.
pub(super) const IDEMPOTENCY_KEY: HeaderName = HeaderName::from_static(IDEMPOTENCY_KEY_HEADER);

/// The response header marking an answer served from the store.
pub(super) const IDEMPOTENCY_REPLAYED: HeaderName =
    HeaderName::from_static(IDEMPOTENCY_REPLAYED_HEADER);

/// The name an operation declares in the OpenAPI document to opt in.
pub(super) const DOCUMENTED_HEADER: &str = "Idempotency-Key";

/// Most completed answers kept at once; the oldest goes first.
pub(crate) const IDEMPOTENCY_RETENTION_ENTRIES: usize = 1024;

/// Most response bytes kept at once across every completed answer.
pub(crate) const IDEMPOTENCY_RETENTION_BYTES: usize = 32 * 1024 * 1024;

/// How long a completed answer stays replayable.
pub(crate) const IDEMPOTENCY_RETENTION_AGE: Duration = Duration::from_hours(1);

/// Longest key accepted. A UUID is 36 characters.
pub(crate) const IDEMPOTENCY_KEY_MAX_LEN: usize = 255;

/// The bounds a store keeps completed answers within.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Retention {
    pub(crate) entries: usize,
    pub(crate) bytes: usize,
    pub(crate) age: Duration,
}

impl Default for Retention {
    fn default() -> Self {
        Self {
            entries: IDEMPOTENCY_RETENTION_ENTRIES,
            bytes: IDEMPOTENCY_RETENTION_BYTES,
            age: IDEMPOTENCY_RETENTION_AGE,
        }
    }
}

/// One daemon run's record of keyed requests, shared by every router built
/// from the same application state.
#[derive(Clone, Default)]
pub(crate) struct IdempotencyStore {
    inner: Arc<Mutex<StoreInner>>,
    retention: Retention,
}

impl std::fmt::Debug for IdempotencyStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IdempotencyStore")
            .field("retention", &self.retention)
            .finish_non_exhaustive()
    }
}

#[derive(Default)]
struct StoreInner {
    entries: HashMap<Arc<str>, Entry>,
    /// Completed entries in completion order, so eviction is O(1). A record
    /// whose sequence no longer matches its entry was replaced and is
    /// skipped when it reaches the front.
    completed: VecDeque<CompletedRecord>,
    completed_count: usize,
    stored_bytes: usize,
    next_sequence: u64,
}

struct CompletedRecord {
    key: Arc<str>,
    sequence: u64,
    completed_at: Instant,
}

enum Entry {
    InFlight {
        fingerprint: Fingerprint,
        done: watch::Receiver<Option<Arc<Outcome>>>,
    },
    Completed {
        fingerprint: Fingerprint,
        outcome: Arc<Outcome>,
        sequence: u64,
        completed_at: Instant,
    },
}

/// What a keyed request finally answered.
#[derive(Debug)]
pub(crate) enum Outcome {
    /// The handler answered.
    Answered(StoredResponse),
    /// The handler ended without answering (it panicked). Its effect is
    /// unknown, so every repeat is refused rather than applied again.
    Abandoned,
}

/// A complete response, kept for replay.
#[derive(Debug)]
pub(crate) struct StoredResponse {
    status: StatusCode,
    headers: HeaderMap,
    body: Bytes,
}

impl StoredResponse {
    fn weight(&self) -> usize {
        self.headers
            .iter()
            .map(|(name, value)| name.as_str().len() + value.len())
            .sum::<usize>()
            + self.body.len()
    }
}

/// SHA-256 over everything that makes two requests the same request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Fingerprint([u8; 32]);

impl Fingerprint {
    fn of(method: &Method, target: &str, if_match: Option<&HeaderValue>, body: &[u8]) -> Self {
        let mut hasher = Sha256::new();
        for part in [
            method.as_str().as_bytes(),
            target.as_bytes(),
            if_match.map_or(&[][..], HeaderValue::as_bytes),
            body,
        ] {
            // Length-prefixed, so no two different splits hash alike.
            hasher.update(u64::try_from(part.len()).unwrap_or(u64::MAX).to_le_bytes());
            hasher.update(part);
        }
        Self(hasher.finalize().into())
    }
}

/// What the store tells a keyed request to do.
pub(crate) enum Claim {
    /// First sighting: run the request and record its answer.
    Execute(InFlight),
    /// The original is still running: wait for its answer.
    Wait(watch::Receiver<Option<Arc<Outcome>>>),
    /// Answered before: replay it.
    Replay(Arc<Outcome>),
    /// The key already names a different request.
    Mismatch,
}

impl IdempotencyStore {
    #[cfg(test)]
    pub(crate) fn with_retention(retention: Retention) -> Self {
        Self {
            inner: Arc::default(),
            retention,
        }
    }

    fn locked(&self) -> std::sync::MutexGuard<'_, StoreInner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Decide what a request carrying `key` does, registering it as in
    /// flight when it is the first to carry the key.
    pub(crate) fn claim(&self, key: &str, fingerprint: Fingerprint) -> Claim {
        let mut inner = self.locked();
        // Read under the lock, so completion times stay in deque order.
        let now = Instant::now();
        inner.expire(now, self.retention.age);
        let expired = match inner.entries.get(key) {
            Some(Entry::InFlight {
                fingerprint: held,
                done,
            }) => {
                return if *held == fingerprint {
                    Claim::Wait(done.clone())
                } else {
                    Claim::Mismatch
                };
            }
            Some(Entry::Completed {
                fingerprint: held,
                outcome,
                completed_at,
                ..
            }) => {
                if now.saturating_duration_since(*completed_at) < self.retention.age {
                    return if *held == fingerprint {
                        Claim::Replay(Arc::clone(outcome))
                    } else {
                        Claim::Mismatch
                    };
                }
                true
            }
            None => false,
        };
        if expired {
            inner.forget(key);
        }

        let key: Arc<str> = Arc::from(key);
        let (sender, done) = watch::channel(None);
        inner
            .entries
            .insert(Arc::clone(&key), Entry::InFlight { fingerprint, done });
        Claim::Execute(InFlight {
            store: self.clone(),
            key,
            fingerprint,
            sender,
            settled: false,
        })
    }

    #[cfg(test)]
    pub(crate) fn completed_len(&self) -> usize {
        self.locked().completed_count
    }

    #[cfg(test)]
    pub(crate) fn stored_bytes(&self) -> usize {
        self.locked().stored_bytes
    }

    fn settle(&self, key: &Arc<str>, fingerprint: Fingerprint, outcome: Arc<Outcome>) {
        let weight = match outcome.as_ref() {
            Outcome::Answered(response) => response.weight(),
            Outcome::Abandoned => 0,
        };
        let mut inner = self.locked();
        let now = Instant::now();
        let sequence = inner.next_sequence;
        inner.next_sequence = inner.next_sequence.wrapping_add(1);
        inner.entries.insert(
            Arc::clone(key),
            Entry::Completed {
                fingerprint,
                outcome,
                sequence,
                completed_at: now,
            },
        );
        inner.completed.push_back(CompletedRecord {
            key: Arc::clone(key),
            sequence,
            completed_at: now,
        });
        inner.completed_count += 1;
        inner.stored_bytes = inner.stored_bytes.saturating_add(weight);
        inner.expire(now, self.retention.age);
        inner.shrink_to(self.retention);
    }
}

impl StoreInner {
    /// Drop completed answers older than `age`, oldest first.
    fn expire(&mut self, now: Instant, age: Duration) {
        while let Some(front) = self.completed.front() {
            if now.saturating_duration_since(front.completed_at) < age {
                break;
            }
            self.evict_front();
        }
    }

    /// Drop the oldest completed answers until both bounds hold.
    fn shrink_to(&mut self, retention: Retention) {
        while self.completed_count > retention.entries || self.stored_bytes > retention.bytes {
            if !self.evict_front() {
                break;
            }
        }
    }

    /// Pop the oldest completion record, removing its entry when the record
    /// still describes it. Answers whether anything was left to pop.
    fn evict_front(&mut self) -> bool {
        let Some(record) = self.completed.pop_front() else {
            return false;
        };
        let current = matches!(
            self.entries.get(&record.key),
            Some(Entry::Completed { sequence, .. }) if *sequence == record.sequence
        );
        if current {
            self.forget(&record.key);
        }
        true
    }

    /// Remove a completed entry and release what it held. An in-flight
    /// entry is never forgotten.
    fn forget(&mut self, key: &str) {
        if matches!(self.entries.get(key), Some(Entry::Completed { .. }))
            && let Some(Entry::Completed { outcome, .. }) = self.entries.remove(key)
        {
            self.completed_count = self.completed_count.saturating_sub(1);
            if let Outcome::Answered(response) = outcome.as_ref() {
                self.stored_bytes = self.stored_bytes.saturating_sub(response.weight());
            }
        }
    }
}

/// A claim on running one keyed request. Settling it records the answer
/// and wakes every duplicate waiting on it; dropping it unsettled (a
/// panicking handler) records the request as abandoned instead, so no
/// waiter hangs and no retry applies a write of unknown effect.
pub(crate) struct InFlight {
    store: IdempotencyStore,
    key: Arc<str>,
    fingerprint: Fingerprint,
    sender: watch::Sender<Option<Arc<Outcome>>>,
    settled: bool,
}

impl InFlight {
    pub(crate) fn settle(mut self, outcome: Outcome) -> Arc<Outcome> {
        self.settled = true;
        let outcome = Arc::new(outcome);
        self.store
            .settle(&self.key, self.fingerprint, Arc::clone(&outcome));
        self.sender.send_replace(Some(Arc::clone(&outcome)));
        outcome
    }
}

impl Drop for InFlight {
    fn drop(&mut self) {
        if self.settled {
            return;
        }
        let outcome = Arc::new(Outcome::Abandoned);
        self.store
            .settle(&self.key, self.fingerprint, Arc::clone(&outcome));
        self.sender.send_replace(Some(outcome));
    }
}

/// The routes an `Idempotency-Key` is honored on, read from the OpenAPI
/// document so the published contract is the enforced one.
#[derive(Debug, Clone, Default)]
pub(crate) struct IdempotentRoutes {
    /// Path template (as the router matches it, without the `/api/v1`
    /// mount) to the methods that opt in there.
    routes: HashMap<String, Vec<Method>>,
}

impl IdempotentRoutes {
    /// Collect every operation that documents the `Idempotency-Key` header.
    pub(crate) fn from_openapi(document: &utoipa::openapi::OpenApi) -> Self {
        use utoipa::openapi::RefOr;
        use utoipa::openapi::path::ParameterIn;

        let mut routes: HashMap<String, Vec<Method>> = HashMap::new();
        for (path, item) in &document.paths.paths {
            let operations = [
                (Method::GET, &item.get),
                (Method::PUT, &item.put),
                (Method::POST, &item.post),
                (Method::DELETE, &item.delete),
                (Method::PATCH, &item.patch),
            ];
            for (method, operation) in operations {
                let declares = operation.as_ref().is_some_and(|operation| {
                    operation.parameters.iter().flatten().any(|parameter| {
                        matches!(
                            parameter,
                            RefOr::T(parameter)
                                if parameter.parameter_in == ParameterIn::Header
                                    && parameter.name.eq_ignore_ascii_case(DOCUMENTED_HEADER)
                        )
                    })
                });
                if declares {
                    routes.entry(path.clone()).or_default().push(method);
                }
            }
        }
        Self { routes }
    }

    fn honors(&self, method: &Method, matched: &str) -> bool {
        let path = matched.strip_prefix("/api/v1").unwrap_or(matched);
        self.routes
            .get(path)
            .is_some_and(|methods| methods.contains(method))
    }

    #[cfg(test)]
    pub(crate) fn of(routes: &[(Method, &str)]) -> Self {
        let mut map: HashMap<String, Vec<Method>> = HashMap::new();
        for (method, path) in routes {
            map.entry((*path).to_owned())
                .or_default()
                .push(method.clone());
        }
        Self { routes: map }
    }
}

/// The middleware's state: the run's store and the routes that honor it.
#[derive(Clone)]
pub(crate) struct IdempotencyLayer {
    pub(crate) store: IdempotencyStore,
    pub(crate) routes: Arc<IdempotentRoutes>,
}

/// Apply `Idempotency-Key` semantics to the opted-in routes.
pub(crate) async fn guard(
    State(layer): State<IdempotencyLayer>,
    request: Request,
    next: Next,
) -> Response {
    let Some(raw_key) = request.headers().get(&IDEMPOTENCY_KEY) else {
        return next.run(request).await;
    };
    let honored = request
        .extensions()
        .get::<MatchedPath>()
        .is_some_and(|matched| layer.routes.honors(request.method(), matched.as_str()));
    if !honored {
        // The draft lets a resource ignore a key it does not support.
        return next.run(request).await;
    }
    let key = match parse_key(raw_key) {
        Ok(key) => key,
        Err(error) => return error.into_response(),
    };

    let (parts, body) = match super::extract::buffer_request(request).await {
        Ok(buffered) => buffered,
        Err(error) => return error.into_response(),
    };
    let target = parts
        .uri
        .path_and_query()
        .map_or_else(|| parts.uri.path(), |target| target.as_str());
    let fingerprint = Fingerprint::of(
        &parts.method,
        target,
        parts.headers.get(header::IF_MATCH),
        &body,
    );

    match layer.store.claim(&key, fingerprint) {
        Claim::Mismatch => DomainError::IdempotencyKeyMismatch.into_response(),
        Claim::Replay(outcome) => respond(&outcome, true),
        Claim::Wait(done) => match wait(done).await {
            Some(outcome) => respond(&outcome, true),
            None => abandoned(true),
        },
        Claim::Execute(in_flight) => {
            let request = Request::from_parts(parts, Body::from(body));
            // Spawned, so the write and its record finish together even
            // when the caller hangs up between them.
            let task = crate::audit_log::spawn_attributed(
                async move {
                    let response = next.run(request).await;
                    in_flight.settle(buffer_response(response).await)
                }
                .in_current_span(),
            );
            match task.await {
                Ok(outcome) => respond(&outcome, false),
                Err(_) => abandoned(false),
            }
        }
    }
}

/// Validate a key: printable ASCII, at most [`IDEMPOTENCY_KEY_MAX_LEN`],
/// written bare or as the draft's quoted structured-field string.
fn parse_key(raw: &HeaderValue) -> Result<String, DomainError> {
    let text = raw
        .to_str()
        .map_err(|_| DomainError::malformed("Idempotency-Key must be printable ASCII"))?
        .trim();
    let key = text
        .strip_prefix('"')
        .and_then(|inner| inner.strip_suffix('"'))
        .unwrap_or(text);
    if key.is_empty() {
        return Err(DomainError::malformed("Idempotency-Key must not be empty"));
    }
    if key.len() > IDEMPOTENCY_KEY_MAX_LEN {
        return Err(DomainError::malformed(format!(
            "Idempotency-Key must be at most {IDEMPOTENCY_KEY_MAX_LEN} characters"
        )));
    }
    if !key
        .bytes()
        .all(|byte| byte.is_ascii_graphic() && byte != b'"')
    {
        return Err(DomainError::malformed(
            "Idempotency-Key must be printable ASCII without spaces or quotes",
        ));
    }
    Ok(key.to_owned())
}

async fn wait(mut done: watch::Receiver<Option<Arc<Outcome>>>) -> Option<Arc<Outcome>> {
    done.wait_for(Option::is_some)
        .await
        .ok()
        .and_then(|outcome| outcome.clone())
}

async fn buffer_response(response: Response) -> Outcome {
    let (parts, body) = response.into_parts();
    match axum::body::to_bytes(body, usize::MAX).await {
        Ok(body) => Outcome::Answered(StoredResponse {
            status: parts.status,
            headers: parts.headers,
            body,
        }),
        Err(error) => {
            tracing::error!(%error, "Idempotent response body failed to buffer");
            Outcome::Abandoned
        }
    }
}

fn respond(outcome: &Outcome, replayed: bool) -> Response {
    let Outcome::Answered(stored) = outcome else {
        return abandoned(replayed);
    };
    let mut response = (stored.status, Body::from(stored.body.clone())).into_response();
    *response.headers_mut() = stored.headers.clone();
    if replayed {
        response
            .headers_mut()
            .insert(IDEMPOTENCY_REPLAYED, HeaderValue::from_static("true"));
    }
    response
}

/// The answer for a key whose original request ended without one.
fn abandoned(replayed: bool) -> Response {
    let mut response = DomainError::Internal(anyhow::anyhow!(
        "the request this Idempotency-Key names ended without an answer"
    ))
    .into_response();
    if replayed {
        response
            .headers_mut()
            .insert(IDEMPOTENCY_REPLAYED, HeaderValue::from_static("true"));
    }
    response
}

#[cfg(test)]
mod tests {
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
    fn keys_are_opaque_printable_ascii() {
        let parse = |raw: &'static str| parse_key(&HeaderValue::from_static(raw));
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
}
