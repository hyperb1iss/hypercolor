//! Per-device output telemetry types and REST fetcher.
//!
//! Mirrors the daemon's `DeviceMetricsSnapshot` shape. The daemon computes
//! rates against a shared 500 ms sampling window, so every caller sees the
//! same numbers for a given `taken_at_ms`.

use serde::Deserialize;

use super::client;

/// Per-device output telemetry snapshot for a single device.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct DeviceMetrics {
    /// Device identity — joined against `DeviceSummary.id` on the client.
    pub id: String,
    /// Derived from `frames_sent` deltas in the latest sampling window.
    pub fps_actual: f32,
    /// Configured target frame rate for the queue.
    pub fps_target: u32,
    /// Payload bytes per second. Excludes transport framing.
    pub payload_bps_estimate: u64,
    /// Rolling average write-path latency from enqueue to completion.
    pub avg_latency_ms: u32,
    pub frames_sent: u64,
    pub frames_dropped: u64,
    /// Cumulative async write failures observed by this queue.
    pub errors_total: u64,
    /// Sanitized last-error string (whitespace collapsed, length-capped).
    pub last_error: Option<String>,
    /// Milliseconds since the last write attempt, `None` if none yet.
    pub last_sent_ago_ms: Option<u64>,
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

/// Fetch the latest shared device metrics snapshot from REST.
pub async fn fetch_device_metrics() -> Result<DeviceMetricsSnapshot, String> {
    let snapshot: DeviceMetricsSnapshot = client::fetch_json("/api/v1/devices/metrics").await?;
    Ok(snapshot)
}
