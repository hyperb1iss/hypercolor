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
    /// Smoothed rate of completed transport deliveries.
    pub delivered_fps: f32,
    /// Smoothed rate of frames accepted by the output queue.
    pub accepted_fps: f32,
    /// Smoothed output write rate.
    pub fps_sent: f32,
    /// Smoothed rate of frames queued into the output lane.
    pub fps_queued: f32,
    /// Backward-compatible alias for `fps_sent`.
    pub fps_actual: f32,
    /// Configured frame-rate cap for the queue.
    pub fps_target: u32,
    /// Configured minimum output interval in milliseconds.
    pub target_interval_ms: Option<u64>,
    /// Payload bytes per second. Excludes transport framing.
    pub payload_bps_estimate: u64,
    /// Rolling average write-path latency from enqueue to completion.
    pub avg_latency_ms: u32,
    /// Actual transport duration, excluding queue wait.
    pub avg_transport_latency_ms: u32,
    /// Total frames accepted by the output queue.
    pub frames_received: u64,
    pub accepted: u64,
    pub frames_sent: u64,
    pub transport_started: u64,
    pub transport_completed: u64,
    pub transport_failed: u64,
    pub completed_payload_bytes: u64,
    pub frames_suppressed: u64,
    pub frames_dropped: u64,
    pub coalesced: u64,
    pub coalesced_target_cadence: u64,
    pub coalesced_backend_overrun: u64,
    /// Cumulative async write failures observed by this queue.
    pub errors_total: u64,
    /// Sanitized last-error string (whitespace collapsed, length-capped).
    pub last_error: Option<String>,
    /// Milliseconds since the last write attempt, `None` if none yet.
    pub last_sent_ago_ms: Option<u64>,
    pub queue_generation: u64,
    pub last_transport_started_sequence: u64,
    pub last_transport_completed_sequence: u64,
    pub last_transport_failed_sequence: u64,
}

impl DeviceMetrics {
    #[must_use]
    pub fn sent_fps(&self) -> f32 {
        if self.delivered_fps > 0.0 {
            self.delivered_fps
        } else if self.fps_sent > 0.0 || self.fps_actual <= 0.0 {
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
