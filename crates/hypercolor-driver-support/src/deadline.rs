//! The clock check every bounded primitive shares.

use std::future::Future;

use tokio::time::{Instant, timeout_at};

/// Run `operation` until `deadline`, or `None` if the deadline won.
///
/// `timeout_at` polls its future before it looks at the clock, so a peer
/// that always has data ready would never see the deadline. Checking first
/// is what makes the deadline absolute.
pub(crate) async fn until_deadline<F: Future>(
    deadline: Instant,
    operation: F,
) -> Option<F::Output> {
    if Instant::now() >= deadline {
        return None;
    }
    timeout_at(deadline, operation).await.ok()
}
