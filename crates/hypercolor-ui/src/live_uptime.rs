//! Daemon uptime that keeps advancing between status fetches.
//!
//! The status snapshot carries the daemon's uptime as of the fetch and is
//! refetched only when the websocket reconnects, so views anchor it to the
//! moment it arrived and advance it with a view-owned display clock.

use std::time::Duration;

use hypercolor_leptos_ext::prelude::{now_ms, set_interval};
use leptos::prelude::*;

/// Live daemon uptime in seconds, anchored to `fetched_uptime_seconds` as of
/// this call and advanced locally once a second. No network reads are needed.
pub fn use_live_uptime(fetched_uptime_seconds: u64) -> Signal<u64> {
    let fetched_at_ms = now_ms();
    let now = RwSignal::new(fetched_at_ms);
    let timer = StoredValue::new_local(Some(set_interval(Duration::from_secs(1), move || {
        now.set(now_ms());
    })));
    on_cleanup(move || {
        timer.update_value(|timer| {
            timer.take();
        });
    });
    Signal::derive(move || {
        advanced_uptime_seconds(fetched_uptime_seconds, fetched_at_ms, now.get())
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
