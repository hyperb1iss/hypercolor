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
            Self::Lost(error) => Err(error),
        }
    }
}

/// What `response`, to a write meant for daemon run `run`, amounts to. An
/// empty `run` means the daemon names no runs (it predates run epochs), so
/// any response is its answer. Otherwise only a response that names a run
/// is the daemon's own: one without (a proxy's `502` while the daemon is
/// down) is no answer at all.
#[must_use]
pub fn classify<T>(run: &str, response: KeyedResponse<T>) -> Answer<T> {
    let Some(status) = response.status else {
        return Answer::Lost(lost_error(response.outcome, None));
    };
    match response.run {
        Some(answered) if run.is_empty() || answered == run => {
            final_answer(status, response.outcome)
        }
        Some(_) => Answer::OtherRun,
        None if run.is_empty() => final_answer(status, response.outcome),
        None => Answer::Lost(lost_error(response.outcome, Some(status))),
    }
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

/// Send a write meant for daemon run `run` until the daemon answers it.
/// `attempt` sends it under its key; while nothing comes back it goes out
/// again, under that same key, after each wait in [`RETRY_DELAYS`].
pub async fn send<T, F, Fut>(run: &str, mut attempt: F) -> Answer<T>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = KeyedResponse<T>>,
{
    let mut waits = RETRY_DELAYS.iter();
    loop {
        match classify(run, attempt().await) {
            Answer::Lost(error) => match waits.next() {
                Some(delay) => sleep(*delay).await,
                None => return Answer::Lost(error),
            },
            answer => return answer,
        }
    }
}
