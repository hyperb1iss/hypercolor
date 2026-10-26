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
//!   true`. A repeat that arrives while the original is still running waits
//!   for it and gets the same; the draft also allows answering `409` there,
//!   but a client retrying a lost reply wants the answer, not a second
//!   retry loop.
//! - The same key on a different request (method, path, `If-Match`, or
//!   body) answers `422 idempotency_key_mismatch`.
//! - A request without the header costs one header lookup and is otherwise
//!   untouched, so the CLI, Home Assistant, and MCP behave as before.
//!
//! Every completed answer is kept, refusals included: a `412` for a stale
//! fence is as definitive as a `200`, and only a stored answer guarantees a
//! retry cannot apply what the original did not.
//!
//! Keys are scoped per credential, as the draft recommends, when the
//! request carries a recognized one; callers without one share a scope.
//! Which routes honor the key is declared in the OpenAPI document: an
//! operation that documents the optional `Idempotency-Key` header gets this
//! behavior, so the published contract and the enforced one cannot drift.
//! A keyed body is buffered under the API-wide body ceiling, so an opted-in
//! route must not rely on a larger route-level limit. Keys live in memory
//! for one daemon run, bounded by [`IDEMPOTENCY_RETENTION_ENTRIES`],
//! [`IDEMPOTENCY_RETENTION_BYTES`], and [`IDEMPOTENCY_RETENTION_AGE`]; the
//! run epoch on every response tells a client when its keys stopped
//! meaning anything.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use axum::body::{Body, Bytes};
use axum::extract::{MatchedPath, Request, State};
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use hypercolor_types::api::envelope::{
    IDEMPOTENCY_KEY_HEADER, IDEMPOTENCY_KEY_MAX_LEN, IDEMPOTENCY_REPLAYED_HEADER,
};
use sha2::{Digest, Sha256};
use tokio::sync::watch;
use tokio::time::Instant;
use tracing::Instrument;

use super::security::CredentialGrant;
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

/// Most response bytes kept at once across every completed answer. The
/// newest answer is always kept, so one answer larger than this still
/// replays until the next one arrives.
pub(crate) const IDEMPOTENCY_RETENTION_BYTES: usize = 32 * 1024 * 1024;

/// How long a completed answer stays replayable.
pub(crate) const IDEMPOTENCY_RETENTION_AGE: Duration = Duration::from_hours(1);

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

    /// Drop the oldest completed answers until both bounds hold, never
    /// the newest one, so an answer larger than the byte bound still
    /// replays for the retry most likely to follow it.
    fn shrink_to(&mut self, retention: Retention) {
        while self.completed_count > retention.entries.max(1)
            || (self.stored_bytes > retention.bytes && self.completed_count > 1)
        {
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
        Ok(key) => scoped_key(request.extensions().get::<CredentialGrant>(), key),
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

/// Place a client's key in its credential's scope, so two credentials that
/// happen to pick the same key never see each other's answers.
fn scoped_key(grant: Option<&CredentialGrant>, key: &str) -> String {
    let scope = grant.map_or("", CredentialGrant::credential_id);
    format!("{}:{scope}:{key}", scope.len())
}

/// Validate a key: visible ASCII with no spaces or quotes, at most
/// [`IDEMPOTENCY_KEY_MAX_LEN`] characters, written bare or as the draft's
/// quoted structured-field string.
fn parse_key(raw: &HeaderValue) -> Result<&str, DomainError> {
    let text = raw
        .to_str()
        .map_err(|_| DomainError::malformed("Idempotency-Key must be visible ASCII"))?
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
            "Idempotency-Key must be visible ASCII without spaces or quotes",
        ));
    }
    Ok(key)
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
mod tests;
