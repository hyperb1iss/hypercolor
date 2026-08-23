//! Diagnostics API contracts — `/api/v1/diagnose`.

use serde::{Deserialize, Serialize};

use crate::api::system::InputStatus;

/// Optional body for `POST /api/v1/diagnose`.
///
/// Omitting `checks` runs the full check set; `system` adds the host
/// environment section to the report.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct DiagnoseRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checks: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct DiagnoseResponse {
    pub checks: Vec<DiagnoseCheck>,
    pub summary: DiagnoseSummary,
    pub snapshot: DiagnoseSnapshot,
}

#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct DiagnoseCheck {
    pub category: String,
    pub name: String,
    pub status: String,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct DiagnoseSummary {
    pub passed: usize,
    pub warnings: usize,
    pub failed: usize,
}

#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct DiagnoseSnapshot {
    pub input: InputStatus,
    pub render: DiagnoseRenderSnapshot,
    pub usb: DiagnoseUsbActorSnapshot,
    pub display_output: DiagnoseDisplayOutputSnapshot,
    pub device_output: DiagnoseDeviceOutputSnapshot,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub macos_screen_parity: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct DiagnoseRenderSnapshot {
    pub latest_frame: Option<DiagnoseLatestFrameSnapshot>,
    pub recent_window: DiagnoseRenderWindowSnapshot,
}

#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
#[allow(
    clippy::struct_excessive_bools,
    reason = "diagnostics snapshot mirrors independent frame freshness flags"
)]
pub struct DiagnoseLatestFrameSnapshot {
    pub frame_token: u64,
    pub frame_age_ms: f64,
    pub compositor_backend: String,
    pub output_frame_source: String,
    pub output_reuses_published_frame: bool,
    pub output_brightness_bits: u32,
    pub output_brightness_generation: u64,
    pub output_routing_signature: u64,
    pub output_zone_shape_signature: u64,
    pub output_unassigned_behavior_generation: u64,
    pub devices_written: u32,
    pub total_leds: u32,
    pub gpu_zone_sampling: bool,
    pub gpu_sample_deferred: bool,
    pub gpu_sample_stale: bool,
    pub gpu_sample_retry_hit: bool,
    pub gpu_sample_queue_saturated: bool,
    pub gpu_sample_wait_blocked: bool,
    pub gpu_sample_cpu_fallback: bool,
    pub cpu_readback_skipped: bool,
    pub gpu_readback_failed: bool,
    pub input_us: u32,
    pub render_us: u32,
    pub producer_us: u32,
    pub composition_us: u32,
    pub sample_us: u32,
    pub push_us: u32,
    pub publish_us: u32,
    pub overhead_us: u32,
    pub total_us: u32,
    pub output_errors: u32,
}

#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct DiagnoseRenderWindowSnapshot {
    pub frames: u32,
    pub gpu_sample_deferred: u32,
    pub gpu_sample_stale: u32,
    pub gpu_sample_retry_hit: u32,
    pub gpu_sample_queue_saturated: u32,
    pub gpu_sample_wait_blocked: u32,
    pub gpu_sample_cpu_fallback: u32,
    pub output_current_frame: u32,
    pub output_published_frame: u32,
    pub output_routed_reuse: u32,
    pub output_reused_published_frame: u32,
    pub output_error_frames: u32,
    pub push_avg_ms: f64,
    pub push_p95_ms: f64,
    pub publish_avg_ms: f64,
    pub publish_p95_ms: f64,
}

#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
#[allow(
    clippy::struct_field_names,
    reason = "JSON names mirror the USB actor metrics exported elsewhere"
)]
pub struct DiagnoseUsbActorSnapshot {
    pub display_frames_total: u64,
    pub display_frames_delayed_for_led_total: u64,
    pub display_led_priority_wait_total_ms: f64,
    pub display_led_priority_wait_avg_ms: f64,
    pub display_led_priority_wait_max_ms: f64,
}

#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct DiagnoseDisplayOutputSnapshot {
    pub captured_devices: usize,
    pub preview_subscribers: usize,
    pub encode_attempts_total: u64,
    pub encode_successes_total: u64,
    pub encode_failures_total: u64,
    pub encode_avg_ms: f64,
    pub encode_max_ms: f64,
    pub encode_last_ms: Option<f64>,
    pub encoded_bytes_total: u64,
    pub encoded_last_bytes: u64,
    pub write_attempts_total: u64,
    pub write_successes_total: u64,
    pub write_failures_total: u64,
    pub retry_attempts_total: u64,
    pub last_failure_age_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct DiagnoseDeviceOutputSnapshot {
    pub queues: usize,
    pub usb_queues: usize,
    pub lagging_queues: usize,
    pub worker_finished_queues: usize,
    pub dropped_frames_total: u64,
    pub errors_total: u64,
    pub items: Vec<DiagnoseDeviceOutputItem>,
}

#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct DiagnoseDeviceOutputItem {
    pub id: String,
    pub backend_id: String,
    pub mapped_layout_ids: Vec<String>,
    pub uses_frame_sink: bool,
    pub worker_finished: bool,
    pub delivered_fps: f32,
    pub accepted_fps: f32,
    pub fps_sent: f32,
    pub fps_queued: f32,
    pub fps_target: u32,
    pub frames_received: u64,
    pub accepted: u64,
    pub frames_sent: u64,
    pub transport_started: u64,
    pub transport_completed: u64,
    pub transport_failed: u64,
    pub completed_payload_bytes: u64,
    pub frames_dropped: u64,
    pub coalesced: u64,
    pub coalesced_target_cadence: u64,
    pub coalesced_backend_overrun: u64,
    pub errors_total: u64,
    pub avg_latency_ms: u32,
    pub avg_queue_wait_ms: u32,
    pub avg_write_ms: u32,
    pub avg_transport_latency_ms: u32,
    pub last_sent_ago_ms: Option<u64>,
    pub last_error: Option<String>,
    pub last_sequence: u64,
    pub queue_generation: u64,
    pub last_transport_started_sequence: u64,
    pub last_transport_completed_sequence: u64,
    pub last_transport_failed_sequence: u64,
    pub display_queue_generation: Option<u64>,
    pub display_transport_started: u64,
    pub display_transport_completed: u64,
    pub display_transport_failed: u64,
}
