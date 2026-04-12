//! Latest composited display frames captured for preview surfaces.
//!
//! The display output worker publishes every encoded JPEG into this runtime
//! before it reaches the device. API handlers can then fetch the most recent
//! frame for any display (real or simulated) for preview and debugging.
//!
//! Frames carry a monotonic counter and capture timestamp so HTTP handlers
//! can respond to conditional requests (`ETag`, `Last-Modified`) without
//! shipping unchanged bytes.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::SystemTime;

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
/// demand. No backpressure: newer frames simply overwrite older ones.
#[derive(Debug, Default)]
pub struct DisplayFrameRuntime {
    frames: HashMap<DeviceId, DisplayFrameSnapshot>,
}

impl DisplayFrameRuntime {
    /// Construct an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Replace the stored frame for a device.
    pub fn set_frame(&mut self, device_id: DeviceId, frame: DisplayFrameSnapshot) {
        self.frames.insert(device_id, frame);
    }

    /// Return the latest frame for a device, if any.
    #[must_use]
    pub fn frame(&self, device_id: DeviceId) -> Option<DisplayFrameSnapshot> {
        self.frames.get(&device_id).cloned()
    }

    /// Forget any frame captured for a device, typically on disconnect.
    pub fn remove(&mut self, device_id: DeviceId) {
        self.frames.remove(&device_id);
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
