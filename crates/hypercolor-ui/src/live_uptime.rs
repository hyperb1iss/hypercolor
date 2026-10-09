//! Daemon uptime that keeps advancing between status fetches.
//!
//! The status snapshot carries the daemon's uptime as of the fetch and is
//! refetched only when the websocket reconnects, so views anchor it to the
//! moment it arrived and advance it locally. Metrics messages arrive a few
//! times a second, which makes their tick the clock for that advance.

use hypercolor_leptos_ext::prelude::now_ms;
use leptos::prelude::*;

use crate::app::WsContext;

/// Live daemon uptime in seconds, anchored to `fetched_uptime_seconds` as of
/// this call and re-derived on every metrics message.
pub fn use_live_uptime(fetched_uptime_seconds: u64) -> Signal<u64> {
    let fetched_at_ms = now_ms();
    let metrics_tick = expect_context::<WsContext>().metrics_tick;
    Signal::derive(move || {
        metrics_tick.track();
        advanced_uptime_seconds(fetched_uptime_seconds, fetched_at_ms, now_ms())
    })
}

/// Uptime advanced from a fetched value by the whole seconds elapsed since
/// the fetch. A clock reading earlier than the fetch adds nothing.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "elapsed milliseconds are clamped non-negative and floored to whole seconds"
)]
#[must_use]
pub fn advanced_uptime_seconds(
    fetched_uptime_seconds: u64,
    fetched_at_ms: f64,
    now_ms: f64,
) -> u64 {
    let elapsed_seconds = ((now_ms - fetched_at_ms).max(0.0) / 1000.0).floor() as u64;
    fetched_uptime_seconds.saturating_add(elapsed_seconds)
}

/// Compact uptime label: seconds under a minute, minutes under an hour, then
/// hours and minutes.
#[must_use]
pub fn format_uptime(seconds: u64) -> String {
    if seconds < 60 {
        format!("{seconds}s")
    } else if seconds < 3600 {
        format!("{}m", seconds / 60)
    } else {
        format!("{}h {}m", seconds / 3600, (seconds % 3600) / 60)
    }
}
