//! The clock check every bounded primitive shares.

use std::future::Future;

use tokio::time::{Instant, timeout_at};

/// Run `operation` until `deadline`, or `None` if the deadline won.
///
/// `timeout_at` alone does not make a deadline absolute, because it polls
/// its future before it looks at the clock: a future that is always ready
/// never sees the timer, and one that wakes late beats it. So the clock is
/// read on both sides, and whatever finishes at or past the deadline
/// missed it.
///
/// Wrap one I/O step at a time. A future that loops internally, like
/// `write_all`, can stay ready across the deadline between those reads.
pub(crate) async fn until_deadline<F: Future>(
    deadline: Instant,
    operation: F,
) -> Option<F::Output> {
    if Instant::now() >= deadline {
        return None;
    }
    let output = timeout_at(deadline, operation).await.ok()?;
    (Instant::now() < deadline).then_some(output)
}
