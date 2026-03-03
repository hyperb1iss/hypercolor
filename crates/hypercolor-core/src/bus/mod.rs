//! Event bus — the nervous system of Hypercolor.
//!
//! Three communication patterns coexist on the [`HypercolorBus`]:
//!
//! - **Broadcast events** — discrete state changes (device connected, effect changed).
//!   Every subscriber sees every event via `tokio::sync::broadcast`.
//! - **Frame data** — latest LED colors via `tokio::sync::watch`. Subscribers skip stale frames.
//! - **Spectrum data** — latest audio analysis via `tokio::sync::watch`. Same semantics.
//!
//! The bus is `Send + Sync`, cloneable, and entirely lock-free.

mod filter;

pub use filter::{EventFilter, FilteredEventReceiver};

use std::time::{Instant, SystemTime};

use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, watch};

use crate::types::event::{FrameData, HypercolorEvent, SpectrumData};

// ── Constants ────────────────────────────────────────────────────────────

/// Broadcast channel capacity.
///
/// 256 events handles burst scenarios (e.g., 8 devices connecting
/// simultaneously during discovery) while keeping memory usage under
/// ~128 KB. At steady-state (~10-30 events/sec), this provides
/// ~8-25 seconds of runway for a stalled subscriber.
const EVENT_CHANNEL_CAPACITY: usize = 256;

// ── TimestampedEvent ─────────────────────────────────────────────────────

/// An event wrapped with timing metadata.
///
/// The bus adds timestamps at publish time so event producers
/// don't need to worry about clocks.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimestampedEvent {
    /// ISO 8601 wall-clock timestamp with millisecond precision.
    pub timestamp: String,
    /// Monotonic millis since the bus was created (for frame correlation).
    pub mono_ms: u64,
    /// The event payload.
    #[serde(flatten)]
    pub event: HypercolorEvent,
}

// ── HypercolorBus ────────────────────────────────────────────────────────

/// The central event bus. Owns all channels and provides typed
/// subscription methods.
///
/// All channel operations are lock-free. The bus is `Send + Sync` and
/// can be shared across arbitrary tokio tasks via `Arc<HypercolorBus>`
/// or direct clone.
#[derive(Clone, Debug)]
pub struct HypercolorBus {
    /// Discrete events -- every subscriber sees every event.
    events: broadcast::Sender<TimestampedEvent>,

    /// Latest LED color data for all zones.
    frame: watch::Sender<FrameData>,

    /// Latest audio spectrum analysis data.
    spectrum: watch::Sender<SpectrumData>,

    /// Monotonic clock base for `mono_ms` timestamps.
    start_instant: Instant,
}

impl HypercolorBus {
    /// Create a new bus with default channel capacities.
    #[must_use]
    pub fn new() -> Self {
        let (events, _) = broadcast::channel(EVENT_CHANNEL_CAPACITY);
        let (frame, _) = watch::channel(FrameData::empty());
        let (spectrum, _) = watch::channel(SpectrumData::empty());

        Self {
            events,
            frame,
            spectrum,
            start_instant: Instant::now(),
        }
    }

    // ── Publishing ───────────────────────────────────────────────────

    /// Publish a discrete event. Timestamp is added automatically.
    ///
    /// Non-blocking -- if no subscribers exist, the event is silently dropped.
    pub fn publish(&self, event: HypercolorEvent) {
        let timestamped = TimestampedEvent {
            timestamp: format_iso8601_now(),
            mono_ms: self.mono_ms(),
            event,
        };
        // Ignore send errors -- they mean no subscribers exist.
        let _ = self.events.send(timestamped);
    }

    // ── Subscribing ──────────────────────────────────────────────────

    /// Subscribe to all discrete events (unfiltered).
    #[must_use]
    pub fn subscribe_all(&self) -> broadcast::Receiver<TimestampedEvent> {
        self.events.subscribe()
    }

    /// Subscribe to discrete events matching a filter.
    ///
    /// Returns a [`FilteredEventReceiver`] that silently consumes
    /// non-matching events.
    #[must_use]
    pub fn subscribe_filtered(&self, filter: EventFilter) -> FilteredEventReceiver {
        FilteredEventReceiver::new(self.events.subscribe(), filter)
    }

    /// Access the frame data watch sender (for the render loop to publish).
    #[must_use]
    pub fn frame_sender(&self) -> &watch::Sender<FrameData> {
        &self.frame
    }

    /// Subscribe to frame data (latest-value semantics).
    #[must_use]
    pub fn frame_receiver(&self) -> watch::Receiver<FrameData> {
        self.frame.subscribe()
    }

    /// Access the spectrum data watch sender (for the audio processor to publish).
    #[must_use]
    pub fn spectrum_sender(&self) -> &watch::Sender<SpectrumData> {
        &self.spectrum
    }

    /// Subscribe to spectrum data (latest-value semantics).
    #[must_use]
    pub fn spectrum_receiver(&self) -> watch::Receiver<SpectrumData> {
        self.spectrum.subscribe()
    }

    /// Number of active broadcast subscribers.
    #[must_use]
    pub fn subscriber_count(&self) -> usize {
        self.events.receiver_count()
    }

    // ── Internal ─────────────────────────────────────────────────────

    /// Monotonic millis since bus creation.
    fn mono_ms(&self) -> u64 {
        let elapsed = self.start_instant.elapsed();
        // Duration::as_millis() returns u128; bus uptime won't exceed u64.
        #[expect(clippy::cast_possible_truncation, clippy::as_conversions)]
        let ms = elapsed.as_millis() as u64;
        ms
    }
}

impl Default for HypercolorBus {
    fn default() -> Self {
        Self::new()
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────

/// Format the current wall-clock time as ISO 8601 with millisecond precision.
///
/// Uses `SystemTime` to avoid pulling in `chrono`. The format is
/// `YYYY-MM-DDTHH:MM:SS.mmmZ` (always UTC).
fn format_iso8601_now() -> String {
    let now = SystemTime::now();
    let duration = now
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();

    let total_secs = duration.as_secs();
    let millis = duration.subsec_millis();

    // Break epoch seconds into calendar components (UTC).
    let (year, month, day, hour, minute, second) = epoch_to_utc(total_secs);

    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{millis:03}Z")
}

/// Convert Unix epoch seconds to (year, month, day, hour, minute, second) in UTC.
///
/// Handles leap years correctly. Not designed for dates before 1970.
#[expect(clippy::cast_possible_truncation, clippy::as_conversions)]
fn epoch_to_utc(epoch_secs: u64) -> (u32, u32, u32, u32, u32, u32) {
    let secs_per_day: u64 = 86400;
    let days = epoch_secs / secs_per_day;
    let day_secs = epoch_secs % secs_per_day;

    let hour = (day_secs / 3600) as u32;
    let minute = ((day_secs % 3600) / 60) as u32;
    let second = (day_secs % 60) as u32;

    // Civil date from day count (algorithm from Howard Hinnant).
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };

    (y as u32, m as u32, d as u32, hour, minute, second)
}

// ── Re-exports for convenience ───────────────────────────────────────────
// All event types (EventCategory, EventPriority, FrameData, HypercolorEvent,
// SpectrumData) are imported above and used internally. Consumers should
// import directly from `hypercolor_types::event`.
