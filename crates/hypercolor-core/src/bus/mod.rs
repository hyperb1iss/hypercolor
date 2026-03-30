//! Event bus — the nervous system of Hypercolor.
//!
//! Three communication patterns coexist on the [`HypercolorBus`]:
//!
//! - **Broadcast events** — discrete state changes (device connected, effect changed).
//!   Every subscriber sees every event via `tokio::sync::broadcast`.
//! - **Frame data** — latest LED colors via `tokio::sync::watch`. Subscribers skip stale frames.
//! - **Spectrum data** — latest audio analysis via `tokio::sync::watch`. Same semantics.
//! - **Canvas previews** — latest render and screen-source canvases via `tokio::sync::watch`.
//!
//! The bus is `Send + Sync`, cloneable, and entirely lock-free.

mod filter;

pub use filter::{EventFilter, FilteredEventReceiver};

use std::fmt;
use std::sync::Arc;
use std::time::{Instant, SystemTime};

use serde::{Serialize, Serializer};
use tokio::sync::{broadcast, watch};

use crate::types::canvas::Canvas;
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EventTimestamp(u64);

impl EventTimestamp {
    #[must_use]
    pub fn now() -> Self {
        let duration = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default();

        #[expect(clippy::cast_possible_truncation, clippy::as_conversions)]
        let epoch_millis = duration.as_millis() as u64;
        Self(epoch_millis)
    }

    #[must_use]
    pub const fn from_epoch_millis(epoch_millis: u64) -> Self {
        Self(epoch_millis)
    }

    #[must_use]
    pub const fn epoch_millis(self) -> u64 {
        self.0
    }
}

impl fmt::Display for EventTimestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let total_secs = self.0 / 1_000;
        let millis = self.0 % 1_000;
        let (year, month, day, hour, minute, second) = epoch_to_utc(total_secs);

        write!(
            f,
            "{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{millis:03}Z"
        )
    }
}

impl Serialize for EventTimestamp {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.collect_str(self)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct TimestampedEvent {
    /// ISO 8601 wall-clock timestamp with millisecond precision.
    pub timestamp: EventTimestamp,
    /// Monotonic millis since the bus was created (for frame correlation).
    pub mono_ms: u64,
    /// The event payload.
    #[serde(flatten)]
    pub event: HypercolorEvent,
}

/// Raw render canvas payload for WebSocket preview streaming.
///
/// Always stores RGBA bytes regardless of downstream transport format.
#[derive(Clone, Debug)]
pub struct CanvasFrame {
    /// Monotonically increasing frame counter.
    pub frame_number: u32,
    /// Millis since daemon start.
    pub timestamp_ms: u32,
    /// Canvas width in pixels.
    pub width: u32,
    /// Canvas height in pixels.
    pub height: u32,
    rgba: Arc<Vec<u8>>,
}

impl CanvasFrame {
    /// Creates an empty frame payload.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            frame_number: 0,
            timestamp_ms: 0,
            width: 0,
            height: 0,
            rgba: Arc::new(Vec::new()),
        }
    }

    /// Snapshot a canvas for transport.
    #[must_use]
    pub fn from_canvas(canvas: &Canvas, frame_number: u32, timestamp_ms: u32) -> Self {
        Self {
            frame_number,
            timestamp_ms,
            width: canvas.width(),
            height: canvas.height(),
            rgba: Arc::new(canvas.as_rgba_bytes().to_vec()),
        }
    }

    /// Consume a canvas without cloning its RGBA backing buffer.
    #[must_use]
    pub fn from_owned_canvas(canvas: Canvas, frame_number: u32, timestamp_ms: u32) -> Self {
        Self {
            frame_number,
            timestamp_ms,
            width: canvas.width(),
            height: canvas.height(),
            rgba: Arc::new(canvas.into_rgba_bytes()),
        }
    }

    /// RGBA canvas bytes in row-major order.
    #[must_use]
    pub fn rgba_bytes(&self) -> &[u8] {
        self.rgba.as_slice()
    }
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

    /// Latest render canvas snapshot.
    canvas: watch::Sender<CanvasFrame>,

    /// Latest screen-source canvas snapshot.
    screen_canvas: watch::Sender<CanvasFrame>,

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
        let (canvas, _) = watch::channel(CanvasFrame::empty());
        let (screen_canvas, _) = watch::channel(CanvasFrame::empty());

        Self {
            events,
            frame,
            spectrum,
            canvas,
            screen_canvas,
            start_instant: Instant::now(),
        }
    }

    // ── Publishing ───────────────────────────────────────────────────

    /// Publish a discrete event. Timestamp is added automatically.
    ///
    /// Non-blocking -- if no subscribers exist, the event is silently dropped.
    pub fn publish(&self, event: HypercolorEvent) {
        let timestamped = TimestampedEvent {
            timestamp: EventTimestamp::now(),
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

    /// Number of active frame watch receivers.
    #[must_use]
    pub fn frame_receiver_count(&self) -> usize {
        self.frame.receiver_count()
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

    /// Number of active spectrum watch receivers.
    #[must_use]
    pub fn spectrum_receiver_count(&self) -> usize {
        self.spectrum.receiver_count()
    }

    /// Access the canvas watch sender (for render preview publication).
    #[must_use]
    pub fn canvas_sender(&self) -> &watch::Sender<CanvasFrame> {
        &self.canvas
    }

    /// Subscribe to canvas updates (latest-value semantics).
    #[must_use]
    pub fn canvas_receiver(&self) -> watch::Receiver<CanvasFrame> {
        self.canvas.subscribe()
    }

    /// Number of active canvas watch receivers.
    #[must_use]
    pub fn canvas_receiver_count(&self) -> usize {
        self.canvas.receiver_count()
    }

    /// Access the screen-canvas watch sender (for source preview publication).
    #[must_use]
    pub fn screen_canvas_sender(&self) -> &watch::Sender<CanvasFrame> {
        &self.screen_canvas
    }

    /// Subscribe to screen-canvas updates (latest-value semantics).
    #[must_use]
    pub fn screen_canvas_receiver(&self) -> watch::Receiver<CanvasFrame> {
        self.screen_canvas.subscribe()
    }

    /// Number of active screen-canvas watch receivers.
    #[must_use]
    pub fn screen_canvas_receiver_count(&self) -> usize {
        self.screen_canvas.receiver_count()
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
