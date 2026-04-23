//! Latest composited display frames captured for preview surfaces.
//!
//! The display output worker publishes every encoded JPEG into this runtime
//! before it reaches the device. API handlers can then fetch the most recent
//! frame for any display (real or simulated) for preview and debugging.
//!
//! Frames carry a monotonic counter and capture timestamp so HTTP handlers
//! can respond to conditional requests (`ETag`, `Last-Modified`) without
//! shipping unchanged bytes.
//!
//! Subscribers that need push-style notifications (e.g. the `display_preview`
//! WebSocket relay) can call [`DisplayFrameRuntime::subscribe`] to receive a
//! `tokio::sync::watch::Receiver` that's ticked on every frame write. The
//! sender for a given device is created lazily on first subscribe, so
//! devices that no one is watching pay zero notification overhead.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::SystemTime;

use tokio::sync::watch;

use hypercolor_types::device::DeviceId;

/// A single captured display frame ready to serve as a preview image.
#[derive(Debug, Clone)]
pub struct DisplayFrameSnapshot {
    /// JPEG bytes produced by the display encoder.
    pub jpeg_data: Arc<Vec<u8>>,
    /// Display width in pixels.
    pub width: u32,
    /// Display height in pixels.
    pub height: u32,
    /// Whether the display renders on a circular surface.
    pub circular: bool,
    /// Monotonic per-device frame counter, useful as an `ETag`.
    pub frame_number: u64,
    /// When the frame was captured, used for `Last-Modified`.
    pub captured_at: SystemTime,
}

/// In-memory registry of the latest display frame per device.
///
/// The worker pipeline publishes here on every write; API handlers read on
/// demand. No backpressure: newer frames simply overwrite older ones. A
/// per-device `watch::Sender` tracks subscribers so push consumers can
/// `await` fresh frames without polling.
#[derive(Debug, Default)]
pub struct DisplayFrameRuntime {
    frames: HashMap<DeviceId, DisplayFrameSnapshot>,
    /// Watch senders keyed by device. Senders are created lazily on first
    /// `subscribe()` call so idle displays incur no notification cost, and
    /// they're dropped in `remove()` so receivers observe closure once the
    /// device disconnects.
    watchers: HashMap<DeviceId, watch::Sender<Option<Arc<DisplayFrameSnapshot>>>>,
}

impl DisplayFrameRuntime {
    /// Construct an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Replace the stored frame for a device and notify any active
    /// subscribers. Dead watch senders (all receivers dropped) are
    /// garbage-collected opportunistically here so the map doesn't grow
    /// without bound when consumers come and go.
    pub fn set_frame(&mut self, device_id: DeviceId, frame: DisplayFrameSnapshot) {
        let shared = Arc::new(frame.clone());
        self.frames.insert(device_id, frame);
        if let Some(sender) = self.watchers.get(&device_id) {
            // `send` fails only when every receiver has been dropped; in
            // that case the sender is no longer useful, so drop it too.
            if sender.send(Some(Arc::clone(&shared))).is_err() {
                self.watchers.remove(&device_id);
            }
        }
    }

    /// Return the latest frame for a device, if any.
    #[must_use]
    pub fn frame(&self, device_id: DeviceId) -> Option<DisplayFrameSnapshot> {
        self.frames.get(&device_id).cloned()
    }

    /// Subscribe to frame notifications for a specific device. The first
    /// subscribe creates the watch channel; subsequent calls clone the
    /// existing receiver so every consumer observes the same frame stream.
    /// The initial value reflects the latest snapshot if one exists.
    pub fn subscribe(
        &mut self,
        device_id: DeviceId,
    ) -> watch::Receiver<Option<Arc<DisplayFrameSnapshot>>> {
        let initial = self.frames.get(&device_id).cloned().map(Arc::new);
        let sender = self
            .watchers
            .entry(device_id)
            .or_insert_with(|| watch::channel(initial.clone()).0);
        sender.subscribe()
    }

    /// Return the devices that currently have at least one live preview
    /// subscriber attached to their watch channel.
    #[must_use]
    pub fn subscribed_device_ids(&self) -> HashSet<DeviceId> {
        self.watchers
            .iter()
            .filter_map(|(device_id, sender)| (sender.receiver_count() > 0).then_some(*device_id))
            .collect()
    }

    /// Forget any frame captured for a device and close the watch channel
    /// so subscribers observe the stream ending. Typically invoked from
    /// the display output worker on disconnect.
    pub fn remove(&mut self, device_id: DeviceId) {
        self.frames.remove(&device_id);
        if let Some(sender) = self.watchers.remove(&device_id) {
            // `None` signals "no more frames coming from this device";
            // dropping the sender then closes receivers on next `await`.
            let _ = sender.send(None);
        }
    }

    /// Number of devices currently holding a captured frame.
    #[must_use]
    pub fn len(&self) -> usize {
        self.frames.len()
    }

    /// Whether the runtime is currently empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }
}
