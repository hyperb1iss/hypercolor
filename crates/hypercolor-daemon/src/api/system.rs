//! System endpoints — `/api/v1/status`, `/health`.
//!
//! Provides daemon status overview and a lightweight health check
//! for monitoring and load balancer probes.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Context;
use axum::extract::{Extension, Path, State};
use axum::response::{IntoResponse, Response};
use cpal::traits::{DeviceTrait, HostTrait};
use hypercolor_core::config::canonical_audio_device_id;
use hypercolor_core::engine::RenderLoopState;
#[cfg(target_os = "linux")]
use hypercolor_core::input::audio::linux;
use hypercolor_core::input::screen::{
    PixelExtent, ScreenAnalysisComputeCapacity, ScreenAnalysisResourcePlan, ScreenAnalysisWorkPlan,
};
use hypercolor_core::input::{
    DataSourceKind, InputData, SourceFreshness, SourceIssue, SourceKind, SourceState, SourceStatus,
};
use hypercolor_types::config::RenderAccelerationMode;
use hypercolor_types::sensor::SystemSnapshot;
use hypercolor_types::source_status::SourceDiagnosticsEnvelope;
use serde::Serialize;
use tracing::{debug, warn};
use utoipa::ToSchema;

use crate::api::AppState;
use crate::api::envelope::ApiResponse;
use crate::api::security::RequestAuthContext;
use crate::domain::{DomainError, ResourceKind};
use crate::macos_owner::{MacosDaemonOwner, MacosHandoverPhase, MacosOwnerSnapshot};
use crate::performance::LatestFrameMetrics;
use crate::preview_runtime::{PreviewDemandSummary, PreviewRuntime};
use crate::session::current_global_brightness;

use hypercolor_core::config::ConfigManager;
use hypercolor_types::server::ServerIdentity;

const DEFAULT_CONFIG_FILE_NAME: &str = "hypercolor.toml";
const MULTI_ZONE_CAPABILITIES: &[&str] = &[
    "multi-zone-sampling",
    "zone-crud",
    "zone-device-assignment",
    "zone-layout-edit",
    "zone-preview-frames",
    "scene-unassigned-behavior-write",
];

// ── Response Types ───────────────────────────────────────────────────────

#[derive(Debug, Serialize, ToSchema)]
pub struct SystemStatus {
    pub running: bool,
    pub version: String,
    pub server: ServerIdentity,
    pub config_path: String,
    pub data_dir: String,
    pub cache_dir: String,
    pub uptime_seconds: u64,
    pub device_count: usize,
    pub effect_count: usize,
    pub scene_count: usize,
    pub active_effect: Option<String>,
    pub active_scene: Option<String>,
    pub active_scene_snapshot_locked: bool,
    pub global_brightness: u8,
    pub audio_available: bool,
    pub capture_available: bool,
    pub screen_capture_capacity: ScreenCaptureCapacityStatus,
    pub input: InputStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub macos_daemon_ownership: Option<MacosDaemonOwnershipApiStatus>,
    pub compositor_acceleration: RenderAccelerationStatus,
    pub render_loop: RenderLoopStatus,
    pub session_performance: SessionPerformanceStatus,
    pub latest_frame: Option<LatestFrameStatus>,
    pub effect_health: EffectHealthStatus,
    pub preview_runtime: PreviewRuntimeStatus,
    pub event_bus_subscribers: usize,
    pub capabilities: Vec<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct SessionPerformanceStatus {
    pub input_stage: LatencyPercentilesStatus,
    pub full_frame_cpu_copies: FullFrameCopySessionStatus,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct LatencyPercentilesStatus {
    pub sample_count: u64,
    pub avg_ms: f64,
    pub p95_ms: f64,
    pub p99_ms: f64,
    pub max_ms: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cumulative_histogram: Option<LatencyHistogramStatus>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct LatencyHistogramStatus {
    pub bucket_width_us: u32,
    pub overflow_bucket_index: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snapshot_frame_token: Option<u64>,
    pub buckets: Vec<LatencyHistogramBucketStatus>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct LatencyHistogramBucketStatus {
    pub bucket_index: u32,
    pub count: u64,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct FullFrameCopySessionStatus {
    pub count: u64,
    pub frames: u64,
    pub bytes: u64,
}

/// Installed byte fences for transactional screen publication admission.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ScreenCaptureCapacityStatus {
    pub admission_enforced: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub physical_transition_byte_capacity: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub physical_transition_backend_capacity: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub physical_reserved_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub physical_available_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub steady_total_byte_budget: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub steady_total_backend_capacity: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub steady_publication_byte_budget: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transition_publication_backend_capacity: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub analysis_width: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub analysis_height: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub analysis_retained_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub analysis_peak_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub analysis_weighted_work_units_per_frame: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub analysis_weighted_work_units_per_second: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub analysis_parallel_capacity_per_second: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub analysis_serial_capacity_per_second: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub analysis_worker_count: Option<u64>,
}

impl ScreenCaptureCapacityStatus {
    const fn without_capacity(admission_enforced: bool) -> Self {
        Self {
            admission_enforced,
            physical_transition_byte_capacity: None,
            physical_transition_backend_capacity: None,
            physical_reserved_bytes: None,
            physical_available_bytes: None,
            steady_total_byte_budget: None,
            steady_total_backend_capacity: None,
            steady_publication_byte_budget: None,
            transition_publication_backend_capacity: None,
            analysis_width: None,
            analysis_height: None,
            analysis_retained_bytes: None,
            analysis_peak_bytes: None,
            analysis_weighted_work_units_per_frame: None,
            analysis_weighted_work_units_per_second: None,
            analysis_parallel_capacity_per_second: None,
            analysis_serial_capacity_per_second: None,
            analysis_worker_count: None,
        }
    }
}

/// Host keyboard/mouse capture health, for consent and remediation UX.
///
/// `enabled` is the consent config gate. `host_capturing` is true when a
/// host backend is actively reading device nodes. `devices_denied` counts
/// input nodes present but unreadable (udev rules missing) — the signal
/// that distinguishes "input is off" from "input is on but blocked".
///
/// `degraded` carries the failures the counters cannot express. Windows has no
/// per-device denial to count: either the process has a visible window station
/// and sees input, or it does not, and that is a session-level fact rather than
/// a per-node one.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct InputStatus {
    pub enabled: bool,
    pub host_capture_registered: bool,
    pub host_capturing: bool,
    pub devices_opened: usize,
    pub devices_denied: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub degraded: Option<String>,
    pub backends: Vec<String>,
    #[serde(default)]
    pub source_graph_generation: u64,
    #[serde(default)]
    pub sources: Vec<InputSourceStatus>,
}

/// Structured source issue safe for operational status surfaces.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct InputSourceIssueStatus {
    pub code: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remediation: Option<String>,
    pub retryable: bool,
}

#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum MacosCapabilityOwnerApi {
    AppSidecar,
    App,
    LaunchdService,
    HomebrewService,
    Broker,
    Standalone,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct MacosDaemonOwnerConflictApiStatus {
    pub active: MacosCapabilityOwnerApi,
    pub contender: MacosCapabilityOwnerApi,
    pub observed_at_ms: u64,
}

#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum MacosDaemonHandoverPhaseApi {
    Prepared,
    AutostartsConfigured,
    StopRequested,
    OutgoingOwnerStopped,
    AwaitingGuardRelease,
    GuardReleased,
    StartRequested,
    RequestedOwnerStarted,
    CommitPending,
    Committed,
    RollbackPending,
    RollbackAutostartsRestored,
    RollbackStopRequested,
    RollbackOwnerStopped,
    RollbackAwaitingGuardRelease,
    RollbackGuardReleased,
    RollbackStartRequested,
    PriorOwnerStarted,
    RollbackCommitPending,
    RolledBack,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct MacosDaemonOwnerRecoveryRequiredApiStatus {
    pub requested_owner: MacosCapabilityOwnerApi,
    pub prior_owner: MacosCapabilityOwnerApi,
    pub phase: MacosDaemonHandoverPhaseApi,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct MacosDaemonOwnershipApiStatus {
    pub active_owner: MacosCapabilityOwnerApi,
    pub owner_epoch: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conflict: Option<MacosDaemonOwnerConflictApiStatus>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recovery_required: Option<MacosDaemonOwnerRecoveryRequiredApiStatus>,
}

/// Lock-free lifecycle and freshness status for one input source.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "source policy and lifecycle flags are independent diagnostics"
)]
pub struct InputSourceStatus {
    pub source_id: String,
    pub kind: String,
    pub backend: String,
    pub configured: bool,
    pub consented: bool,
    pub demanded: bool,
    pub active_consumer_count: usize,
    pub state: String,
    pub freshness: String,
    pub source_graph_generation: u64,
    pub session_generation: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_sample_age_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub freshness_remaining_ms: Option<u64>,
    pub resource_count: usize,
    pub denied_resource_count: usize,
    /// Effective issue, with freshness taking precedence while stale.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issue: Option<InputSourceIssueStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lifecycle_issue: Option<InputSourceIssueStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub freshness_issue: Option<InputSourceIssueStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action_issue: Option<InputSourceIssueStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diagnostics: Option<SourceDiagnosticsEnvelope>,
    pub retired: bool,
}

#[derive(Debug)]
pub(crate) struct InputDiagnostic {
    pub source_id: String,
    pub status: &'static str,
    pub detail: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct RenderLoopStatus {
    pub state: String,
    pub fps_tier: String,
    pub target_fps: u32,
    pub ceiling_fps: u32,
    pub capacity_fps: f64,
    pub delivered_fps: f64,
    pub actual_fps: f64,
    pub consecutive_misses: u32,
    pub total_frames: u64,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct RenderAccelerationStatus {
    pub requested_mode: String,
    pub effective_mode: String,
    pub fallback_reason: Option<String>,
    pub servo_gpu_import_mode: String,
    pub servo_gpu_import_attempting: bool,
    pub gpu_probe: Option<GpuCompositorProbeStatus>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct GpuCompositorProbeStatus {
    pub adapter_name: String,
    pub adapter_device_type: String,
    pub backend: String,
    pub texture_format: String,
    pub max_texture_dimension_2d: u32,
    pub max_storage_textures_per_shader_stage: u32,
    pub software_adapter_reason: Option<String>,
    pub servo_gpu_import_backend_compatible: bool,
    pub servo_gpu_import_backend_reason: Option<String>,
    pub linux_servo_gpu_import_backend_compatible: bool,
    pub linux_servo_gpu_import_backend_reason: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct LatestFrameStatus {
    pub frame_token: u64,
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
    /// Deprecated v1 compatibility field. Always `false`.
    pub cpu_sampling_late_readback: bool,
    /// Deprecated v1 compatibility alias. Always `false`.
    pub led_sampling_readback: bool,
    pub preview_surface: bool,
    pub scene_canvas_forced_surface: bool,
    pub cpu_readback_skipped: bool,
    pub gpu_readback_failed: bool,
    pub total_ms: f64,
    pub wake_late_ms: f64,
    pub jitter_ms: f64,
    pub frame_age_ms: f64,
    pub input_sampling_ms: f64,
    pub producer_ms: f64,
    pub producer_render_ms: f64,
    #[serde(rename = "producer_preview_compose_ms")]
    pub producer_scene_compose_ms: f64,
    pub composition_ms: f64,
    pub effect_rendering_ms: f64,
    pub spatial_sampling_ms: f64,
    pub device_output_ms: f64,
    pub preview_postprocess_ms: f64,
    pub event_bus_ms: f64,
    pub coordination_overhead_ms: f64,
    pub publish_frame_data_ms: f64,
    pub publish_group_canvas_ms: f64,
    pub publish_preview_ms: f64,
    pub publish_events_ms: f64,
    pub logical_layer_count: u32,
    pub render_group_count: u32,
    pub full_frame_copy_count: u32,
    pub full_frame_copy_kb: f64,
    pub producer_full_frame_copy_count: u32,
    pub producer_full_frame_copy_kb: f64,
    pub producer_full_frame_copy_reason: Option<String>,
    pub publication_full_frame_copy_count: u32,
    pub publication_full_frame_copy_kb: f64,
    pub publication_full_frame_copy_reason: Option<String>,
    pub output_errors: u32,
    pub render_surfaces: RenderSurfaceStatus,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct RenderSurfaceStatus {
    /// Deprecated v1 alias for `scene_pool_slot_count`.
    pub slot_count: u32,
    /// Deprecated v1 alias for `scene_pool_free_slots`.
    pub free_slots: u32,
    /// Deprecated v1 alias for `scene_pool_published_slots`.
    pub published_slots: u32,
    /// Deprecated v1 alias for `scene_pool_dequeued_slots`.
    pub dequeued_slots: u32,
    pub canvas_receivers: u32,
    pub scene_pool_slot_count: u32,
    pub scene_pool_free_slots: u32,
    pub scene_pool_published_slots: u32,
    pub scene_pool_dequeued_slots: u32,
    pub direct_pool_slot_count: u32,
    pub direct_pool_free_slots: u32,
    pub direct_pool_published_slots: u32,
    pub direct_pool_dequeued_slots: u32,
    pub preview_pool_slot_count: u32,
    pub preview_pool_free_slots: u32,
    pub preview_pool_published_slots: u32,
    pub preview_pool_dequeued_slots: u32,
    pub compositor_pool_slot_count: u32,
    pub compositor_pool_free_slots: u32,
    pub compositor_pool_published_slots: u32,
    pub compositor_pool_dequeued_slots: u32,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct EffectHealthStatus {
    pub errors_total: u64,
    pub fallbacks_applied_total: u64,
    pub producer_gpu_readback_failures_total: u64,
    pub servo_soft_stalls_total: u64,
    pub servo_breaker_opens_total: u64,
    pub servo_session_creates_total: u64,
    pub servo_session_create_failures_total: u64,
    pub servo_session_create_wait_total_ms: f64,
    pub servo_session_create_wait_max_ms: f64,
    pub servo_page_loads_total: u64,
    pub servo_page_load_failures_total: u64,
    pub servo_page_load_wait_total_ms: f64,
    pub servo_page_load_wait_max_ms: f64,
    pub servo_detached_destroys_total: u64,
    pub servo_detached_destroy_failures_total: u64,
    pub servo_render_requests_total: u64,
    pub servo_render_queue_wait_total_ms: f64,
    pub servo_render_queue_wait_max_ms: f64,
    pub servo_render_scene_requests_total: u64,
    pub servo_render_scene_queue_wait_total_ms: f64,
    pub servo_render_scene_queue_wait_max_ms: f64,
    pub servo_render_display_requests_total: u64,
    pub servo_render_display_queue_wait_total_ms: f64,
    pub servo_render_display_queue_wait_max_ms: f64,
    pub servo_render_cpu_frames_total: u64,
    pub servo_render_cached_frames_total: u64,
    pub servo_render_gpu_frames_total: u64,
    pub servo_gpu_import_failures_total: u64,
    pub servo_gpu_import_fallbacks_total: u64,
    pub servo_gpu_import_fallback_reason: Option<&'static str>,
    pub servo_gpu_import_windows_sync_mode: Option<&'static str>,
    pub servo_gpu_import_stale_frame_total: u64,
    pub servo_gpu_import_adapter_mismatch_total: u64,
    pub servo_gpu_import_slot_count: u64,
    pub servo_gpu_import_pending_slots: u64,
    pub servo_gpu_import_pending_slots_max: u64,
    pub servo_gpu_import_completed_slots: u64,
    pub servo_gpu_import_available_slots: u64,
    pub servo_gpu_import_available_slots_min: u64,
    pub servo_gpu_import_oldest_pending_age_max_ms: f64,
    pub servo_gpu_import_blit_total_ms: f64,
    pub servo_gpu_import_blit_max_ms: f64,
    pub servo_gpu_import_sync_total_ms: f64,
    pub servo_gpu_import_sync_max_ms: f64,
    pub servo_gpu_import_total_ms: f64,
    pub servo_gpu_import_max_ms: f64,
    pub producer_cpu_frames_total: u64,
    pub producer_gpu_frames_total: u64,
    pub producer_gpu_cpu_materialization_blocked_total: u64,
    pub sparkleflinger_gpu_source_upload_skipped_total: u64,
    pub sparkleflinger_media_texture_allocations_total: u64,
    pub sparkleflinger_media_texture_upload_bytes_total: u64,
    pub sparkleflinger_display_finalize_rgba_attempts_total: u64,
    pub sparkleflinger_display_finalize_yuv_attempts_total: u64,
    pub sparkleflinger_display_finalize_successes_total: u64,
    pub sparkleflinger_display_finalize_misses_total: u64,
    pub sparkleflinger_display_finalize_latches_total: u64,
    pub sparkleflinger_display_finalize_blocking_wait_total_ms: f64,
    pub sparkleflinger_display_finalize_blocking_wait_max_ms: f64,
    pub sparkleflinger_display_finalize_surface_reallocs_total: u64,
    pub servo_render_evaluate_scripts_total_ms: f64,
    pub servo_render_evaluate_scripts_max_ms: f64,
    pub servo_render_event_loop_total_ms: f64,
    pub servo_render_event_loop_max_ms: f64,
    pub servo_render_paint_total_ms: f64,
    pub servo_render_paint_max_ms: f64,
    pub servo_render_readback_total_ms: f64,
    pub servo_render_readback_max_ms: f64,
    pub servo_render_frame_total_ms: f64,
    pub servo_render_frame_max_ms: f64,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct PreviewRuntimeStatus {
    pub canvas_receivers: u32,
    pub scene_canvas_receivers: u32,
    pub screen_canvas_receivers: u32,
    pub zone_preview_receivers: u32,
    pub canvas_frames_published: u64,
    pub scene_canvas_frames_published: u64,
    pub screen_canvas_frames_published: u64,
    pub zone_preview_frames_published: u64,
    pub latest_canvas_frame_number: u32,
    pub latest_scene_canvas_frame_number: u32,
    pub latest_screen_canvas_frame_number: u32,
    pub latest_zone_preview_frame_number: u32,
    pub canvas_demand: PreviewDemandStatus,
    pub scene_canvas_demand: PreviewDemandStatus,
    pub screen_canvas_demand: PreviewDemandStatus,
    pub zone_preview_demand: PreviewDemandStatus,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct PreviewDemandStatus {
    pub subscribers: u32,
    pub max_fps: u32,
    pub max_width: u32,
    pub max_height: u32,
    pub any_full_resolution: bool,
    pub any_rgb: bool,
    pub any_rgba: bool,
    pub any_jpeg: bool,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct HealthResponse {
    pub status: String,
    pub version: String,
    pub uptime_seconds: u64,
    pub checks: HealthChecks,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct HealthChecks {
    pub render_loop: String,
    pub device_backends: String,
    pub event_bus: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ServerInfo {
    #[serde(flatten)]
    pub identity: ServerIdentity,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_session_id: Option<String>,
    pub device_count: usize,
    pub auth_required: bool,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct SystemResource {
    pub identity: ServerInfo,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<SystemStatus>,
}

/// Build the bounded input health snapshot used without protected control.
#[must_use]
pub(crate) fn input_status_snapshot(state: &AppState) -> InputStatus {
    input_status_snapshot_with_privacy(state, false)
}

fn input_status_snapshot_with_privacy(
    state: &AppState,
    include_private_diagnostics: bool,
) -> InputStatus {
    let now = Instant::now();
    let registry = state.input_status.snapshot();
    let statuses = registry
        .handles()
        .iter()
        .map(|handle| handle.snapshot_at(now))
        .collect::<Vec<_>>();
    let host_sources = statuses
        .iter()
        .filter(|source| is_host_interaction_source(source));
    let sources = statuses
        .iter()
        .map(|source| input_source_status(source, now, include_private_diagnostics))
        .collect();

    InputStatus {
        enabled: state
            .config_manager
            .as_ref()
            .is_some_and(|manager| manager.get().input.enabled),
        host_capture_registered: host_sources.clone().next().is_some(),
        host_capturing: host_sources
            .clone()
            .any(|source| matches!(source.state, SourceState::Live | SourceState::Degraded)),
        devices_opened: host_sources
            .clone()
            .map(|source| source.resource_count)
            .sum(),
        devices_denied: host_sources
            .clone()
            .map(|source| source.denied_resource_count)
            .sum(),
        degraded: host_sources
            .clone()
            .find_map(|source| source.issue.as_ref())
            .map(|issue| issue.code.to_string()),
        backends: host_sources
            .map(|source| source.backend.to_string())
            .collect(),
        source_graph_generation: registry.source_graph_generation(),
        sources,
    }
}

/// Return only source states that require operator attention.
#[must_use]
pub(crate) fn actionable_input_diagnostics(input: &InputStatus) -> Vec<InputDiagnostic> {
    input
        .sources
        .iter()
        .filter_map(|source| {
            let (status, fallback, issue) =
                if source.demanded && matches!(source.state.as_str(), "failed" | "unavailable") {
                    (
                        "fail",
                        "demanded source is unavailable",
                        source.lifecycle_issue.as_ref(),
                    )
                } else if source.demanded && source.state == "stopped" {
                    (
                        "warning",
                        "demanded source is stopped",
                        source.lifecycle_issue.as_ref(),
                    )
                } else if source.demanded
                    && matches!(source.state.as_str(), "live" | "degraded")
                    && source.freshness == "stale"
                {
                    (
                        "warning",
                        "demanded source data is stale",
                        source.freshness_issue.as_ref(),
                    )
                } else if source.configured && source.state == "degraded" {
                    (
                        "warning",
                        "configured source is degraded",
                        source.lifecycle_issue.as_ref(),
                    )
                } else {
                    return None;
                };

            Some(InputDiagnostic {
                source_id: source.source_id.clone(),
                status,
                detail: format_input_diagnostic_detail(issue, fallback),
            })
        })
        .collect()
}

fn input_source_status(
    source: &SourceStatus,
    now: Instant,
    include_private_diagnostics: bool,
) -> InputSourceStatus {
    let lifecycle_issue = source.issue.as_ref().map(input_source_issue_status);
    let freshness_issue = source
        .freshness_issue
        .as_ref()
        .map(input_source_issue_status);
    let action_issue = source.action_issue.as_ref().map(input_source_issue_status);
    let issue = freshness_issue
        .clone()
        .or_else(|| action_issue.clone())
        .or_else(|| lifecycle_issue.clone());

    InputSourceStatus {
        source_id: source.source_id.to_string(),
        kind: source_kind_name(source.kind).to_owned(),
        backend: source.backend.to_string(),
        configured: source.configured,
        consented: source.consented,
        demanded: source.demanded,
        active_consumer_count: source.active_consumer_count,
        state: source_state_name(source.state).to_owned(),
        freshness: source_freshness_name(source.freshness).to_owned(),
        source_graph_generation: source.source_graph_generation,
        session_generation: source.session_generation,
        last_sample_age_ms: source
            .last_sample_at
            .map(|sampled_at| duration_ms(now.saturating_duration_since(sampled_at))),
        freshness_remaining_ms: source
            .freshness_deadline
            .map(|deadline| duration_ms(deadline.saturating_duration_since(now))),
        resource_count: source.resource_count,
        denied_resource_count: source.denied_resource_count,
        issue,
        lifecycle_issue,
        freshness_issue,
        action_issue,
        diagnostics: source.diagnostics.as_deref().and_then(|diagnostics| {
            if include_private_diagnostics {
                Some(diagnostics.clone())
            } else {
                diagnostics.public_projection()
            }
        }),
        retired: source.retired,
    }
}

const fn macos_daemon_owner(owner: MacosDaemonOwner) -> MacosCapabilityOwnerApi {
    match owner {
        MacosDaemonOwner::AppSidecar => MacosCapabilityOwnerApi::AppSidecar,
        MacosDaemonOwner::DirectLaunchd => MacosCapabilityOwnerApi::LaunchdService,
        MacosDaemonOwner::Homebrew => MacosCapabilityOwnerApi::HomebrewService,
        MacosDaemonOwner::Standalone => MacosCapabilityOwnerApi::Standalone,
    }
}

fn macos_daemon_ownership(snapshot: &MacosOwnerSnapshot) -> MacosDaemonOwnershipApiStatus {
    MacosDaemonOwnershipApiStatus {
        active_owner: macos_daemon_owner(snapshot.active_owner),
        owner_epoch: snapshot.owner_epoch,
        conflict: snapshot
            .conflict
            .map(|conflict| MacosDaemonOwnerConflictApiStatus {
                active: macos_daemon_owner(conflict.active_owner),
                contender: macos_daemon_owner(conflict.contender_owner),
                observed_at_ms: conflict.observed_at_ms,
            }),
        recovery_required: snapshot.recovery_required.map(|recovery| {
            MacosDaemonOwnerRecoveryRequiredApiStatus {
                requested_owner: macos_daemon_owner(recovery.requested_owner),
                prior_owner: macos_daemon_owner(recovery.prior_owner),
                phase: macos_daemon_handover_phase(recovery.phase),
            }
        }),
    }
}

const fn macos_daemon_handover_phase(phase: MacosHandoverPhase) -> MacosDaemonHandoverPhaseApi {
    match phase {
        MacosHandoverPhase::Prepared => MacosDaemonHandoverPhaseApi::Prepared,
        MacosHandoverPhase::AutostartsConfigured => {
            MacosDaemonHandoverPhaseApi::AutostartsConfigured
        }
        MacosHandoverPhase::StopRequested => MacosDaemonHandoverPhaseApi::StopRequested,
        MacosHandoverPhase::OutgoingOwnerStopped => {
            MacosDaemonHandoverPhaseApi::OutgoingOwnerStopped
        }
        MacosHandoverPhase::AwaitingGuardRelease => {
            MacosDaemonHandoverPhaseApi::AwaitingGuardRelease
        }
        MacosHandoverPhase::GuardReleased => MacosDaemonHandoverPhaseApi::GuardReleased,
        MacosHandoverPhase::StartRequested => MacosDaemonHandoverPhaseApi::StartRequested,
        MacosHandoverPhase::RequestedOwnerStarted => {
            MacosDaemonHandoverPhaseApi::RequestedOwnerStarted
        }
        MacosHandoverPhase::CommitPending => MacosDaemonHandoverPhaseApi::CommitPending,
        MacosHandoverPhase::Committed => MacosDaemonHandoverPhaseApi::Committed,
        MacosHandoverPhase::RollbackPending => MacosDaemonHandoverPhaseApi::RollbackPending,
        MacosHandoverPhase::RollbackAutostartsRestored => {
            MacosDaemonHandoverPhaseApi::RollbackAutostartsRestored
        }
        MacosHandoverPhase::RollbackStopRequested => {
            MacosDaemonHandoverPhaseApi::RollbackStopRequested
        }
        MacosHandoverPhase::RollbackOwnerStopped => {
            MacosDaemonHandoverPhaseApi::RollbackOwnerStopped
        }
        MacosHandoverPhase::RollbackAwaitingGuardRelease => {
            MacosDaemonHandoverPhaseApi::RollbackAwaitingGuardRelease
        }
        MacosHandoverPhase::RollbackGuardReleased => {
            MacosDaemonHandoverPhaseApi::RollbackGuardReleased
        }
        MacosHandoverPhase::RollbackStartRequested => {
            MacosDaemonHandoverPhaseApi::RollbackStartRequested
        }
        MacosHandoverPhase::PriorOwnerStarted => MacosDaemonHandoverPhaseApi::PriorOwnerStarted,
        MacosHandoverPhase::RollbackCommitPending => {
            MacosDaemonHandoverPhaseApi::RollbackCommitPending
        }
        MacosHandoverPhase::RolledBack => MacosDaemonHandoverPhaseApi::RolledBack,
    }
}

fn input_source_issue_status(issue: &SourceIssue) -> InputSourceIssueStatus {
    InputSourceIssueStatus {
        code: issue.code.to_string(),
        message: issue.message.to_string(),
        remediation: issue.remediation.as_ref().map(ToString::to_string),
        retryable: issue.retryable,
    }
}

fn format_input_diagnostic_detail(
    issue: Option<&InputSourceIssueStatus>,
    fallback: &str,
) -> String {
    let Some(issue) = issue else {
        return fallback.to_owned();
    };
    issue.remediation.as_ref().map_or_else(
        || format!("{}: {}", issue.code, issue.message),
        |remediation| format!("{}: {}; {}", issue.code, issue.message, remediation),
    )
}

fn is_host_interaction_source(source: &SourceStatus) -> bool {
    source.kind == SourceKind::Interaction && source.backend.as_ref() != "browser"
}

const fn source_kind_name(kind: SourceKind) -> &'static str {
    match kind {
        SourceKind::Audio => "audio",
        SourceKind::Screen => "screen",
        SourceKind::Interaction => "interaction",
        SourceKind::Media => "media",
        SourceKind::Network => "network",
        SourceKind::Sensors => "sensors",
    }
}

const fn source_state_name(state: SourceState) -> &'static str {
    match state {
        SourceState::Stopped => "stopped",
        SourceState::Starting => "starting",
        SourceState::Live => "live",
        SourceState::Degraded => "degraded",
        SourceState::Unavailable => "unavailable",
        SourceState::Failed => "failed",
    }
}

const fn source_freshness_name(freshness: SourceFreshness) -> &'static str {
    match freshness {
        SourceFreshness::NotApplicable => "not_applicable",
        SourceFreshness::AwaitingSample => "awaiting_sample",
        SourceFreshness::Fresh => "fresh",
        SourceFreshness::Stale => "stale",
    }
}

fn duration_ms(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

// ── Handlers ─────────────────────────────────────────────────────────────

/// `GET /api/v1/status` — Full system status overview.
#[utoipa::path(
    get,
    path = "/api/v1/status",
    responses(
        (
            status = 200,
            description = "Full daemon status overview",
            body = crate::api::envelope::ApiResponse<SystemStatus>
        )
    ),
    tag = "system"
)]
pub async fn get_status(State(state): State<Arc<AppState>>) -> Response {
    ApiResponse::ok(system_status_with_privacy(state, false).await)
}

async fn system_status_with_privacy(
    state: Arc<AppState>,
    include_private_diagnostics: bool,
) -> SystemStatus {
    let device_count = state.device_registry.len().await;
    let effect_count = state.effect_registry.read().await.len();
    let scene_count = state.scene_manager.read().await.scene_count();
    let subscribers = state.event_bus.subscriber_count();

    // Query the live effect engine for the active effect name.
    let active_effect = crate::api::effects::active_primary_effect(state.as_ref())
        .await
        .map(|(_, effect)| effect.name);
    let (active_scene, active_scene_snapshot_locked) = {
        let scene_manager = state.scene_manager.read().await;
        scene_manager.active_scene().map_or((None, false), |scene| {
            (Some(scene.name.clone()), scene.blocks_runtime_mutation())
        })
    };

    let (performance, input_time_histogram) = {
        let performance = state.performance.read().await;
        (
            performance.snapshot(),
            performance.input_time_histogram_snapshot(),
        )
    };

    // Query the live render loop for timing data.
    let render_loop_status = {
        let rl = state.render_loop.read().await;
        let snapshot = rl.stats();
        let capacity_fps = if snapshot.state == RenderLoopState::Running {
            round_1(paced_fps(
                snapshot.avg_frame_time.as_secs_f64(),
                snapshot.tier.fps(),
            ))
        } else {
            0.0
        };
        RenderLoopStatus {
            state: snapshot.state.to_string(),
            fps_tier: snapshot.tier.to_string(),
            target_fps: snapshot.tier.fps(),
            ceiling_fps: snapshot.max_tier.fps(),
            capacity_fps,
            delivered_fps: if snapshot.state == RenderLoopState::Running {
                round_1(performance.delivered_fps)
            } else {
                0.0
            },
            actual_fps: capacity_fps,
            consecutive_misses: snapshot.consecutive_misses,
            total_frames: snapshot.total_frames,
        }
    };
    let running = render_loop_is_operational(render_loop_status.state.as_str());
    let latest_frame = if render_loop_status.state == "running" {
        performance.latest_frame.as_ref().map(|frame| {
            latest_frame_status(frame, state.start_time.elapsed().as_secs_f64() * 1000.0)
        })
    } else {
        None
    };
    let session_performance = SessionPerformanceStatus {
        input_stage: LatencyPercentilesStatus {
            sample_count: performance.input_time_sample_count,
            avg_ms: round_2(performance.input_time.avg_ms),
            p95_ms: round_2(performance.input_time.p95_ms),
            p99_ms: round_2(performance.input_time.p99_ms),
            max_ms: round_2(performance.input_time.max_ms),
            cumulative_histogram: Some(LatencyHistogramStatus {
                bucket_width_us: input_time_histogram.bucket_width_us,
                overflow_bucket_index: input_time_histogram.overflow_bucket_index,
                snapshot_frame_token: performance
                    .latest_frame
                    .as_ref()
                    .map(|frame| frame.timeline.frame_token),
                buckets: input_time_histogram
                    .buckets
                    .into_iter()
                    .map(|bucket| LatencyHistogramBucketStatus {
                        bucket_index: bucket.bucket_index,
                        count: bucket.count,
                    })
                    .collect(),
            }),
        },
        full_frame_cpu_copies: FullFrameCopySessionStatus {
            count: performance.full_frame_copy_count_total,
            frames: performance.full_frame_copy_frames_total,
            bytes: performance.full_frame_copy_bytes_total,
        },
    };
    let servo_health = servo_effect_health_counts();
    let pipeline_health = render_pipeline_health_counts();
    let effect_health = EffectHealthStatus {
        errors_total: performance.effect_health.errors_total,
        fallbacks_applied_total: performance.effect_health.fallbacks_applied_total,
        producer_gpu_readback_failures_total: performance
            .effect_health
            .producer_gpu_readback_failures_total,
        servo_soft_stalls_total: servo_health.soft_stalls_total,
        servo_breaker_opens_total: servo_health.breaker_opens_total,
        servo_session_creates_total: servo_health.session_creates_total,
        servo_session_create_failures_total: servo_health.session_create_failures_total,
        servo_session_create_wait_total_ms: us_to_ms_f64(servo_health.session_create_wait_total_us),
        servo_session_create_wait_max_ms: us_to_ms_f64(servo_health.session_create_wait_max_us),
        servo_page_loads_total: servo_health.page_loads_total,
        servo_page_load_failures_total: servo_health.page_load_failures_total,
        servo_page_load_wait_total_ms: us_to_ms_f64(servo_health.page_load_wait_total_us),
        servo_page_load_wait_max_ms: us_to_ms_f64(servo_health.page_load_wait_max_us),
        servo_detached_destroys_total: servo_health.detached_destroys_total,
        servo_detached_destroy_failures_total: servo_health.detached_destroy_failures_total,
        servo_render_requests_total: servo_health.render_requests_total,
        servo_render_queue_wait_total_ms: us_to_ms_f64(servo_health.render_queue_wait_total_us),
        servo_render_queue_wait_max_ms: us_to_ms_f64(servo_health.render_queue_wait_max_us),
        servo_render_scene_requests_total: servo_health.render_scene_requests_total,
        servo_render_scene_queue_wait_total_ms: us_to_ms_f64(
            servo_health.render_scene_queue_wait_total_us,
        ),
        servo_render_scene_queue_wait_max_ms: us_to_ms_f64(
            servo_health.render_scene_queue_wait_max_us,
        ),
        servo_render_display_requests_total: servo_health.render_display_requests_total,
        servo_render_display_queue_wait_total_ms: us_to_ms_f64(
            servo_health.render_display_queue_wait_total_us,
        ),
        servo_render_display_queue_wait_max_ms: us_to_ms_f64(
            servo_health.render_display_queue_wait_max_us,
        ),
        servo_render_cpu_frames_total: servo_health.render_cpu_frames_total,
        servo_render_cached_frames_total: servo_health.render_cached_frames_total,
        servo_render_gpu_frames_total: servo_health.render_gpu_frames_total,
        servo_gpu_import_failures_total: servo_health.render_gpu_import_failures_total,
        servo_gpu_import_fallbacks_total: servo_health.render_gpu_import_fallbacks_total,
        servo_gpu_import_fallback_reason: servo_health.render_gpu_import_fallback_reason,
        servo_gpu_import_windows_sync_mode: servo_health.render_gpu_import_windows_sync_mode,
        servo_gpu_import_stale_frame_total: servo_health.render_gpu_import_stale_frame_total,
        servo_gpu_import_adapter_mismatch_total: servo_health
            .render_gpu_import_adapter_mismatch_total,
        servo_gpu_import_slot_count: servo_health.render_gpu_import_slot_count,
        servo_gpu_import_pending_slots: servo_health.render_gpu_import_pending_slots,
        servo_gpu_import_pending_slots_max: servo_health.render_gpu_import_pending_slots_max,
        servo_gpu_import_completed_slots: servo_health.render_gpu_import_completed_slots,
        servo_gpu_import_available_slots: servo_health.render_gpu_import_available_slots,
        servo_gpu_import_available_slots_min: servo_health.render_gpu_import_available_slots_min,
        servo_gpu_import_oldest_pending_age_max_ms: us_to_ms_f64(
            servo_health.render_gpu_import_oldest_pending_age_max_us,
        ),
        servo_gpu_import_blit_total_ms: us_to_ms_f64(servo_health.render_gpu_import_blit_total_us),
        servo_gpu_import_blit_max_ms: us_to_ms_f64(servo_health.render_gpu_import_blit_max_us),
        servo_gpu_import_sync_total_ms: us_to_ms_f64(servo_health.render_gpu_import_sync_total_us),
        servo_gpu_import_sync_max_ms: us_to_ms_f64(servo_health.render_gpu_import_sync_max_us),
        servo_gpu_import_total_ms: us_to_ms_f64(servo_health.render_gpu_import_total_us),
        servo_gpu_import_max_ms: us_to_ms_f64(servo_health.render_gpu_import_max_us),
        producer_cpu_frames_total: pipeline_health.cpu_producer_frames,
        producer_gpu_frames_total: pipeline_health.gpu_producer_frames,
        producer_gpu_cpu_materialization_blocked_total: pipeline_health
            .gpu_cpu_materialization_blocked_total,
        sparkleflinger_gpu_source_upload_skipped_total: pipeline_health.skipped_gpu_source_uploads,
        sparkleflinger_media_texture_allocations_total: pipeline_health
            .media_texture_allocations_total,
        sparkleflinger_media_texture_upload_bytes_total: pipeline_health
            .media_texture_upload_bytes_total,
        sparkleflinger_display_finalize_rgba_attempts_total: pipeline_health
            .display_finalize_rgba_attempts_total,
        sparkleflinger_display_finalize_yuv_attempts_total: pipeline_health
            .display_finalize_yuv_attempts_total,
        sparkleflinger_display_finalize_successes_total: pipeline_health
            .display_finalize_successes_total,
        sparkleflinger_display_finalize_misses_total: pipeline_health.display_finalize_misses_total,
        sparkleflinger_display_finalize_latches_total: pipeline_health
            .display_finalize_latches_total,
        sparkleflinger_display_finalize_blocking_wait_total_ms: us_to_ms_f64(
            pipeline_health.display_finalize_blocking_wait_total_us,
        ),
        sparkleflinger_display_finalize_blocking_wait_max_ms: us_to_ms_f64(
            pipeline_health.display_finalize_blocking_wait_max_us,
        ),
        sparkleflinger_display_finalize_surface_reallocs_total: pipeline_health
            .display_finalize_surface_reallocs_total,
        servo_render_evaluate_scripts_total_ms: us_to_ms_f64(
            servo_health.render_evaluate_scripts_total_us,
        ),
        servo_render_evaluate_scripts_max_ms: us_to_ms_f64(
            servo_health.render_evaluate_scripts_max_us,
        ),
        servo_render_event_loop_total_ms: us_to_ms_f64(servo_health.render_event_loop_total_us),
        servo_render_event_loop_max_ms: us_to_ms_f64(servo_health.render_event_loop_max_us),
        servo_render_paint_total_ms: us_to_ms_f64(servo_health.render_paint_total_us),
        servo_render_paint_max_ms: us_to_ms_f64(servo_health.render_paint_max_us),
        servo_render_readback_total_ms: us_to_ms_f64(servo_health.render_readback_total_us),
        servo_render_readback_max_ms: us_to_ms_f64(servo_health.render_readback_max_us),
        servo_render_frame_total_ms: us_to_ms_f64(servo_health.render_frame_total_us),
        servo_render_frame_max_ms: us_to_ms_f64(servo_health.render_frame_max_us),
    };
    let preview_runtime = preview_runtime_status(&state.preview_runtime);

    let input_status = input_status_snapshot_with_privacy(&state, include_private_diagnostics);
    let audio_available = input_status.sources.iter().any(|source| {
        source.kind == "audio"
            && !source.retired
            && !matches!(source.state.as_str(), "unavailable" | "failed")
    });
    let screen_capture_capacity = {
        let capacity_snapshot = state.screen_capacity_status.snapshot();
        let policy = capacity_snapshot.policy();
        if policy.capacity_enforced() {
            let resource_snapshot = capacity_snapshot.physical();
            let resource_capacity = resource_snapshot.capacity();
            let total_capacity = policy.total_capacity();
            let publication_capacity = policy.publication_capacity();
            let demand = policy.capture_demand();
            let analysis = policy.analysis_resource_plan();
            let analysis_work = policy.analysis_work_plan();
            let analysis_compute = policy.analysis_compute_capacity();
            let extent = demand.requested_extent();
            ScreenCaptureCapacityStatus {
                admission_enforced: true,
                physical_transition_byte_capacity: Some(resource_capacity.byte_budget()),
                physical_transition_backend_capacity: Some(resource_capacity.backend_capacity()),
                physical_reserved_bytes: Some(resource_snapshot.reserved_bytes()),
                physical_available_bytes: Some(resource_snapshot.available_bytes()),
                steady_total_byte_budget: Some(total_capacity.byte_budget()),
                steady_total_backend_capacity: Some(total_capacity.backend_capacity()),
                steady_publication_byte_budget: Some(publication_capacity.byte_budget()),
                transition_publication_backend_capacity: Some(
                    publication_capacity.backend_capacity(),
                ),
                analysis_width: extent.map(PixelExtent::width),
                analysis_height: extent.map(PixelExtent::height),
                analysis_retained_bytes: analysis.map(ScreenAnalysisResourcePlan::retained_bytes),
                analysis_peak_bytes: analysis.map(ScreenAnalysisResourcePlan::peak_bytes),
                analysis_weighted_work_units_per_frame: analysis_work
                    .map(ScreenAnalysisWorkPlan::weighted_work_units_per_frame),
                analysis_weighted_work_units_per_second: analysis_work
                    .map(ScreenAnalysisWorkPlan::weighted_work_units_per_second),
                analysis_parallel_capacity_per_second: analysis_compute.and_then(
                    ScreenAnalysisComputeCapacity::total_parallel_weighted_work_units_per_second,
                ),
                analysis_serial_capacity_per_second: analysis_compute
                    .map(|capacity| capacity.serial_weighted_work_units_per_second().get()),
                analysis_worker_count: analysis_compute
                    .and_then(|capacity| u64::try_from(capacity.worker_count().get()).ok()),
            }
        } else {
            ScreenCaptureCapacityStatus::without_capacity(false)
        }
    };

    let uptime_seconds = state.start_time.elapsed().as_secs();
    let config_path = config_path(&state).display().to_string();
    let data_dir = ConfigManager::data_dir().display().to_string();
    let cache_dir = ConfigManager::cache_dir().display().to_string();
    let macos_daemon_ownership = state
        .macos_daemon_ownership
        .load_full()
        .as_deref()
        .map(macos_daemon_ownership);

    SystemStatus {
        running,
        version: env!("CARGO_PKG_VERSION").to_owned(),
        server: state.server_identity.clone(),
        config_path,
        data_dir,
        cache_dir,
        uptime_seconds,
        device_count,
        effect_count,
        scene_count,
        active_effect,
        active_scene,
        active_scene_snapshot_locked,
        global_brightness: brightness_percent(current_global_brightness(&state.power_state)),
        audio_available,
        capture_available: capture_input_available(),
        screen_capture_capacity,
        input: input_status,
        macos_daemon_ownership,
        compositor_acceleration: render_acceleration_status(&state.render_acceleration),
        render_loop: render_loop_status,
        session_performance,
        latest_frame,
        effect_health,
        preview_runtime,
        event_bus_subscribers: subscribers,
        capabilities: MULTI_ZONE_CAPABILITIES
            .iter()
            .map(|capability| (*capability).to_owned())
            .collect(),
    }
}

/// `GET /api/v1/system` -- Public identity with authorized daemon status.
#[utoipa::path(
    get,
    path = "/api/v1/system",
    responses(
        (
            status = 200,
            description = "Daemon identity and authorized status",
            body = crate::api::envelope::ApiResponse<SystemResource>
        )
    ),
    tag = "system"
)]
pub(crate) async fn get_system(
    State(state): State<Arc<AppState>>,
    Extension(auth_context): Extension<RequestAuthContext>,
) -> Response {
    let identity = server_info(&state).await;
    let status = if auth_context.can_read_system_status() {
        Some(
            system_status_with_privacy(Arc::clone(&state), auth_context.can_protected_control())
                .await,
        )
    } else {
        None
    };

    ApiResponse::ok(SystemResource { identity, status })
}

/// `GET /api/v1/system/sensors` — Latest system sensor snapshot.
pub async fn get_sensors(State(state): State<Arc<AppState>>) -> Response {
    ApiResponse::ok(latest_sensor_snapshot(&state).await.as_ref().clone())
}

/// `GET /api/v1/system/sensors/{label}` — Resolve one named sensor.
pub async fn get_sensor(State(state): State<Arc<AppState>>, Path(label): Path<String>) -> Response {
    let snapshot = latest_sensor_snapshot(&state).await;
    if let Some(reading) = snapshot.reading(&label) {
        return ApiResponse::ok(reading);
    }

    DomainError::not_found(ResourceKind::Sensor, &label).into_response()
}

/// `GET /api/v1/server` — Lightweight server identity for discovery probes.
#[utoipa::path(
    get,
    path = "/api/v1/server",
    responses(
        (
            status = 200,
            description = "Lightweight server identity for discovery probes",
            body = crate::api::envelope::ApiResponse<ServerInfo>
        )
    ),
    tag = "system"
)]
pub async fn get_server(State(state): State<Arc<AppState>>) -> Response {
    ApiResponse::ok(server_info(&state).await)
}

async fn server_info(state: &AppState) -> ServerInfo {
    ServerInfo {
        identity: state.server_identity.clone(),
        server_session_id: state.server_session_id.clone(),
        device_count: state.device_registry.len().await,
        auth_required: state.security_state.security_enabled(),
    }
}

/// `GET /health` — Lightweight health check (no envelope).
#[utoipa::path(
    get,
    path = "/health",
    responses(
        (status = 200, description = "Daemon is healthy", body = HealthResponse),
        (status = 503, description = "Daemon is degraded", body = HealthResponse)
    ),
    tag = "system"
)]
pub async fn health_check(State(state): State<Arc<AppState>>) -> Response {
    let uptime_seconds = state.start_time.elapsed().as_secs();
    let render_loop = {
        let render_loop = state.render_loop.read().await;
        render_loop_health(render_loop.stats().state).to_owned()
    };
    let device_count = state.device_registry.len().await;
    let device_backends = {
        let backend_manager = state.backend_manager.lock().await;
        backend_health(backend_manager.backend_count(), device_count).to_owned()
    };
    let event_bus = event_bus_health(&state.event_bus).to_owned();
    let checks = HealthChecks {
        render_loop,
        device_backends,
        event_bus,
    };

    let health = overall_health(&checks);
    let resp = HealthResponse {
        status: health.to_owned(),
        version: env!("CARGO_PKG_VERSION").to_owned(),
        uptime_seconds,
        checks,
    };

    let status = match health {
        "healthy" => axum::http::StatusCode::OK,
        _ => axum::http::StatusCode::SERVICE_UNAVAILABLE,
    };

    (status, axum::Json(resp)).into_response()
}

fn config_path(state: &AppState) -> PathBuf {
    state.config_manager.as_ref().map_or_else(
        || ConfigManager::config_dir().join(DEFAULT_CONFIG_FILE_NAME),
        |manager| manager.path().to_path_buf(),
    )
}

pub(crate) async fn latest_sensor_snapshot(state: &AppState) -> Arc<SystemSnapshot> {
    let graph = state.input_manager.input_graph_handle();
    graph
        .snapshot()
        .latest_data_source(DataSourceKind::Sensors)
        .and_then(|sample| match sample.as_ref() {
            InputData::Sensors(snapshot) => Some(Arc::clone(snapshot)),
            _ => None,
        })
        .unwrap_or_else(|| Arc::new(SystemSnapshot::empty()))
}

#[allow(
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "brightness is clamped to 0-100 percent before narrowing to a byte"
)]
fn brightness_percent(brightness: f32) -> u8 {
    let scaled = (brightness.clamp(0.0, 1.0) * 100.0).round();
    if scaled <= 0.0 {
        0
    } else if scaled >= 100.0 {
        100
    } else {
        scaled as u8
    }
}

fn render_loop_health(state: RenderLoopState) -> &'static str {
    match state {
        RenderLoopState::Running => "ok",
        RenderLoopState::Created | RenderLoopState::Paused => "idle",
        RenderLoopState::Stopped => "degraded",
    }
}

fn backend_health(backend_count: usize, device_count: usize) -> &'static str {
    if backend_count == 0 && device_count > 0 {
        "degraded"
    } else if backend_count == 0 {
        "idle"
    } else {
        "ok"
    }
}

fn event_bus_health(bus: &hypercolor_core::bus::HypercolorBus) -> &'static str {
    if bus.subscriber_count() == 0
        && bus.frame_receiver_count() == 0
        && bus.spectrum_receiver_count() == 0
        && bus.canvas_receiver_count() == 0
    {
        "idle"
    } else {
        "ok"
    }
}

fn overall_health(checks: &HealthChecks) -> &'static str {
    if [
        checks.render_loop.as_str(),
        checks.device_backends.as_str(),
        checks.event_bus.as_str(),
    ]
    .contains(&"degraded")
    {
        "degraded"
    } else {
        "healthy"
    }
}

fn render_loop_is_operational(state: &str) -> bool {
    state != "stopped"
}

fn render_acceleration_status(
    resolution: &crate::startup::CompositorAccelerationResolution,
) -> RenderAccelerationStatus {
    RenderAccelerationStatus {
        requested_mode: render_acceleration_mode_name(resolution.requested_mode).to_owned(),
        effective_mode: render_acceleration_mode_name(resolution.effective_mode).to_owned(),
        fallback_reason: resolution.fallback_reason.map(str::to_owned),
        servo_gpu_import_mode: servo_gpu_import_mode_name().to_owned(),
        servo_gpu_import_attempting: servo_gpu_import_attempting(),
        gpu_probe: resolution
            .gpu_probe
            .as_ref()
            .map(|probe| GpuCompositorProbeStatus {
                adapter_name: probe.adapter_name.clone(),
                adapter_device_type: probe.adapter_device_type.to_owned(),
                backend: probe.backend.to_owned(),
                texture_format: probe.texture_format.to_owned(),
                max_texture_dimension_2d: probe.max_texture_dimension_2d,
                max_storage_textures_per_shader_stage: probe.max_storage_textures_per_shader_stage,
                software_adapter_reason: probe.software_adapter_reason.map(str::to_owned),
                servo_gpu_import_backend_compatible: probe.servo_gpu_import_backend_compatible,
                servo_gpu_import_backend_reason: probe
                    .servo_gpu_import_backend_reason
                    .map(str::to_owned),
                linux_servo_gpu_import_backend_compatible: probe
                    .linux_servo_gpu_import_backend_compatible,
                linux_servo_gpu_import_backend_reason: probe
                    .linux_servo_gpu_import_backend_reason
                    .map(str::to_owned),
            }),
    }
}

#[cfg(feature = "servo-gpu-import")]
fn servo_gpu_import_mode_name() -> &'static str {
    match hypercolor_core::effect::servo_gpu_import_mode() {
        hypercolor_types::config::ServoGpuImportMode::Off => "off",
        hypercolor_types::config::ServoGpuImportMode::Auto => "auto",
        hypercolor_types::config::ServoGpuImportMode::On => "on",
    }
}

#[cfg(not(feature = "servo-gpu-import"))]
const fn servo_gpu_import_mode_name() -> &'static str {
    "unavailable"
}

#[cfg(feature = "servo-gpu-import")]
fn servo_gpu_import_attempting() -> bool {
    hypercolor_core::effect::servo_gpu_import_should_attempt()
}

#[cfg(not(feature = "servo-gpu-import"))]
const fn servo_gpu_import_attempting() -> bool {
    false
}

const fn render_acceleration_mode_name(mode: RenderAccelerationMode) -> &'static str {
    match mode {
        RenderAccelerationMode::Cpu => "cpu",
        RenderAccelerationMode::Auto => "auto",
        RenderAccelerationMode::Gpu => "gpu",
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct ServoEffectHealthCounts {
    soft_stalls_total: u64,
    breaker_opens_total: u64,
    session_creates_total: u64,
    session_create_failures_total: u64,
    session_create_wait_total_us: u64,
    session_create_wait_max_us: u64,
    page_loads_total: u64,
    page_load_failures_total: u64,
    page_load_wait_total_us: u64,
    page_load_wait_max_us: u64,
    detached_destroys_total: u64,
    detached_destroy_failures_total: u64,
    render_requests_total: u64,
    render_queue_wait_total_us: u64,
    render_queue_wait_max_us: u64,
    render_scene_requests_total: u64,
    render_scene_queue_wait_total_us: u64,
    render_scene_queue_wait_max_us: u64,
    render_display_requests_total: u64,
    render_display_queue_wait_total_us: u64,
    render_display_queue_wait_max_us: u64,
    render_cpu_frames_total: u64,
    render_cached_frames_total: u64,
    render_gpu_frames_total: u64,
    render_gpu_import_failures_total: u64,
    render_gpu_import_fallbacks_total: u64,
    render_gpu_import_fallback_reason: Option<&'static str>,
    render_gpu_import_windows_sync_mode: Option<&'static str>,
    render_gpu_import_stale_frame_total: u64,
    render_gpu_import_adapter_mismatch_total: u64,
    render_gpu_import_slot_count: u64,
    render_gpu_import_pending_slots: u64,
    render_gpu_import_pending_slots_max: u64,
    render_gpu_import_completed_slots: u64,
    render_gpu_import_available_slots: u64,
    render_gpu_import_available_slots_min: u64,
    render_gpu_import_oldest_pending_age_max_us: u64,
    render_gpu_import_blit_total_us: u64,
    render_gpu_import_blit_max_us: u64,
    render_gpu_import_sync_total_us: u64,
    render_gpu_import_sync_max_us: u64,
    render_gpu_import_total_us: u64,
    render_gpu_import_max_us: u64,
    render_evaluate_scripts_total_us: u64,
    render_evaluate_scripts_max_us: u64,
    render_event_loop_total_us: u64,
    render_event_loop_max_us: u64,
    render_paint_total_us: u64,
    render_paint_max_us: u64,
    render_readback_total_us: u64,
    render_readback_max_us: u64,
    render_frame_total_us: u64,
    render_frame_max_us: u64,
}

#[cfg(feature = "servo")]
fn servo_effect_health_counts() -> ServoEffectHealthCounts {
    let snapshot = hypercolor_core::effect::servo_telemetry_snapshot();
    ServoEffectHealthCounts {
        soft_stalls_total: snapshot.soft_stalls_total,
        breaker_opens_total: snapshot.breaker_opens_total,
        session_creates_total: snapshot.session_creates_total,
        session_create_failures_total: snapshot.session_create_failures_total,
        session_create_wait_total_us: snapshot.session_create_wait_total_us,
        session_create_wait_max_us: snapshot.session_create_wait_max_us,
        page_loads_total: snapshot.page_loads_total,
        page_load_failures_total: snapshot.page_load_failures_total,
        page_load_wait_total_us: snapshot.page_load_wait_total_us,
        page_load_wait_max_us: snapshot.page_load_wait_max_us,
        detached_destroys_total: snapshot.detached_destroys_total,
        detached_destroy_failures_total: snapshot.detached_destroy_failures_total,
        render_requests_total: snapshot.render_requests_total,
        render_queue_wait_total_us: snapshot.render_queue_wait_total_us,
        render_queue_wait_max_us: snapshot.render_queue_wait_max_us,
        render_scene_requests_total: snapshot.render_scene_requests_total,
        render_scene_queue_wait_total_us: snapshot.render_scene_queue_wait_total_us,
        render_scene_queue_wait_max_us: snapshot.render_scene_queue_wait_max_us,
        render_display_requests_total: snapshot.render_display_requests_total,
        render_display_queue_wait_total_us: snapshot.render_display_queue_wait_total_us,
        render_display_queue_wait_max_us: snapshot.render_display_queue_wait_max_us,
        render_cpu_frames_total: snapshot.render_cpu_frames_total,
        render_cached_frames_total: snapshot.render_cached_frames_total,
        render_gpu_frames_total: snapshot.render_gpu_frames_total,
        render_gpu_import_failures_total: snapshot.render_gpu_import_failures_total,
        render_gpu_import_fallbacks_total: snapshot.render_gpu_import_fallbacks_total,
        render_gpu_import_fallback_reason: snapshot.render_gpu_import_fallback_reason,
        render_gpu_import_windows_sync_mode: snapshot.render_gpu_import_windows_sync_mode,
        render_gpu_import_stale_frame_total: snapshot.render_gpu_import_stale_frame_total,
        render_gpu_import_adapter_mismatch_total: snapshot.render_gpu_import_adapter_mismatch_total,
        render_gpu_import_slot_count: snapshot.render_gpu_import_slot_count,
        render_gpu_import_pending_slots: snapshot.render_gpu_import_pending_slots,
        render_gpu_import_pending_slots_max: snapshot.render_gpu_import_pending_slots_max,
        render_gpu_import_completed_slots: snapshot.render_gpu_import_completed_slots,
        render_gpu_import_available_slots: snapshot.render_gpu_import_available_slots,
        render_gpu_import_available_slots_min: snapshot.render_gpu_import_available_slots_min,
        render_gpu_import_oldest_pending_age_max_us: snapshot
            .render_gpu_import_oldest_pending_age_max_us,
        render_gpu_import_blit_total_us: snapshot.render_gpu_import_blit_total_us,
        render_gpu_import_blit_max_us: snapshot.render_gpu_import_blit_max_us,
        render_gpu_import_sync_total_us: snapshot.render_gpu_import_sync_total_us,
        render_gpu_import_sync_max_us: snapshot.render_gpu_import_sync_max_us,
        render_gpu_import_total_us: snapshot.render_gpu_import_total_us,
        render_gpu_import_max_us: snapshot.render_gpu_import_max_us,
        render_evaluate_scripts_total_us: snapshot.render_evaluate_scripts_total_us,
        render_evaluate_scripts_max_us: snapshot.render_evaluate_scripts_max_us,
        render_event_loop_total_us: snapshot.render_event_loop_total_us,
        render_event_loop_max_us: snapshot.render_event_loop_max_us,
        render_paint_total_us: snapshot.render_paint_total_us,
        render_paint_max_us: snapshot.render_paint_max_us,
        render_readback_total_us: snapshot.render_readback_total_us,
        render_readback_max_us: snapshot.render_readback_max_us,
        render_frame_total_us: snapshot.render_frame_total_us,
        render_frame_max_us: snapshot.render_frame_max_us,
    }
}

#[cfg(not(feature = "servo"))]
const fn servo_effect_health_counts() -> ServoEffectHealthCounts {
    ServoEffectHealthCounts {
        soft_stalls_total: 0,
        breaker_opens_total: 0,
        session_creates_total: 0,
        session_create_failures_total: 0,
        session_create_wait_total_us: 0,
        session_create_wait_max_us: 0,
        page_loads_total: 0,
        page_load_failures_total: 0,
        page_load_wait_total_us: 0,
        page_load_wait_max_us: 0,
        detached_destroys_total: 0,
        detached_destroy_failures_total: 0,
        render_requests_total: 0,
        render_queue_wait_total_us: 0,
        render_queue_wait_max_us: 0,
        render_scene_requests_total: 0,
        render_scene_queue_wait_total_us: 0,
        render_scene_queue_wait_max_us: 0,
        render_display_requests_total: 0,
        render_display_queue_wait_total_us: 0,
        render_display_queue_wait_max_us: 0,
        render_cpu_frames_total: 0,
        render_cached_frames_total: 0,
        render_gpu_frames_total: 0,
        render_gpu_import_failures_total: 0,
        render_gpu_import_fallbacks_total: 0,
        render_gpu_import_fallback_reason: None,
        render_gpu_import_windows_sync_mode: None,
        render_gpu_import_stale_frame_total: 0,
        render_gpu_import_adapter_mismatch_total: 0,
        render_gpu_import_slot_count: 0,
        render_gpu_import_pending_slots: 0,
        render_gpu_import_pending_slots_max: 0,
        render_gpu_import_completed_slots: 0,
        render_gpu_import_available_slots: 0,
        render_gpu_import_available_slots_min: 0,
        render_gpu_import_oldest_pending_age_max_us: 0,
        render_gpu_import_blit_total_us: 0,
        render_gpu_import_blit_max_us: 0,
        render_gpu_import_sync_total_us: 0,
        render_gpu_import_sync_max_us: 0,
        render_gpu_import_total_us: 0,
        render_gpu_import_max_us: 0,
        render_evaluate_scripts_total_us: 0,
        render_evaluate_scripts_max_us: 0,
        render_event_loop_total_us: 0,
        render_event_loop_max_us: 0,
        render_paint_total_us: 0,
        render_paint_max_us: 0,
        render_readback_total_us: 0,
        render_readback_max_us: 0,
        render_frame_total_us: 0,
        render_frame_max_us: 0,
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct RenderPipelineHealthCounts {
    cpu_producer_frames: u64,
    gpu_producer_frames: u64,
    gpu_cpu_materialization_blocked_total: u64,
    skipped_gpu_source_uploads: u64,
    media_texture_allocations_total: u64,
    media_texture_upload_bytes_total: u64,
    display_finalize_rgba_attempts_total: u64,
    display_finalize_yuv_attempts_total: u64,
    display_finalize_successes_total: u64,
    display_finalize_misses_total: u64,
    display_finalize_latches_total: u64,
    display_finalize_blocking_wait_total_us: u64,
    display_finalize_blocking_wait_max_us: u64,
    display_finalize_surface_reallocs_total: u64,
}

fn render_pipeline_health_counts() -> RenderPipelineHealthCounts {
    let producer = crate::render_thread::producer_frame_counts();
    let gpu = gpu_sparkleflinger_health_counts();
    RenderPipelineHealthCounts {
        cpu_producer_frames: producer.cpu_frames,
        gpu_producer_frames: producer.gpu_frames,
        gpu_cpu_materialization_blocked_total: producer.gpu_cpu_materialization_blocked,
        skipped_gpu_source_uploads: gpu.source_upload_skipped_total,
        media_texture_allocations_total: gpu.media_texture_allocations_total,
        media_texture_upload_bytes_total: gpu.media_texture_upload_bytes_total,
        display_finalize_rgba_attempts_total: gpu.display_finalize_rgba_attempts_total,
        display_finalize_yuv_attempts_total: gpu.display_finalize_yuv_attempts_total,
        display_finalize_successes_total: gpu.display_finalize_successes_total,
        display_finalize_misses_total: gpu.display_finalize_misses_total,
        display_finalize_latches_total: gpu.display_finalize_latches_total,
        display_finalize_blocking_wait_total_us: gpu.display_finalize_blocking_wait_total_us,
        display_finalize_blocking_wait_max_us: gpu.display_finalize_blocking_wait_max_us,
        display_finalize_surface_reallocs_total: gpu.display_finalize_surface_reallocs_total,
    }
}

#[cfg(feature = "wgpu")]
fn gpu_sparkleflinger_health_counts() -> GpuSparkleFlingerHealthCounts {
    let snapshot =
        crate::render_thread::sparkleflinger::gpu::gpu_sparkleflinger_telemetry_snapshot();
    GpuSparkleFlingerHealthCounts {
        source_upload_skipped_total: snapshot.source_upload_skipped_total,
        media_texture_allocations_total: snapshot.media_texture_allocations_total,
        media_texture_upload_bytes_total: snapshot.media_texture_upload_bytes_total,
        display_finalize_rgba_attempts_total: snapshot.display_finalize_rgba_attempts_total,
        display_finalize_yuv_attempts_total: snapshot.display_finalize_yuv_attempts_total,
        display_finalize_successes_total: snapshot.display_finalize_successes_total,
        display_finalize_misses_total: snapshot.display_finalize_misses_total,
        display_finalize_latches_total: snapshot.display_finalize_latches_total,
        display_finalize_blocking_wait_total_us: snapshot.display_finalize_blocking_wait_total_us,
        display_finalize_blocking_wait_max_us: snapshot.display_finalize_blocking_wait_max_us,
        display_finalize_surface_reallocs_total: snapshot.display_finalize_surface_reallocs_total,
    }
}

#[cfg(not(feature = "wgpu"))]
const fn gpu_sparkleflinger_health_counts() -> GpuSparkleFlingerHealthCounts {
    GpuSparkleFlingerHealthCounts {
        source_upload_skipped_total: 0,
        media_texture_allocations_total: 0,
        media_texture_upload_bytes_total: 0,
        display_finalize_rgba_attempts_total: 0,
        display_finalize_yuv_attempts_total: 0,
        display_finalize_successes_total: 0,
        display_finalize_misses_total: 0,
        display_finalize_latches_total: 0,
        display_finalize_blocking_wait_total_us: 0,
        display_finalize_blocking_wait_max_us: 0,
        display_finalize_surface_reallocs_total: 0,
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct GpuSparkleFlingerHealthCounts {
    source_upload_skipped_total: u64,
    media_texture_allocations_total: u64,
    media_texture_upload_bytes_total: u64,
    display_finalize_rgba_attempts_total: u64,
    display_finalize_yuv_attempts_total: u64,
    display_finalize_successes_total: u64,
    display_finalize_misses_total: u64,
    display_finalize_latches_total: u64,
    display_finalize_blocking_wait_total_us: u64,
    display_finalize_blocking_wait_max_us: u64,
    display_finalize_surface_reallocs_total: u64,
}

fn latest_frame_status(frame: &LatestFrameMetrics, render_elapsed_ms: f64) -> LatestFrameStatus {
    let frame_age_ms = if frame.timestamp_ms > 0 {
        (render_elapsed_ms - f64::from(frame.timestamp_ms)).max(0.0)
    } else {
        0.0
    };

    LatestFrameStatus {
        frame_token: frame.timeline.frame_token,
        compositor_backend: frame.compositor_backend.as_str().to_owned(),
        output_frame_source: frame.output_frame_source.as_str().to_owned(),
        output_reuses_published_frame: frame.output_reuses_published_frame,
        output_brightness_bits: frame.output_brightness_bits,
        output_brightness_generation: frame.output_brightness_generation,
        output_routing_signature: frame.output_routing_signature,
        output_zone_shape_signature: frame.output_zone_shape_signature,
        output_unassigned_behavior_generation: frame.output_unassigned_behavior_generation,
        devices_written: frame.devices_written,
        total_leds: frame.total_leds,
        gpu_zone_sampling: frame.gpu_zone_sampling,
        gpu_sample_deferred: frame.gpu_sample_deferred,
        gpu_sample_stale: frame.gpu_sample_stale,
        gpu_sample_retry_hit: frame.gpu_sample_retry_hit,
        gpu_sample_queue_saturated: frame.gpu_sample_queue_saturated,
        gpu_sample_wait_blocked: frame.gpu_sample_wait_blocked,
        gpu_sample_cpu_fallback: frame.gpu_sample_cpu_fallback,
        cpu_sampling_late_readback: false,
        led_sampling_readback: false,
        preview_surface: frame.preview_surface,
        scene_canvas_forced_surface: frame.scene_canvas_forced_surface,
        cpu_readback_skipped: frame.cpu_readback_skipped,
        gpu_readback_failed: frame.gpu_readback_failed,
        total_ms: round_2(us_to_ms(frame.total_us)),
        wake_late_ms: round_2(us_to_ms(frame.wake_late_us)),
        jitter_ms: round_2(us_to_ms(frame.jitter_us)),
        frame_age_ms: round_2(frame_age_ms),
        input_sampling_ms: round_2(us_to_ms(frame.input_us)),
        producer_ms: round_2(us_to_ms(frame.producer_us)),
        producer_render_ms: round_2(us_to_ms(frame.producer_render_us)),
        producer_scene_compose_ms: round_2(us_to_ms(frame.producer_scene_compose_us)),
        composition_ms: round_2(us_to_ms(frame.composition_us)),
        effect_rendering_ms: round_2(us_to_ms(frame.render_us)),
        spatial_sampling_ms: round_2(us_to_ms(frame.sample_us)),
        device_output_ms: round_2(us_to_ms(frame.push_us)),
        preview_postprocess_ms: round_2(us_to_ms(frame.postprocess_us)),
        event_bus_ms: round_2(us_to_ms(frame.publish_us)),
        coordination_overhead_ms: round_2(us_to_ms(frame.overhead_us)),
        publish_frame_data_ms: round_2(us_to_ms(frame.publish_frame_data_us)),
        publish_group_canvas_ms: round_2(us_to_ms(frame.publish_group_canvas_us)),
        publish_preview_ms: round_2(us_to_ms(frame.publish_preview_us)),
        publish_events_ms: round_2(us_to_ms(frame.publish_events_us)),
        logical_layer_count: frame.logical_layer_count,
        render_group_count: frame.render_group_count,
        full_frame_copy_count: frame.full_frame_copy_count,
        full_frame_copy_kb: round_2(bytes_to_kib(frame.full_frame_copy_bytes)),
        producer_full_frame_copy_count: frame.producer_full_frame_copy.count,
        producer_full_frame_copy_kb: round_2(bytes_to_kib(frame.producer_full_frame_copy.bytes)),
        producer_full_frame_copy_reason: frame.producer_full_frame_copy.reason.map(str::to_owned),
        publication_full_frame_copy_count: frame.publication_full_frame_copy.count,
        publication_full_frame_copy_kb: round_2(bytes_to_kib(
            frame.publication_full_frame_copy.bytes,
        )),
        publication_full_frame_copy_reason: frame
            .publication_full_frame_copy
            .reason
            .map(str::to_owned),
        output_errors: frame.output_errors,
        render_surfaces: RenderSurfaceStatus {
            slot_count: frame.render_surface_slot_count,
            free_slots: frame.render_surface_free_slots,
            published_slots: frame.render_surface_published_slots,
            dequeued_slots: frame.render_surface_dequeued_slots,
            canvas_receivers: frame.canvas_receiver_count,
            scene_pool_slot_count: frame.scene_pool_slot_count,
            scene_pool_free_slots: frame.scene_pool_free_slots,
            scene_pool_published_slots: frame.scene_pool_published_slots,
            scene_pool_dequeued_slots: frame.scene_pool_dequeued_slots,
            direct_pool_slot_count: frame.direct_pool_slot_count,
            direct_pool_free_slots: frame.direct_pool_free_slots,
            direct_pool_published_slots: frame.direct_pool_published_slots,
            direct_pool_dequeued_slots: frame.direct_pool_dequeued_slots,
            preview_pool_slot_count: frame.preview_pool_slot_count,
            preview_pool_free_slots: frame.preview_pool_free_slots,
            preview_pool_published_slots: frame.preview_pool_published_slots,
            preview_pool_dequeued_slots: frame.preview_pool_dequeued_slots,
            compositor_pool_slot_count: frame.compositor_pool_slot_count,
            compositor_pool_free_slots: frame.compositor_pool_free_slots,
            compositor_pool_published_slots: frame.compositor_pool_published_slots,
            compositor_pool_dequeued_slots: frame.compositor_pool_dequeued_slots,
        },
    }
}

fn preview_runtime_status(runtime: &PreviewRuntime) -> PreviewRuntimeStatus {
    let snapshot = runtime.snapshot();
    PreviewRuntimeStatus {
        canvas_receivers: snapshot.canvas_receivers,
        scene_canvas_receivers: snapshot.scene_canvas_receivers,
        screen_canvas_receivers: snapshot.screen_canvas_receivers,
        zone_preview_receivers: snapshot.zone_preview_receivers,
        canvas_frames_published: snapshot.canvas_frames_published,
        scene_canvas_frames_published: snapshot.scene_canvas_frames_published,
        screen_canvas_frames_published: snapshot.screen_canvas_frames_published,
        zone_preview_frames_published: snapshot.zone_preview_frames_published,
        latest_canvas_frame_number: snapshot.latest_canvas_frame_number,
        latest_scene_canvas_frame_number: snapshot.latest_scene_canvas_frame_number,
        latest_screen_canvas_frame_number: snapshot.latest_screen_canvas_frame_number,
        latest_zone_preview_frame_number: snapshot.latest_zone_preview_frame_number,
        canvas_demand: preview_demand_status(runtime.canvas_demand()),
        scene_canvas_demand: preview_demand_status(runtime.scene_canvas_demand()),
        screen_canvas_demand: preview_demand_status(runtime.screen_canvas_demand()),
        zone_preview_demand: preview_demand_status(runtime.zone_preview_demand()),
    }
}

fn preview_demand_status(summary: PreviewDemandSummary) -> PreviewDemandStatus {
    PreviewDemandStatus {
        subscribers: summary.subscribers,
        max_fps: summary.max_fps,
        max_width: summary.max_width,
        max_height: summary.max_height,
        any_full_resolution: summary.any_full_resolution,
        any_rgb: summary.any_rgb,
        any_rgba: summary.any_rgba,
        any_jpeg: summary.any_jpeg,
    }
}

fn paced_fps(avg_frame_secs: f64, target_fps: u32) -> f64 {
    if avg_frame_secs <= 0.0 {
        return f64::from(target_fps);
    }

    (1.0 / avg_frame_secs).clamp(0.0, f64::from(target_fps))
}

fn us_to_ms(value: u32) -> f64 {
    f64::from(value) / 1000.0
}

fn us_to_ms_f64(value: u64) -> f64 {
    std::time::Duration::from_micros(value).as_secs_f64() * 1000.0
}

fn bytes_to_kib(value: u32) -> f64 {
    f64::from(value) / 1024.0
}

fn round_1(value: f64) -> f64 {
    (value * 10.0).round() / 10.0
}

fn round_2(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

// ── Audio Devices ────────────────────────────────────────────────────────

/// One selectable audio input source.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct AudioDeviceInfo {
    pub id: String,
    pub name: String,
    pub description: String,
}

/// The audio input inventory plus the configured selection.
#[derive(Debug, Clone, Serialize)]
pub struct AudioDevicesResponse {
    pub devices: Vec<AudioDeviceInfo>,
    pub current: String,
}

/// `GET /api/v1/system/audio-devices` — Enumerate audio input devices.
pub async fn list_audio_devices(State(state): State<Arc<AppState>>) -> Response {
    let current = current_audio_device_id(&state);
    let devices = audio_device_options(&current);

    ApiResponse::ok(AudioDevicesResponse { devices, current })
}

pub(crate) fn capture_input_available() -> bool {
    if cfg!(any(target_os = "windows", target_os = "macos")) {
        return true;
    }
    cfg!(target_os = "linux") && std::env::var_os("WAYLAND_DISPLAY").is_some()
}

fn current_audio_device_id(state: &AppState) -> String {
    state.config_manager.as_ref().map_or_else(
        || "default".to_owned(),
        |manager| canonical_audio_device_id(&manager.get().audio.device),
    )
}

fn audio_device_options(current: &str) -> Vec<AudioDeviceInfo> {
    let mut devices = vec![
        default_audio_device(),
        microphone_audio_device(),
        disabled_audio_device(),
    ];

    match enumerate_audio_input_devices() {
        Ok(mut enumerated) => devices.append(&mut enumerated),
        Err(error) => {
            warn!(
                %error,
                "Failed to enumerate audio input devices; returning fallback settings options"
            );
        }
    }

    if should_include_current_device(current, &devices) {
        devices.push(AudioDeviceInfo {
            id: current.to_owned(),
            name: current.to_owned(),
            description: "Configured device (currently unavailable)".to_owned(),
        });
    }

    dedupe_audio_devices(&mut devices);
    devices.sort_by_cached_key(|device| {
        let rank = match device.id.as_str() {
            "default" => 0,
            "microphone" => 1,
            "none" => 2,
            _ => 3,
        };
        (rank, device.name.to_ascii_lowercase())
    });
    devices
}

fn enumerate_audio_input_devices() -> anyhow::Result<Vec<AudioDeviceInfo>> {
    #[cfg(target_os = "linux")]
    if let Ok(devices) = enumerate_linux_audio_input_devices()
        && !devices.is_empty()
    {
        return Ok(devices);
    }

    enumerate_cpal_audio_input_devices()
}

#[cfg(target_os = "linux")]
fn enumerate_linux_audio_input_devices() -> anyhow::Result<Vec<AudioDeviceInfo>> {
    Ok(linux::enumerate_named_audio_sources()?
        .into_iter()
        .map(|source| AudioDeviceInfo {
            id: source.id,
            name: source.name,
            description: source.description,
        })
        .collect())
}

fn enumerate_cpal_audio_input_devices() -> anyhow::Result<Vec<AudioDeviceInfo>> {
    let host = cpal::default_host();
    let mut devices = Vec::new();
    let mut filtered = Vec::new();

    for device in host
        .input_devices()
        .context("failed to enumerate input devices")?
    {
        let description = match device.description() {
            Ok(description) => description,
            Err(error) => {
                warn!(%error, "Skipping audio device with unreadable description");
                continue;
            }
        };

        let name = description.name().trim().to_owned();
        if name.is_empty() {
            continue;
        }

        if !should_offer_named_audio_device(&name) {
            filtered.push(name);
            continue;
        }

        devices.push(AudioDeviceInfo {
            id: name.clone(),
            name: name.clone(),
            description: name,
        });
    }

    if !filtered.is_empty() {
        debug!(
            filtered = ?filtered,
            "Filtered unsupported or synthetic audio devices from the input list"
        );
    }
    debug!(
        count = devices.len(),
        "Enumerated named audio capture devices"
    );

    Ok(devices)
}

fn default_audio_device() -> AudioDeviceInfo {
    AudioDeviceInfo {
        id: "default".to_owned(),
        name: "System Monitor".to_owned(),
        description: "Prefer the active system output monitor source".to_owned(),
    }
}

fn microphone_audio_device() -> AudioDeviceInfo {
    AudioDeviceInfo {
        id: "microphone".to_owned(),
        name: "Default Microphone".to_owned(),
        description: "Capture from the default input device".to_owned(),
    }
}

fn disabled_audio_device() -> AudioDeviceInfo {
    AudioDeviceInfo {
        id: "none".to_owned(),
        name: "Disabled".to_owned(),
        description: "Send silence to audio-reactive effects".to_owned(),
    }
}

fn should_include_current_device(current: &str, devices: &[AudioDeviceInfo]) -> bool {
    !current.trim().is_empty()
        && !devices
            .iter()
            .any(|device| device.id.eq_ignore_ascii_case(current))
}

fn dedupe_audio_devices(devices: &mut Vec<AudioDeviceInfo>) {
    let mut seen = HashSet::new();
    devices.retain(|device| seen.insert(device.id.to_ascii_lowercase()));
}

#[doc(hidden)]
pub fn should_offer_named_audio_device(name: &str) -> bool {
    let normalized = name.trim();
    !normalized.is_empty()
        && !is_monitorish_device_name(normalized)
        && !is_serverish_device_name(normalized)
}

fn is_serverish_device_name(name: &str) -> bool {
    let normalized = name.to_ascii_lowercase();
    [
        "sound server",
        "pipewire",
        "pulseaudio",
        "default alsa output",
        "default output",
        "discard all samples",
        "rate converter plugin",
        "plugin for channel",
        "plugin using speex",
    ]
    .iter()
    .any(|needle| normalized.contains(needle))
}

fn is_monitorish_device_name(name: &str) -> bool {
    let normalized = name.to_ascii_lowercase();
    ["monitor", "loopback", "what u hear", "stereo mix"]
        .iter()
        .any(|needle| normalized.contains(needle))
}

#[cfg(test)]
mod tests {
    use super::{
        get_sensor, get_sensors, get_server, get_status, get_system, input_source_status,
        macos_daemon_ownership, us_to_ms_f64,
    };
    use crate::api::AppState;
    use crate::api::security::RequestAuthContext;
    use crate::macos_owner::{
        MacosDaemonOwner, MacosDaemonSessionAttestation, MacosHandoverPhase, MacosOwnerConflict,
        MacosOwnerIdentity, MacosOwnerRecoveryRequired, MacosOwnerSnapshot,
        MacosProtectedControlCredential, MacosServerSessionId,
    };
    use crate::performance::{
        CompositorBackendKind, FrameTimeline, FullFrameCopyMetrics, LatestFrameMetrics,
        OutputFrameSourceKind,
    };
    use crate::preview_runtime::{PreviewPixelFormat, PreviewStreamDemand};
    use axum::body::to_bytes;
    use axum::extract::{Extension, Path, State};
    use hypercolor_core::bus::CanvasFrame;
    use hypercolor_core::input::screen::ScreenAdmissionCapacity;
    use hypercolor_core::input::{
        DataSource, DataSourceKind, DataSourceRole, InputData, InputSource, ManagedSourceRole,
        SourceFreshness, SourceKind, SourceRoleBinding, SourceState, SourceStatus,
        SourceStatusHandle, SourceStatusReporter,
    };
    use hypercolor_types::canvas::Canvas;
    use hypercolor_types::sensor::{SensorReading, SensorUnit, SystemSnapshot};
    use hypercolor_types::source_status::{
        SourceDiagnosticsDisplayField, SourceDiagnosticsEnvelope,
    };
    use serde_json::{Value, json};
    use std::sync::Arc;
    use std::time::Instant;

    struct FixedSensorSource {
        snapshot: Arc<SystemSnapshot>,
        running: bool,
    }

    struct FixedDiagnosticsSource {
        status: SourceStatusReporter,
    }

    impl InputSource for FixedDiagnosticsSource {
        fn name(&self) -> &'static str {
            "fixed-diagnostics"
        }

        fn source_status_handle(&self) -> Option<SourceStatusHandle> {
            Some(self.status.handle())
        }

        fn source_status_reporter(&mut self) -> Option<&mut SourceStatusReporter> {
            Some(&mut self.status)
        }

        fn start(&mut self) -> anyhow::Result<()> {
            Ok(())
        }

        fn stop(&mut self) {}

        fn sample(&mut self) -> anyhow::Result<InputData> {
            Ok(InputData::None)
        }

        fn is_running(&self) -> bool {
            false
        }
    }

    impl SourceRoleBinding for FixedDiagnosticsSource {
        type Role = DataSourceRole;
    }

    impl DataSource for FixedDiagnosticsSource {
        fn data_source_kind(&self) -> DataSourceKind {
            DataSourceKind::Sensors
        }
    }

    impl InputSource for FixedSensorSource {
        fn name(&self) -> &'static str {
            "fixed-sensors"
        }

        fn start(&mut self) -> anyhow::Result<()> {
            self.running = true;
            Ok(())
        }

        fn stop(&mut self) {
            self.running = false;
        }

        fn sample(&mut self) -> anyhow::Result<InputData> {
            Ok(if self.running {
                InputData::Sensors(Arc::clone(&self.snapshot))
            } else {
                InputData::None
            })
        }

        fn is_running(&self) -> bool {
            self.running
        }
    }

    impl SourceRoleBinding for FixedSensorSource {
        type Role = DataSourceRole;
    }

    impl DataSource for FixedSensorSource {
        fn data_source_kind(&self) -> DataSourceKind {
            DataSourceKind::Sensors
        }
    }

    async fn install_sensor_snapshot(state: &AppState, snapshot: Arc<SystemSnapshot>) {
        state
            .input_manager
            .add_source(ManagedSourceRole::data(Box::new(FixedSensorSource {
                snapshot,
                running: false,
            })))
            .expect("fixed sensor source should register");
        state
            .input_manager
            .start_all()
            .expect("fixed sensor source starts");
        state.input_manager.sample_sources(0.0);
    }

    #[tokio::test]
    async fn server_response_exposes_only_the_attested_session_id() {
        let tempdir = tempfile::tempdir().expect("server test data dir should be created");
        let session_id = MacosServerSessionId::from_bytes([0x33; 16]);
        let credential = MacosProtectedControlCredential::from_bytes([0x77; 32]);
        let attestation = MacosDaemonSessionAttestation {
            schema_version: crate::macos_owner::MACOS_DAEMON_SESSION_ATTESTATION_SCHEMA_VERSION,
            owner: MacosDaemonOwner::AppSidecar,
            owner_epoch: 7,
            owner_identity: MacosOwnerIdentity::new(
                "audit-server",
                tempdir.path().join("hypercolor-daemon"),
                "requirement-server",
                4242,
            )
            .expect("fixture identity should be valid"),
            server_session_id: session_id.clone(),
            protected_control_credential: credential.clone(),
        };
        let mut state = AppState::new_with_data_dir(tempdir.path().join("data"));
        state.install_macos_daemon_session(&attestation);

        let response = get_server(State(Arc::new(state))).await;
        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("server response should read");
        let value: Value = serde_json::from_slice(&bytes).expect("server response should be JSON");

        assert_eq!(value["data"]["server_session_id"], session_id.as_str());
        assert!(!String::from_utf8_lossy(&bytes).contains(credential.expose_secret()));
    }

    fn source_status_fixture(diagnostics: Option<SourceDiagnosticsEnvelope>) -> SourceStatus {
        SourceStatus {
            source_id: Arc::from("fixture:source"),
            kind: SourceKind::Interaction,
            backend: Arc::from("fixture"),
            configured: true,
            consented: true,
            demanded: true,
            active_consumer_count: 2,
            state: SourceState::Live,
            freshness: SourceFreshness::NotApplicable,
            source_graph_generation: 7,
            session_generation: 11,
            last_sample_at: None,
            freshness_deadline: None,
            resource_count: 2,
            denied_resource_count: 0,
            issue: None,
            freshness_issue: None,
            action_issue: None,
            diagnostics: diagnostics.map(Arc::new),
            retired: false,
        }
    }

    #[test]
    fn input_source_status_relays_opaque_diagnostics() {
        let diagnostics = SourceDiagnosticsEnvelope::try_new(
            "fixture.backend",
            17,
            vec![SourceDiagnosticsDisplayField::new(
                "mode",
                "Mode",
                "diagnostic",
            )],
            json!({"future_probe": {"available": true}}),
        )
        .expect("fixture diagnostics should be bounded");
        let status = input_source_status(
            &source_status_fixture(Some(diagnostics.clone())),
            Instant::now(),
            true,
        );
        let value = serde_json::to_value(status).expect("input status should serialize");

        assert_eq!(value["diagnostics"]["schema"], "fixture.backend");
        assert_eq!(value["diagnostics"]["version"], 17);
        assert_eq!(value["diagnostics"]["display"][0]["label"], "Mode");
        assert_eq!(
            value["diagnostics"]["payload"],
            diagnostics.payload().clone()
        );
    }

    #[test]
    fn input_source_status_exposes_wayland_drop_reason_counters() {
        let diagnostics = SourceDiagnosticsEnvelope::try_new(
            "wayland.pipewire.capture",
            1,
            Vec::new(),
            json!({
                "copied_frames": 8,
                "dropped_frames": 3,
                "copied_bytes": 4096,
                "drop_reasons": {
                    "invalid_crop": 2,
                    "invalid_transform": 1,
                },
            }),
        )
        .expect("Wayland callback diagnostics should be bounded");
        let status = input_source_status(
            &source_status_fixture(Some(diagnostics)),
            Instant::now(),
            true,
        );
        let value = serde_json::to_value(status).expect("input status should serialize");

        assert_eq!(
            value["diagnostics"]["payload"]["drop_reasons"]["invalid_crop"],
            2
        );
        assert_eq!(
            value["diagnostics"]["payload"]["drop_reasons"]["invalid_transform"],
            1
        );
    }

    #[tokio::test]
    async fn system_resource_redacts_protected_diagnostics_without_control() {
        let secret = "display:com.secret.private";
        let diagnostics = SourceDiagnosticsEnvelope::try_new_with_public_payload(
            "fixture.protected",
            1,
            Vec::new(),
            json!({"selection": {"source_id": secret}}),
            json!({"selection": {"source_id": "display"}}),
        )
        .expect("protected diagnostics should remain bounded");
        let mut reporter = SourceStatusReporter::new(
            "fixture:protected",
            SourceKind::Sensors,
            "fixture",
            true,
            true,
            true,
        );
        reporter
            .set_diagnostics(Some(diagnostics))
            .expect("fixture diagnostics should publish");
        let state = AppState::new();
        state
            .input_manager
            .add_source(ManagedSourceRole::data(Box::new(FixedDiagnosticsSource {
                status: reporter,
            })))
            .expect("fixture source should register");
        let state = Arc::new(state);

        let anonymous = get_system(
            State(Arc::clone(&state)),
            Extension(RequestAuthContext::preflight()),
        )
        .await;
        let read = get_system(
            State(Arc::clone(&state)),
            Extension(RequestAuthContext::read_only()),
        )
        .await;
        let legacy = get_status(State(Arc::clone(&state))).await;
        let control = get_system(State(state), Extension(RequestAuthContext::control())).await;
        let anonymous = to_bytes(anonymous.into_body(), usize::MAX)
            .await
            .expect("anonymous response should read");
        let read = to_bytes(read.into_body(), usize::MAX)
            .await
            .expect("read response should read");
        let control = to_bytes(control.into_body(), usize::MAX)
            .await
            .expect("control response should read");
        let legacy = to_bytes(legacy.into_body(), usize::MAX)
            .await
            .expect("legacy response should read");
        let anonymous: Value =
            serde_json::from_slice(&anonymous).expect("anonymous response should parse");
        let read: Value = serde_json::from_slice(&read).expect("read response should parse");
        let control: Value =
            serde_json::from_slice(&control).expect("control response should parse");
        let legacy: Value = serde_json::from_slice(&legacy).expect("legacy response should parse");

        assert!(anonymous["data"].get("status").is_none());
        assert!(!read.to_string().contains(secret));
        assert!(read.to_string().contains("display"));
        assert!(!legacy.to_string().contains(secret));
        assert!(control.to_string().contains(secret));
    }

    #[test]
    fn system_status_serializes_authoritative_macos_daemon_ownership() {
        let value = serde_json::to_value(macos_daemon_ownership(&MacosOwnerSnapshot {
            active_owner: MacosDaemonOwner::DirectLaunchd,
            owner_epoch: 42,
            conflict: Some(MacosOwnerConflict {
                active_owner: MacosDaemonOwner::DirectLaunchd,
                active_epoch: 42,
                contender_owner: MacosDaemonOwner::Homebrew,
                observed_at_ms: 1_725_000_000_789,
            }),
            recovery_required: Some(MacosOwnerRecoveryRequired {
                requested_owner: MacosDaemonOwner::AppSidecar,
                prior_owner: MacosDaemonOwner::Homebrew,
                phase: MacosHandoverPhase::RollbackStopRequested,
            }),
        }))
        .expect("macOS daemon ownership should serialize");

        assert_eq!(
            value,
            json!({
                "active_owner": "launchd_service",
                "owner_epoch": 42,
                "conflict": {
                    "active": "launchd_service",
                    "contender": "homebrew_service",
                    "observed_at_ms": 1_725_000_000_789_u64
                },
                "recovery_required": {
                    "requested_owner": "app_sidecar",
                    "prior_owner": "homebrew_service",
                    "phase": "rollback_stop_requested"
                }
            })
        );
    }

    #[test]
    fn input_source_status_omits_absent_diagnostics() {
        let status = input_source_status(&source_status_fixture(None), Instant::now(), false);
        let value = serde_json::to_value(status).expect("source status should serialize");

        assert!(value.get("diagnostics").is_none());
    }

    #[test]
    fn source_diagnostics_are_present_in_openapi_without_platform_variants() {
        use utoipa::OpenApi;

        let document = crate::api::openapi::ApiDoc::openapi();
        let value = serde_json::to_value(document).expect("OpenAPI should serialize");
        let schemas = value["components"]["schemas"]
            .as_object()
            .expect("OpenAPI should contain component schemas");

        assert!(schemas.contains_key("SourceDiagnosticsEnvelope"));
        assert!(schemas.contains_key("SourceDiagnosticsDisplayField"));
        assert!(schemas.contains_key("MacosDaemonOwnershipApiStatus"));
        assert!(schemas.contains_key("MacosDaemonOwnerConflictApiStatus"));
        assert!(schemas.contains_key("MacosDaemonOwnerRecoveryRequiredApiStatus"));
        assert!(schemas.contains_key("MacosDaemonHandoverPhaseApi"));
        assert!(!schemas.contains_key("InputSourcePlatformStatus"));
        assert!(!schemas.contains_key("MacosScreenTelemetryApiStatus"));
    }

    #[expect(
        clippy::too_many_lines,
        reason = "Status response assertions cover many nested metrics fields in one scenario"
    )]
    #[tokio::test]
    async fn status_includes_latest_frame_surface_stats() {
        let tempdir = tempfile::tempdir().expect("status test data dir should be created");
        let state = Arc::new(AppState::new_with_data_dir(tempdir.path().join("data")));
        state.render_loop.write().await.start();
        let mut preview_rx = state.preview_runtime.canvas_receiver();
        let mut scene_preview_rx = state.preview_runtime.scene_canvas_receiver();
        let mut screen_preview_rx = state.preview_runtime.screen_canvas_receiver();
        preview_rx.update_demand(PreviewStreamDemand {
            fps: 24,
            format: PreviewPixelFormat::Jpeg,
            width: 640,
            height: 360,
        });
        scene_preview_rx.update_demand(PreviewStreamDemand {
            fps: 12,
            format: PreviewPixelFormat::Rgb,
            width: 320,
            height: 180,
        });
        screen_preview_rx.update_demand(PreviewStreamDemand {
            fps: 30,
            format: PreviewPixelFormat::Rgba,
            width: 0,
            height: 0,
        });
        let canvas_frame = CanvasFrame::from_canvas(&Canvas::new(2, 1), 88, 44);
        let scene_frame = CanvasFrame::from_canvas(&Canvas::new(2, 1), 66, 33);
        let screen_frame = CanvasFrame::from_canvas(&Canvas::new(1, 1), 45, 21);
        let _ = state.event_bus.canvas_sender().send(canvas_frame.clone());
        let _ = state
            .event_bus
            .scene_canvas_sender()
            .send(scene_frame.clone());
        let _ = state
            .event_bus
            .screen_canvas_sender()
            .send(screen_frame.clone());
        state
            .preview_runtime
            .record_canvas_publication(canvas_frame.frame_number, canvas_frame.timestamp_ms);
        state
            .preview_runtime
            .record_scene_canvas_publication(scene_frame.frame_number, scene_frame.timestamp_ms);
        state
            .preview_runtime
            .record_screen_canvas_publication(screen_frame.frame_number, screen_frame.timestamp_ms);
        {
            let mut performance = state.performance.write().await;
            performance.record_effect_error();
            performance.record_effect_fallback_applied();
            let frame = LatestFrameMetrics {
                timestamp_ms: 40,
                input_sampled: true,
                input_us: 100,
                deferred_sample_us: 40,
                producer_us: 500,
                producer_render_us: 320,
                producer_scene_compose_us: 60,
                composition_us: 200,
                render_us: 700,
                preview_advance_us: 25,
                sample_us: 150,
                sample_dispatch_us: 90,
                push_us: 250,
                postprocess_us: 0,
                publish_us: 120,
                publish_frame_data_us: 30,
                publish_group_canvas_us: 20,
                publish_preview_us: 60,
                publish_events_us: 10,
                overhead_us: 50,
                total_us: 1_270,
                wake_late_us: 90,
                jitter_us: 30,
                reused_inputs: false,
                reused_canvas: false,
                retained_effect: false,
                retained_screen: false,
                composition_bypassed: false,
                gpu_zone_sampling: true,
                gpu_sample_deferred: true,
                gpu_sample_stale: true,
                gpu_sample_retry_hit: true,
                gpu_sample_queue_saturated: true,
                gpu_sample_wait_blocked: true,
                gpu_sample_cpu_fallback: true,
                preview_surface: true,
                scene_canvas_forced_surface: true,
                cpu_readback_skipped: true,
                gpu_readback_failed: true,
                compositor_backend: CompositorBackendKind::GpuFallback,
                output_frame_source: OutputFrameSourceKind::RoutedReuse,
                output_reuses_published_frame: true,
                output_brightness_bits: 1.0_f32.to_bits(),
                output_brightness_generation: 5,
                output_routing_signature: 7,
                output_zone_shape_signature: 11,
                output_unassigned_behavior_generation: 13,
                devices_written: 3,
                total_leds: 144,
                logical_layer_count: 2,
                render_group_count: 1,
                scene_active: true,
                scene_transition_active: false,
                render_surface_slot_count: 6,
                render_surface_free_slots: 1,
                render_surface_published_slots: 4,
                render_surface_dequeued_slots: 1,
                scene_pool_saturation_reallocs: 0,
                direct_pool_saturation_reallocs: 0,
                scene_pool_grown_slots: 0,
                direct_pool_grown_slots: 0,
                scene_pool_slot_count: 0,
                scene_pool_max_slots: 0,
                direct_pool_slot_count: 0,
                direct_pool_max_slots: 0,
                scene_pool_shared_published_slots: 0,
                scene_pool_max_ref_count: 0,
                direct_pool_shared_published_slots: 0,
                direct_pool_max_ref_count: 0,
                scene_pool_free_slots: 0,
                scene_pool_published_slots: 0,
                scene_pool_dequeued_slots: 0,
                direct_pool_free_slots: 0,
                direct_pool_published_slots: 0,
                direct_pool_dequeued_slots: 0,
                preview_pool_slot_count: 0,
                preview_pool_free_slots: 0,
                preview_pool_published_slots: 0,
                preview_pool_dequeued_slots: 0,
                compositor_pool_slot_count: 0,
                compositor_pool_free_slots: 0,
                compositor_pool_published_slots: 0,
                compositor_pool_dequeued_slots: 0,
                canvas_receiver_count: 2,
                producer_full_frame_copy: FullFrameCopyMetrics {
                    count: 1,
                    bytes: 128_000,
                    reason: Some("producer_test"),
                },
                publication_full_frame_copy: FullFrameCopyMetrics {
                    count: 1,
                    bytes: 128_000,
                    reason: Some("publication_test"),
                },
                full_frame_copy_count: 2,
                full_frame_copy_bytes: 256_000,
                output_errors: 0,
                timeline: FrameTimeline {
                    frame_token: 77,
                    budget_us: 16_666,
                    scene_snapshot_done_us: 80,
                    input_done_us: 180,
                    producer_done_us: 680,
                    composition_done_us: 880,
                    sample_done_us: 1_030,
                    output_done_us: 1_280,
                    publish_done_us: 1_400,
                    frame_done_us: 1_450,
                },
            };
            performance.record_frame(&frame);
            drop(performance);
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            state.performance.write().await.record_frame(&frame);
        }
        state
            .input_manager
            .set_screen_capacity_plan(
                ScreenAdmissionCapacity::new(2_000_000, 2_000_000),
                ScreenAdmissionCapacity::new(123, 456),
                ScreenAdmissionCapacity::new(123, 456),
            )
            .expect("empty manager should accept test capacity");

        let response = get_status(State(state)).await;
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("status body should read");
        let json: Value = serde_json::from_slice(&body).expect("status should serialize");
        let servo_health = super::servo_effect_health_counts();

        assert_eq!(json["data"]["render_loop"]["target_fps"], 60);
        assert_eq!(json["data"]["render_loop"]["ceiling_fps"], 60);
        assert_eq!(json["data"]["render_loop"]["capacity_fps"], 60.0);
        let delivered_fps = json["data"]["render_loop"]["delivered_fps"]
            .as_f64()
            .expect("delivered_fps should be numeric");
        assert!(delivered_fps > 0.0);
        assert!(delivered_fps < 60.0);
        assert_eq!(json["data"]["render_loop"]["actual_fps"], 60.0);
        assert_eq!(
            json["data"]["session_performance"]["input_stage"]["sample_count"],
            2
        );
        assert_eq!(
            json["data"]["session_performance"]["input_stage"]["p95_ms"],
            0.1
        );
        assert_eq!(
            json["data"]["session_performance"]["input_stage"]["p99_ms"],
            0.1
        );
        assert_eq!(
            json["data"]["session_performance"]["input_stage"]["cumulative_histogram"]["bucket_width_us"],
            100
        );
        assert_eq!(
            json["data"]["session_performance"]["input_stage"]["cumulative_histogram"]["overflow_bucket_index"],
            4096
        );
        assert_eq!(
            json["data"]["session_performance"]["input_stage"]["cumulative_histogram"]["snapshot_frame_token"],
            77
        );
        assert_eq!(
            json["data"]["session_performance"]["input_stage"]["cumulative_histogram"]["buckets"],
            serde_json::json!([{ "bucket_index": 1, "count": 2 }])
        );
        assert_eq!(
            json["data"]["session_performance"]["full_frame_cpu_copies"]["count"],
            4
        );
        assert_eq!(
            json["data"]["session_performance"]["full_frame_cpu_copies"]["frames"],
            2
        );
        assert_eq!(
            json["data"]["session_performance"]["full_frame_cpu_copies"]["bytes"],
            512_000
        );
        assert_eq!(
            json["data"]["compositor_acceleration"]["requested_mode"],
            "cpu"
        );
        assert_eq!(
            json["data"]["compositor_acceleration"]["effective_mode"],
            "cpu"
        );
        assert!(json["data"]["compositor_acceleration"]["fallback_reason"].is_null());
        assert!(json["data"]["compositor_acceleration"]["gpu_probe"].is_null());
        assert_eq!(
            json["data"]["screen_capture_capacity"]["admission_enforced"],
            true
        );
        assert_eq!(
            json["data"]["screen_capture_capacity"]["physical_transition_byte_capacity"],
            2_000_000
        );
        assert_eq!(
            json["data"]["screen_capture_capacity"]["physical_transition_backend_capacity"],
            2_000_000
        );
        assert_eq!(
            json["data"]["screen_capture_capacity"]["physical_reserved_bytes"],
            0
        );
        assert_eq!(
            json["data"]["screen_capture_capacity"]["physical_available_bytes"],
            2_000_000
        );
        assert_eq!(
            json["data"]["screen_capture_capacity"]["steady_total_byte_budget"],
            123
        );
        assert_eq!(
            json["data"]["screen_capture_capacity"]["steady_publication_byte_budget"],
            123
        );
        assert!(json["data"]["screen_capture_capacity"]["analysis_retained_bytes"].is_null());
        assert_eq!(json["data"]["latest_frame"]["frame_token"], 77);
        assert_eq!(
            json["data"]["latest_frame"]["compositor_backend"],
            "gpu_fallback"
        );
        assert_eq!(
            json["data"]["latest_frame"]["output_frame_source"],
            "routed_reuse"
        );
        assert_eq!(
            json["data"]["latest_frame"]["output_reuses_published_frame"],
            true
        );
        assert_eq!(
            json["data"]["latest_frame"]["output_brightness_generation"],
            5
        );
        assert_eq!(json["data"]["latest_frame"]["output_routing_signature"], 7);
        assert_eq!(
            json["data"]["latest_frame"]["output_zone_shape_signature"],
            11
        );
        assert_eq!(
            json["data"]["latest_frame"]["output_unassigned_behavior_generation"],
            13
        );
        assert_eq!(json["data"]["latest_frame"]["devices_written"], 3);
        assert_eq!(json["data"]["latest_frame"]["total_leds"], 144);
        assert_eq!(json["data"]["latest_frame"]["gpu_zone_sampling"], true);
        assert_eq!(json["data"]["latest_frame"]["gpu_sample_deferred"], true);
        assert_eq!(json["data"]["latest_frame"]["gpu_sample_stale"], true);
        assert_eq!(json["data"]["latest_frame"]["gpu_sample_retry_hit"], true);
        assert_eq!(
            json["data"]["latest_frame"]["gpu_sample_queue_saturated"],
            true
        );
        assert_eq!(
            json["data"]["latest_frame"]["gpu_sample_wait_blocked"],
            true
        );
        assert_eq!(
            json["data"]["latest_frame"]["gpu_sample_cpu_fallback"],
            true
        );
        assert_eq!(
            json["data"]["latest_frame"]["cpu_sampling_late_readback"],
            false
        );
        assert_eq!(json["data"]["latest_frame"]["led_sampling_readback"], false);
        assert_eq!(json["data"]["latest_frame"]["preview_surface"], true);
        assert_eq!(
            json["data"]["latest_frame"]["scene_canvas_forced_surface"],
            true
        );
        assert_eq!(json["data"]["latest_frame"]["jitter_ms"], 0.03);
        assert_eq!(json["data"]["latest_frame"]["input_sampling_ms"], 0.1);
        assert_eq!(json["data"]["latest_frame"]["producer_ms"], 0.5);
        assert_eq!(json["data"]["latest_frame"]["producer_render_ms"], 0.32);
        assert_eq!(
            json["data"]["latest_frame"]["producer_preview_compose_ms"],
            0.06
        );
        assert_eq!(json["data"]["latest_frame"]["composition_ms"], 0.2);
        assert_eq!(json["data"]["latest_frame"]["effect_rendering_ms"], 0.7);
        assert_eq!(json["data"]["latest_frame"]["spatial_sampling_ms"], 0.15);
        assert_eq!(json["data"]["latest_frame"]["device_output_ms"], 0.25);
        assert_eq!(json["data"]["latest_frame"]["preview_postprocess_ms"], 0.0);
        assert_eq!(json["data"]["latest_frame"]["event_bus_ms"], 0.12);
        assert_eq!(
            json["data"]["latest_frame"]["coordination_overhead_ms"],
            0.05
        );
        assert_eq!(json["data"]["latest_frame"]["publish_frame_data_ms"], 0.03);
        assert_eq!(
            json["data"]["latest_frame"]["publish_group_canvas_ms"],
            0.02
        );
        assert_eq!(json["data"]["latest_frame"]["publish_preview_ms"], 0.06);
        assert_eq!(json["data"]["latest_frame"]["publish_events_ms"], 0.01);
        assert_eq!(json["data"]["latest_frame"]["cpu_readback_skipped"], true);
        assert_eq!(json["data"]["latest_frame"]["gpu_readback_failed"], true);
        assert_eq!(
            json["data"]["latest_frame"]["render_surfaces"]["slot_count"],
            6
        );
        assert_eq!(
            json["data"]["latest_frame"]["render_surfaces"]["scene_pool_slot_count"],
            0
        );
        assert_eq!(
            json["data"]["latest_frame"]["render_surfaces"]["preview_pool_slot_count"],
            0
        );
        assert_eq!(
            json["data"]["latest_frame"]["render_surfaces"]["compositor_pool_slot_count"],
            0
        );
        assert_eq!(
            json["data"]["latest_frame"]["render_surfaces"]["canvas_receivers"],
            2
        );
        assert_eq!(json["data"]["latest_frame"]["full_frame_copy_count"], 2);
        assert_eq!(json["data"]["latest_frame"]["full_frame_copy_kb"], 250.0);
        assert_eq!(
            json["data"]["latest_frame"]["producer_full_frame_copy_count"],
            1
        );
        assert_eq!(
            json["data"]["latest_frame"]["producer_full_frame_copy_kb"],
            125.0
        );
        assert_eq!(
            json["data"]["latest_frame"]["producer_full_frame_copy_reason"],
            "producer_test"
        );
        assert_eq!(
            json["data"]["latest_frame"]["publication_full_frame_copy_count"],
            1
        );
        assert_eq!(
            json["data"]["latest_frame"]["publication_full_frame_copy_kb"],
            125.0
        );
        assert_eq!(
            json["data"]["latest_frame"]["publication_full_frame_copy_reason"],
            "publication_test"
        );
        assert_eq!(json["data"]["latest_frame"]["output_errors"], 0);
        assert_eq!(json["data"]["effect_health"]["errors_total"], 1);
        assert_eq!(json["data"]["effect_health"]["fallbacks_applied_total"], 1);
        assert_eq!(
            json["data"]["effect_health"]["producer_gpu_readback_failures_total"],
            2
        );
        assert_eq!(
            json["data"]["effect_health"]["servo_soft_stalls_total"],
            servo_health.soft_stalls_total
        );
        assert_eq!(
            json["data"]["effect_health"]["servo_breaker_opens_total"],
            servo_health.breaker_opens_total
        );
        assert_eq!(
            json["data"]["effect_health"]["servo_session_creates_total"],
            servo_health.session_creates_total
        );
        assert_eq!(
            json["data"]["effect_health"]["servo_session_create_failures_total"],
            servo_health.session_create_failures_total
        );
        assert_eq!(
            json["data"]["effect_health"]["servo_page_loads_total"],
            servo_health.page_loads_total
        );
        assert_eq!(
            json["data"]["effect_health"]["servo_page_load_failures_total"],
            servo_health.page_load_failures_total
        );
        assert_eq!(
            json["data"]["effect_health"]["servo_detached_destroys_total"],
            servo_health.detached_destroys_total
        );
        assert_eq!(
            json["data"]["effect_health"]["servo_detached_destroy_failures_total"],
            servo_health.detached_destroy_failures_total
        );
        assert_eq!(
            json["data"]["effect_health"]["servo_render_requests_total"],
            servo_health.render_requests_total
        );
        assert_eq!(
            json["data"]["effect_health"]["servo_render_queue_wait_total_ms"],
            us_to_ms_f64(servo_health.render_queue_wait_total_us)
        );
        assert_eq!(
            json["data"]["effect_health"]["servo_render_queue_wait_max_ms"],
            us_to_ms_f64(servo_health.render_queue_wait_max_us)
        );
        assert_eq!(
            json["data"]["effect_health"]["servo_render_scene_requests_total"],
            servo_health.render_scene_requests_total
        );
        assert_eq!(
            json["data"]["effect_health"]["servo_render_scene_queue_wait_total_ms"],
            us_to_ms_f64(servo_health.render_scene_queue_wait_total_us)
        );
        assert_eq!(
            json["data"]["effect_health"]["servo_render_scene_queue_wait_max_ms"],
            us_to_ms_f64(servo_health.render_scene_queue_wait_max_us)
        );
        assert_eq!(
            json["data"]["effect_health"]["servo_render_display_requests_total"],
            servo_health.render_display_requests_total
        );
        assert_eq!(
            json["data"]["effect_health"]["servo_render_display_queue_wait_total_ms"],
            us_to_ms_f64(servo_health.render_display_queue_wait_total_us)
        );
        assert_eq!(
            json["data"]["effect_health"]["servo_render_display_queue_wait_max_ms"],
            us_to_ms_f64(servo_health.render_display_queue_wait_max_us)
        );
        assert_eq!(
            json["data"]["effect_health"]["servo_render_cpu_frames_total"],
            servo_health.render_cpu_frames_total
        );
        assert_eq!(
            json["data"]["effect_health"]["servo_render_cached_frames_total"],
            servo_health.render_cached_frames_total
        );
        assert_eq!(
            json["data"]["effect_health"]["servo_gpu_import_slot_count"],
            servo_health.render_gpu_import_slot_count
        );
        assert_eq!(
            json["data"]["effect_health"]["servo_gpu_import_pending_slots"],
            servo_health.render_gpu_import_pending_slots
        );
        assert_eq!(
            json["data"]["effect_health"]["servo_gpu_import_available_slots"],
            servo_health.render_gpu_import_available_slots
        );
        assert!(
            json["data"]["effect_health"]["sparkleflinger_display_finalize_blocking_wait_total_ms"]
                .is_number()
        );
        assert!(
            json["data"]["effect_health"]["sparkleflinger_media_texture_allocations_total"]
                .is_number()
        );
        assert!(
            json["data"]["effect_health"]["producer_gpu_cpu_materialization_blocked_total"]
                .is_number()
        );
        assert_eq!(
            json["data"]["effect_health"]["servo_render_evaluate_scripts_total_ms"],
            us_to_ms_f64(servo_health.render_evaluate_scripts_total_us)
        );
        assert_eq!(
            json["data"]["effect_health"]["servo_render_evaluate_scripts_max_ms"],
            us_to_ms_f64(servo_health.render_evaluate_scripts_max_us)
        );
        assert_eq!(
            json["data"]["effect_health"]["servo_render_event_loop_total_ms"],
            us_to_ms_f64(servo_health.render_event_loop_total_us)
        );
        assert_eq!(
            json["data"]["effect_health"]["servo_render_event_loop_max_ms"],
            us_to_ms_f64(servo_health.render_event_loop_max_us)
        );
        assert_eq!(
            json["data"]["effect_health"]["servo_render_paint_total_ms"],
            us_to_ms_f64(servo_health.render_paint_total_us)
        );
        assert_eq!(
            json["data"]["effect_health"]["servo_render_paint_max_ms"],
            us_to_ms_f64(servo_health.render_paint_max_us)
        );
        assert_eq!(
            json["data"]["effect_health"]["servo_render_readback_total_ms"],
            us_to_ms_f64(servo_health.render_readback_total_us)
        );
        assert_eq!(
            json["data"]["effect_health"]["servo_render_readback_max_ms"],
            us_to_ms_f64(servo_health.render_readback_max_us)
        );
        assert_eq!(
            json["data"]["effect_health"]["servo_render_frame_total_ms"],
            us_to_ms_f64(servo_health.render_frame_total_us)
        );
        assert_eq!(
            json["data"]["effect_health"]["servo_render_frame_max_ms"],
            us_to_ms_f64(servo_health.render_frame_max_us)
        );
        assert_eq!(json["data"]["preview_runtime"]["canvas_receivers"], 1);
        assert_eq!(json["data"]["preview_runtime"]["scene_canvas_receivers"], 1);
        assert_eq!(
            json["data"]["preview_runtime"]["screen_canvas_receivers"],
            1
        );
        assert_eq!(
            json["data"]["preview_runtime"]["canvas_frames_published"],
            1
        );
        assert_eq!(
            json["data"]["preview_runtime"]["scene_canvas_frames_published"],
            1
        );
        assert_eq!(
            json["data"]["preview_runtime"]["screen_canvas_frames_published"],
            1
        );
        assert_eq!(
            json["data"]["preview_runtime"]["latest_canvas_frame_number"],
            88
        );
        assert_eq!(
            json["data"]["preview_runtime"]["latest_scene_canvas_frame_number"],
            66
        );
        assert_eq!(
            json["data"]["preview_runtime"]["latest_screen_canvas_frame_number"],
            45
        );
        assert_eq!(
            json["data"]["preview_runtime"]["canvas_demand"]["subscribers"],
            1
        );
        assert_eq!(
            json["data"]["preview_runtime"]["canvas_demand"]["max_fps"],
            24
        );
        assert_eq!(
            json["data"]["preview_runtime"]["canvas_demand"]["max_width"],
            640
        );
        assert_eq!(
            json["data"]["preview_runtime"]["canvas_demand"]["max_height"],
            360
        );
        assert_eq!(
            json["data"]["preview_runtime"]["canvas_demand"]["any_jpeg"],
            true
        );
        assert_eq!(
            json["data"]["preview_runtime"]["scene_canvas_demand"]["subscribers"],
            1
        );
        assert_eq!(
            json["data"]["preview_runtime"]["scene_canvas_demand"]["max_fps"],
            12
        );
        assert_eq!(
            json["data"]["preview_runtime"]["scene_canvas_demand"]["max_width"],
            320
        );
        assert_eq!(
            json["data"]["preview_runtime"]["scene_canvas_demand"]["max_height"],
            180
        );
        assert_eq!(
            json["data"]["preview_runtime"]["scene_canvas_demand"]["any_rgb"],
            true
        );
        assert_eq!(
            json["data"]["preview_runtime"]["screen_canvas_demand"]["subscribers"],
            1
        );
        assert_eq!(
            json["data"]["preview_runtime"]["screen_canvas_demand"]["any_full_resolution"],
            true
        );
        assert_eq!(
            json["data"]["preview_runtime"]["screen_canvas_demand"]["any_rgba"],
            true
        );
    }

    #[tokio::test]
    async fn sensors_endpoint_returns_latest_snapshot() {
        let state = Arc::new(AppState::new());
        let snapshot = Arc::new(SystemSnapshot {
            cpu_load_percent: 51.0,
            cpu_loads: vec![48.0, 54.0],
            cpu_temp_celsius: Some(72.5),
            gpu_temp_celsius: None,
            gpu_load_percent: None,
            gpu_vram_used_mb: None,
            ram_used_percent: 44.0,
            ram_used_mb: 8192.0,
            ram_total_mb: 16384.0,
            components: vec![SensorReading::new(
                "Package id 0",
                72.5,
                SensorUnit::Celsius,
                None,
                Some(100.0),
                None,
            )],
            polled_at_ms: 1234,
        });
        install_sensor_snapshot(&state, snapshot).await;

        let response = get_sensors(State(state)).await;
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("sensor body should read");
        let json: Value = serde_json::from_slice(&body).expect("sensor response should serialize");

        assert_eq!(json["data"]["cpu_load_percent"], 51.0);
        assert_eq!(json["data"]["cpu_temp_celsius"], 72.5);
        assert_eq!(json["data"]["polled_at_ms"], 1234);
    }

    #[tokio::test]
    async fn single_sensor_endpoint_resolves_normalized_labels() {
        let state = Arc::new(AppState::new());
        let snapshot = Arc::new(SystemSnapshot {
            cpu_load_percent: 40.0,
            cpu_loads: vec![40.0],
            cpu_temp_celsius: Some(68.0),
            gpu_temp_celsius: None,
            gpu_load_percent: None,
            gpu_vram_used_mb: None,
            ram_used_percent: 30.0,
            ram_used_mb: 2048.0,
            ram_total_mb: 8192.0,
            components: vec![SensorReading::new(
                "Package id 0",
                68.0,
                SensorUnit::Celsius,
                None,
                Some(95.0),
                None,
            )],
            polled_at_ms: 77,
        });
        install_sensor_snapshot(&state, snapshot).await;

        let response = get_sensor(State(state), Path("package-id-0".to_owned())).await;
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("single sensor body should read");
        let json: Value =
            serde_json::from_slice(&body).expect("single sensor response should serialize");

        assert_eq!(json["data"]["label"], "Package id 0");
        assert_eq!(json["data"]["value"], 68.0);
        assert_eq!(json["data"]["unit"], "celsius");
    }
}
