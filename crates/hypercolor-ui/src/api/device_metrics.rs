//! Per-device output telemetry types and REST fetcher.
//!
//! Mirrors the daemon's `DeviceMetricsSnapshot` shape. The daemon computes
//! stable output rates in one shared collector, so every caller sees the same
//! numbers for a given `taken_at_ms`.

use serde::Deserialize;

/// Per-device output telemetry snapshot for a single device.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct DeviceMetrics {
    /// Device identity — joined against `DeviceSummary.id` on the client.
    pub id: String,
    /// Smoothed output write rate.
    pub fps_sent: f32,
    /// Smoothed rate of frames queued into the output lane.
    pub fps_queued: f32,
    /// Backward-compatible alias for `fps_sent`.
    pub fps_actual: f32,
    /// Configured frame-rate cap for the queue.
    pub fps_target: u32,
    /// Payload bytes per second. Excludes transport framing.
    pub payload_bps_estimate: u64,
    /// Rolling average write-path latency from enqueue to completion.
    pub avg_latency_ms: u32,
    /// Total frames accepted by the output queue.
    pub frames_received: u64,
    pub frames_sent: u64,
    pub frames_dropped: u64,
    /// Cumulative async write failures observed by this queue.
    pub errors_total: u64,
    /// Sanitized last-error string (whitespace collapsed, length-capped).
    pub last_error: Option<String>,
    /// Milliseconds since the last write attempt, `None` if none yet.
    pub last_sent_ago_ms: Option<u64>,
}

impl DeviceMetrics {
    #[must_use]
    pub fn sent_fps(&self) -> f32 {
        if self.fps_sent > 0.0 || self.fps_actual <= 0.0 {
            self.fps_sent
        } else {
            self.fps_actual
        }
    }

    #[must_use]
    pub fn queued_fps(&self) -> f32 {
        if self.fps_queued > 0.0 {
            self.fps_queued
        } else {
            self.sent_fps()
        }
    }
}

/// Shared snapshot served by `/api/v1/devices/metrics` and the
/// `device_metrics` WebSocket topic.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct DeviceMetricsSnapshot {
    /// Unix milliseconds at which the snapshot was taken.
    pub taken_at_ms: i64,
    pub items: Vec<DeviceMetrics>,
}
