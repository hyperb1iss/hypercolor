//! Sending one scene write under an idempotency key until the daemon
//! answers it.
//!
//! The key names one logical write. Every retry of that write goes out
//! under the same key, and the daemon applies a key at most once: a retry
//! of a request whose reply was lost gets the first request's answer back,
//! waiting for it when that request is still under way, rather than a
//! second apply. Whatever the daemon answers is final for the key.
//!
//! Two answers are no answer: a request with no answer within
//! [`crate::api::client::KEYED_WRITE_DEADLINE`], which is given up on, and
//! a `503` the daemon gives when it cannot take the write right now (it
//! never records the key then). Both go out again under the same key, so
//! a request given up on that commits late is still applied once.
//!
//! Keys live for one daemon run. An answer from another run (the daemon
//! restarted while the write was out) says nothing about what the first
//! request did, so the caller settles that write from a fresh read of the
//! scene instead.
//!
//! Within a run the daemon keeps an answer for a while, not for ever: an
//! hour, or less once it holds many, though never while its request is
//! still under way. A replay says so (`Idempotency-Replayed: true`). A
//! retry it answers without that ran as a new request, and when it
//! refuses one because the scene moved past the write's fence (or holds no
//! such zone), the first request may be what moved it. That refusal is
//! stored under the key in turn, so a later retry can get it back as a
//! replay. Neither says what the first request did, and the caller settles
//! the write from a fresh read too (see [`tells`]).

use std::future::Future;
use std::time::Duration;

use crate::api::client::{KeyedResponse, sleep};
use crate::api::{ApiError, ApiResult, MutationOutcome};

/// The daemon's error code for a write it cannot take right now, as when
/// too many keyed writes are in flight. It keeps nothing under the key
/// then, so the same write may go out again.
pub const TRY_AGAIN_CODE: &str = "service_unavailable";

/// The waits before each retry of a write no answer came back for.
pub const RETRY_DELAYS: [Duration; 3] = [
    Duration::from_millis(250),
    Duration::from_secs(1),
    Duration::from_secs(3),
];

/// A key for a new logical write.
#[must_use]
pub fn new_key() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// What a write sent under a key came to.
#[derive(Debug, Clone)]
pub enum Answer<T> {
    /// The daemon answered in the run the write was meant for. Final: a
    /// retry under the same key would get this same answer.
    Final {
        status: u16,
        outcome: ApiResult<MutationOutcome<T>>,
    },
    /// Another daemon run answered, so whatever the first request did, its
    /// key means nothing here.
    OtherRun,
    /// A refusal for what the scene holds that may not be the first
    /// request's: the daemon may have let that answer go and run a retry
    /// afresh, against a scene the first request changed itself. Whether
    /// the first request landed is unknown.
    Forgotten,
    /// No answer came back from the daemon.
    Lost(ApiError),
}

impl<T> Answer<T> {
    /// The answer as the write's plain result. An answer from another run,
    /// or none at all, leaves the write's outcome unknown, so it reads as
    /// an error.
    pub fn into_result(self) -> ApiResult<MutationOutcome<T>> {
        match self {
            Self::Final { outcome, .. } => outcome,
            Self::OtherRun => Err(ApiError::Network(
                "the daemon restarted before it answered".to_owned(),
            )),
            Self::Forgotten => Err(ApiError::Network(
                "the daemon no longer knows whether the write went through".to_owned(),
            )),
            Self::Lost(error) => Err(error),
        }
    }
}

/// What `response`, to a write meant for daemon run `run`, amounts to.
/// `unanswered` counts the attempts of the write that went out before
/// under the same key with no answer coming back. An empty `run` means
/// the daemon names no runs (it predates run epochs), so any response is
/// its answer. Otherwise only a response that names a run is the daemon's
/// own: one without (a proxy's `502` while the daemon is down) is no
/// answer at all.
#[must_use]
pub fn classify<T>(run: &str, unanswered: u32, response: KeyedResponse<T>) -> Answer<T> {
    let Some(status) = response.status else {
        return Answer::Lost(lost_error(response.outcome, None));
    };
    match response.run.as_deref() {
        Some(answered) if !run.is_empty() && answered != run => return Answer::OtherRun,
        None if !run.is_empty() => {
            return Answer::Lost(lost_error(response.outcome, Some(status)));
        }
        _ => {}
    }
    if !tells(unanswered, response.replayed, status) {
        return Answer::Forgotten;
    }
    final_answer(status, response.outcome)
}

/// Whether an answer with `status` says what became of a write after
/// `unanswered` of its attempts went out with no answer coming back, given
/// whether the daemon `replayed` it from what it stored under the key.
///
/// The first attempt's answer always does. After that, an apply does (had
/// the first request landed, its fence would refuse every later one), and
/// so does a refusal that does not turn on what the scene holds. One that
/// does ([`may_be_its_own_doing`]) says what the first request did only
/// as a replay to the first retry, which can only be serving the first
/// request's own answer. Run afresh, a retry may be meeting what the first
/// request did; and a later retry may get such a refusal back as a replay.
#[must_use]
pub fn tells(unanswered: u32, replayed: bool, status: u16) -> bool {
    unanswered == 0 || !may_be_its_own_doing(status) || (replayed && unanswered == 1)
}

/// Whether a refusal with `status` depends on what the scene holds now: a
/// fence the scene moved past (`412`), a commit another write beat
/// (`409`), a zone that is gone (`404`).
#[must_use]
pub fn may_be_its_own_doing(status: u16) -> bool {
    matches!(status, 404 | 409 | 412)
}

/// The daemon's own answer, unless it is one that asks for the write again.
fn final_answer<T>(status: u16, outcome: ApiResult<MutationOutcome<T>>) -> Answer<T> {
    match outcome {
        Err(error @ ApiError::Http { status: 503, .. }) if try_again(&error) => Answer::Lost(error),
        outcome => Answer::Final { status, outcome },
    }
}

fn try_again(error: &ApiError) -> bool {
    matches!(error, ApiError::Http { code: Some(code), .. } if code == TRY_AGAIN_CODE)
}

fn lost_error<T>(outcome: ApiResult<MutationOutcome<T>>, status: Option<u16>) -> ApiError {
    match (outcome, status) {
        (Err(error), None) => error,
        (_, Some(status)) => {
            ApiError::Network(format!("no answer from the daemon (HTTP {status})"))
        }
        (Ok(_), None) => ApiError::Network("no answer from the daemon".to_owned()),
    }
}

/// Send a new write meant for daemon run `run` until the daemon answers
/// it. `attempt` sends it under its key; while nothing comes back it goes
/// out again, under that same key, after each wait in [`RETRY_DELAYS`].
pub async fn send<T, F, Fut>(run: &str, attempt: F) -> Answer<T>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = KeyedResponse<T>>,
{
    send_from(run, 0, attempt).await
}

/// [`send`] for a write held after going out more than once under its key
/// with no answer coming back (or with one that did not tell), to learn
/// what became of it. Only an answer that [`tells`] after that settles it.
pub async fn resend<T, F, Fut>(run: &str, attempt: F) -> Answer<T>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = KeyedResponse<T>>,
{
    send_from(run, 2, attempt).await
}

async fn send_from<T, F, Fut>(run: &str, mut unanswered: u32, mut attempt: F) -> Answer<T>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = KeyedResponse<T>>,
{
    let mut waits = RETRY_DELAYS.iter();
    loop {
        match classify(run, unanswered, attempt().await) {
            Answer::Lost(error) => match waits.next() {
                Some(delay) => {
                    unanswered += 1;
                    sleep(*delay).await;
                }
                None => return Answer::Lost(error),
            },
            answer => return answer,
        }
    }
}
