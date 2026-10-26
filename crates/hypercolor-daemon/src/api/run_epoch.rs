//! Name the daemon run that answered, on every HTTP response.
//!
//! Scene revisions are comparable only within one run, and an `ETag` or a
//! `412` carries a revision with no other context. Stamping the run epoch
//! beside every answer lets a client notice a restart from any response,
//! a zone write reply or a refused fence included, without a second read.

use axum::extract::{Request, State};
use axum::http::{HeaderName, HeaderValue};
use axum::middleware::Next;
use axum::response::Response;
use hypercolor_types::api::envelope::RUN_EPOCH_HEADER;

/// The header [`stamp`] writes.
pub(super) const RUN_EPOCH: HeaderName = HeaderName::from_static(RUN_EPOCH_HEADER);

/// Attach this run's epoch to the response.
pub(super) async fn stamp(
    State(epoch): State<HeaderValue>,
    request: Request,
    next: Next,
) -> Response {
    let mut response = next.run(request).await;
    response.headers_mut().insert(RUN_EPOCH, epoch);
    response
}
