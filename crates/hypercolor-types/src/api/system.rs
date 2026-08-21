//! System API contracts for daemon identity, operational status, and inputs.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::server::ServerIdentity;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema, Default)]
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
    pub macos_daemon_ownership: Option<MacosDaemonOwnershipStatus>,
    pub compositor_acceleration: RenderAccelerationStatus,
    pub render_loop: RenderLoopStatus,
    pub session_performance: SessionPerformanceStatus,
    pub latest_frame: Option<LatestFrameStatus>,
    pub effect_health: EffectHealthStatus,
    pub preview_runtime: PreviewRuntimeStatus,
    pub event_bus_subscribers: usize,
    pub capabilities: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema, Default)]
pub struct SessionPerformanceStatus {
    pub input_stage: LatencyPercentilesStatus,
    pub full_frame_cpu_copies: FullFrameCopySessionStatus,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema, Default)]
pub struct LatencyPercentilesStatus {
    pub sample_count: u64,
    pub avg_ms: f64,
    pub p95_ms: f64,
    pub p99_ms: f64,
    pub max_ms: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cumulative_histogram: Option<LatencyHistogramStatus>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema, Default)]
pub struct LatencyHistogramStatus {
    pub bucket_width_us: u32,
    pub overflow_bucket_index: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snapshot_frame_token: Option<u64>,
    pub buckets: Vec<LatencyHistogramBucketStatus>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema, Default)]
pub struct LatencyHistogramBucketStatus {
    pub bucket_index: u32,
    pub count: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema, Default)]
pub struct FullFrameCopySessionStatus {
    pub count: u64,
    pub frames: u64,
    pub bytes: u64,
}

/// Installed byte fences for transactional screen publication admission.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema, Default)]
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
    /// Build a status snapshot for hosts without enforced capture capacity.
    #[must_use]
    pub const fn without_capacity(admission_enforced: bool) -> Self {
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
/// input nodes present but unreadable (udev rules missing), the signal
/// that distinguishes "input is off" from "input is on but blocked".
///
/// `degraded` carries the failures the counters cannot express. Windows has no
/// per-device denial to count: either the process has a visible window station
/// and sees input, or it does not, and that is a session-level fact rather than
/// a per-node one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema, Default)]
pub struct InputStatus {
    pub enabled: bool,
    pub host_capture_registered: bool,
    pub host_capturing: bool,
    pub devices_opened: usize,
    pub devices_denied: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub degraded: Option<String>,
    pub backends: Vec<String>,
    pub source_graph_generation: u64,
    pub sources: Vec<InputSourceStatus>,
}

/// Structured source issue safe for operational status surfaces.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema, Default)]
pub struct InputSourceIssueStatus {
    pub code: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remediation: Option<String>,
    pub retryable: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum MacosProtectedSourceState {
    #[default]
    Disabled,
    NeedsUserAction,
    PermissionDenied,
    NeedsProcessRestart,
    NeedsSelection,
    ReadyIdle,
    Starting,
    Live,
    Interrupted,
    Revoked,
    Failed,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum MacosAuthorizationState {
    #[default]
    Unknown,
    NotDetermined,
    Denied,
    Authorized,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum MacosCapabilityOwner {
    AppSidecar,
    App,
    LaunchdService,
    HomebrewService,
    Broker,
    #[default]
    Standalone,
}

impl MacosCapabilityOwner {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AppSidecar => "app_sidecar",
            Self::App => "app",
            Self::LaunchdService => "launchd_service",
            Self::HomebrewService => "homebrew_service",
            Self::Broker => "broker",
            Self::Standalone => "standalone",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema, Default)]
pub struct MacosDaemonOwnerConflictStatus {
    pub active: MacosCapabilityOwner,
    pub contender: MacosCapabilityOwner,
    pub observed_at_ms: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum MacosDaemonHandoverPhase {
    #[default]
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema, Default)]
pub struct MacosDaemonOwnerRecoveryRequiredStatus {
    pub requested_owner: MacosCapabilityOwner,
    pub prior_owner: MacosCapabilityOwner,
    pub phase: MacosDaemonHandoverPhase,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema, Default)]
pub struct MacosDaemonOwnershipStatus {
    pub active_owner: MacosCapabilityOwner,
    pub owner_epoch: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conflict: Option<MacosDaemonOwnerConflictStatus>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recovery_required: Option<MacosDaemonOwnerRecoveryRequiredStatus>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MacosSelectionState {
    #[default]
    None,
    Display {
        source_id: String,
    },
    SessionScoped {
        content_style: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema, Default)]
pub struct MacosTahoeSelectionCapabilities {
    pub source_id: String,
    pub capture_session_generation: u64,
    pub hdr_capture: bool,
    pub dual_range_screenshots: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum MacosArchitecture {
    AppleSilicon,
    #[default]
    Intel,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema, Default)]
pub struct MacosTahoeCapabilities {
    pub host_architecture: MacosArchitecture,
    pub translated_process: bool,
    pub content_tone_mapping_info: bool,
    pub metal4: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema, Default)]
pub struct MacosInputTelemetry {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authorization_last_transition_age_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_designated_requirement_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_architecture: Option<MacosArchitecture>,
    pub executable_architecture: MacosArchitecture,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub translated_process: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capture_session_generation: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub topology_generation: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queue_capacity: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queue_depth: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_events_received: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_events_published: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_events_dropped: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tap_disabled_timeout: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tap_disabled_user_input: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tap_reenabled: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state_gaps: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub callback_to_publication_timing: Option<MacosTiming>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema, Default)]
pub struct MacosTiming {
    pub sample_count: u64,
    pub total_ns: u64,
    pub max_ns: u64,
    pub p95_ns: u64,
    pub p99_ns: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema, Default)]
pub struct MacosScreenTiming {
    pub callback: MacosTiming,
    pub retain: MacosTiming,
    pub enqueue: MacosTiming,
    pub conversion: MacosTiming,
    pub cpu_reduction: MacosTiming,
    pub native_import: MacosTiming,
    pub native_reduction_submit: MacosTiming,
    pub publication: MacosTiming,
    pub capture_to_native_publication: MacosTiming,
    pub capture_to_converted_publication: MacosTiming,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema, Default)]
pub struct MacosFrameDrop {
    pub reason: String,
    pub count: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema, Default)]
pub struct MacosScreenTelemetry {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authorization_last_transition_age_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_designated_requirement_hash: Option<String>,
    pub executable_architecture: MacosArchitecture,
    pub stream_state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capture_session_generation: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub topology_generation: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource_generation: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publication_plan_generation: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pixel_format: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dynamic_range: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color_space: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transfer_function: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selection_diagnostic_label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_scale: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_width: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_height: Option<u32>,
    pub queue_depth: usize,
    pub admitted_native_bytes: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pinned_generations: Option<usize>,
    pub frames_received: u64,
    pub frames_published: u64,
    pub frames_superseded: u64,
    pub frames_malformed: u64,
    pub frames_dropped: Vec<MacosFrameDrop>,
    pub frames_stale: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publication_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timing: Option<MacosScreenTiming>,
    pub callback_total_ns: u64,
    pub callback_max_ns: u64,
    pub retain_total_ns: u64,
    pub retain_max_ns: u64,
    pub conversion_total_ns: u64,
    pub conversion_max_ns: u64,
    pub cpu_reduction_total_ns: u64,
    pub cpu_reduction_max_ns: u64,
    pub native_import_total_ns: u64,
    pub native_import_max_ns: u64,
    pub native_reduction_submit_total_ns: u64,
    pub native_reduction_submit_max_ns: u64,
    pub publication_total_ns: u64,
    pub publication_max_ns: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum InputSourcePlatformStatus {
    MacosInput {
        keyboard: MacosProtectedSourceState,
        pointer: MacosProtectedSourceState,
        keyboard_tcc: MacosAuthorizationState,
        secure_input_active: bool,
        keyboard_owner: MacosCapabilityOwner,
        pointer_owner: MacosCapabilityOwner,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        owner_conflict: Option<MacosDaemonOwnerConflictStatus>,
        telemetry: MacosInputTelemetry,
    },
    MacosScreen {
        state: MacosProtectedSourceState,
        tcc: MacosAuthorizationState,
        owner: MacosCapabilityOwner,
        selection: MacosSelectionState,
        tahoe: MacosTahoeCapabilities,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tahoe_selection: Option<MacosTahoeSelectionCapabilities>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        owner_conflict: Option<MacosDaemonOwnerConflictStatus>,
        telemetry: MacosScreenTelemetry,
    },
}

/// Lock-free lifecycle and freshness status for one input source.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, ToSchema)]
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
    pub platform: Option<InputSourcePlatformStatus>,
    pub retired: bool,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema, Default)]
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema, Default)]
pub struct RenderAccelerationStatus {
    pub requested_mode: String,
    pub effective_mode: String,
    pub fallback_reason: Option<String>,
    pub servo_gpu_import_mode: String,
    pub servo_gpu_import_attempting: bool,
    pub gpu_probe: Option<GpuCompositorProbeStatus>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema, Default)]
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema, Default)]
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema, Default)]
pub struct RenderSurfaceStatus {
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema, Default)]
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
    pub servo_gpu_import_fallback_reason: Option<String>,
    pub servo_gpu_import_windows_sync_mode: Option<String>,
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema, Default)]
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema, Default)]
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema, Default)]
pub struct HealthResponse {
    pub status: String,
    pub version: String,
    pub uptime_seconds: u64,
    pub checks: HealthChecks,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema, Default)]
pub struct HealthChecks {
    pub render_loop: String,
    pub device_backends: String,
    pub event_bus: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema, Default)]
pub struct ServerInfo {
    pub instance_id: String,
    pub instance_name: String,
    pub version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_session_id: Option<String>,
    pub device_count: usize,
    pub auth_required: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema, Default)]
pub struct SystemResource {
    pub identity: ServerInfo,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<SystemStatus>,
}
/// One selectable audio input source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema, Default)]
pub struct AudioDeviceInfo {
    pub id: String,
    pub name: String,
    pub description: String,
}

/// The audio input inventory plus the configured selection.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema, Default)]
pub struct AudioDevicesResponse {
    pub devices: Vec<AudioDeviceInfo>,
    pub current: String,
}
