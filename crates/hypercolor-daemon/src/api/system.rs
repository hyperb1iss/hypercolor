//! System endpoints — `/api/v1/status`, `/health`.
//!
//! Provides daemon status overview and a lightweight health check
//! for monitoring and load balancer probes.

use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::response::{IntoResponse, Response};
use hypercolor_core::engine::RenderLoopState;
use hypercolor_types::config::RenderAccelerationMode;
use hypercolor_types::sensor::SystemSnapshot;
use serde::Serialize;
use utoipa::ToSchema;

use crate::api::AppState;
use crate::api::envelope::{ApiError, ApiResponse};
use crate::api::settings;
use crate::performance::LatestFrameMetrics;
use crate::preview_runtime::{PreviewDemandSummary, PreviewRuntime};
use crate::session::current_global_brightness;

use hypercolor_core::config::ConfigManager;
use hypercolor_types::server::ServerIdentity;

const DEFAULT_CONFIG_FILE_NAME: &str = "hypercolor.toml";

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
    pub compositor_acceleration: RenderAccelerationStatus,
    pub render_loop: RenderLoopStatus,
    pub latest_frame: Option<LatestFrameStatus>,
    pub effect_health: EffectHealthStatus,
    pub preview_runtime: PreviewRuntimeStatus,
    pub event_bus_subscribers: usize,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct RenderLoopStatus {
    pub state: String,
    pub fps_tier: String,
    pub target_fps: u32,
    pub ceiling_fps: u32,
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
    pub backend: String,
    pub texture_format: String,
    pub max_texture_dimension_2d: u32,
    pub max_storage_textures_per_shader_stage: u32,
    pub linux_servo_gpu_import_backend_compatible: bool,
    pub linux_servo_gpu_import_backend_reason: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct LatestFrameStatus {
    pub frame_token: u64,
    pub compositor_backend: String,
    pub gpu_zone_sampling: bool,
    pub gpu_sample_deferred: bool,
    pub gpu_sample_stale: bool,
    pub gpu_sample_retry_hit: bool,
    pub gpu_sample_queue_saturated: bool,
    pub gpu_sample_wait_blocked: bool,
    pub gpu_sample_cpu_fallback: bool,
    pub cpu_sampling_late_readback: bool,
    pub cpu_readback_skipped: bool,
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
    pub output_errors: u32,
    pub render_surfaces: RenderSurfaceStatus,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct RenderSurfaceStatus {
    pub slot_count: u32,
    pub free_slots: u32,
    pub published_slots: u32,
    pub dequeued_slots: u32,
    pub canvas_receivers: u32,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct EffectHealthStatus {
    pub errors_total: u64,
    pub fallbacks_applied_total: u64,
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
    pub servo_render_cpu_frames_total: u64,
    pub servo_render_cached_frames_total: u64,
    pub servo_render_gpu_frames_total: u64,
    pub servo_gpu_import_failures_total: u64,
    pub servo_gpu_import_fallbacks_total: u64,
    pub servo_gpu_import_fallback_reason: Option<&'static str>,
    pub servo_gpu_import_blit_total_ms: f64,
    pub servo_gpu_import_blit_max_ms: f64,
    pub servo_gpu_import_sync_total_ms: f64,
    pub servo_gpu_import_sync_max_ms: f64,
    pub servo_gpu_import_total_ms: f64,
    pub servo_gpu_import_max_ms: f64,
    pub producer_cpu_frames_total: u64,
    pub producer_gpu_frames_total: u64,
    pub sparkleflinger_gpu_source_upload_skipped_total: u64,
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
    pub screen_canvas_receivers: u32,
    pub canvas_frames_published: u64,
    pub screen_canvas_frames_published: u64,
    pub latest_canvas_frame_number: u32,
    pub latest_screen_canvas_frame_number: u32,
    pub canvas_demand: PreviewDemandStatus,
    pub screen_canvas_demand: PreviewDemandStatus,
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
    pub device_count: usize,
    pub auth_required: bool,
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

    // Query the live render loop for timing data.
    let render_loop_status = {
        let rl = state.render_loop.read().await;
        let snapshot = rl.stats();
        let actual_fps = if snapshot.state == RenderLoopState::Running {
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
            actual_fps,
            consecutive_misses: snapshot.consecutive_misses,
            total_frames: snapshot.total_frames,
        }
    };
    let running = render_loop_is_operational(render_loop_status.state.as_str());
    let performance = state.performance.read().await.snapshot();
    let latest_frame = if render_loop_status.state == "running" {
        performance.latest_frame.map(|frame| {
            latest_frame_status(frame, state.start_time.elapsed().as_secs_f64() * 1000.0)
        })
    } else {
        None
    };
    let servo_health = servo_effect_health_counts();
    let pipeline_health = render_pipeline_health_counts();
    let effect_health = EffectHealthStatus {
        errors_total: performance.effect_health.errors_total,
        fallbacks_applied_total: performance.effect_health.fallbacks_applied_total,
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
        servo_render_cpu_frames_total: servo_health.render_cpu_frames_total,
        servo_render_cached_frames_total: servo_health.render_cached_frames_total,
        servo_render_gpu_frames_total: servo_health.render_gpu_frames_total,
        servo_gpu_import_failures_total: servo_health.render_gpu_import_failures_total,
        servo_gpu_import_fallbacks_total: servo_health.render_gpu_import_fallbacks_total,
        servo_gpu_import_fallback_reason: servo_health.render_gpu_import_fallback_reason,
        servo_gpu_import_blit_total_ms: us_to_ms_f64(servo_health.render_gpu_import_blit_total_us),
        servo_gpu_import_blit_max_ms: us_to_ms_f64(servo_health.render_gpu_import_blit_max_us),
        servo_gpu_import_sync_total_ms: us_to_ms_f64(servo_health.render_gpu_import_sync_total_us),
        servo_gpu_import_sync_max_ms: us_to_ms_f64(servo_health.render_gpu_import_sync_max_us),
        servo_gpu_import_total_ms: us_to_ms_f64(servo_health.render_gpu_import_total_us),
        servo_gpu_import_max_ms: us_to_ms_f64(servo_health.render_gpu_import_max_us),
        producer_cpu_frames_total: pipeline_health.cpu_producer_frames,
        producer_gpu_frames_total: pipeline_health.gpu_producer_frames,
        sparkleflinger_gpu_source_upload_skipped_total: pipeline_health.skipped_gpu_source_uploads,
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

    let uptime_seconds = state.start_time.elapsed().as_secs();
    let config_path = config_path(&state).display().to_string();
    let data_dir = ConfigManager::data_dir().display().to_string();
    let cache_dir = ConfigManager::cache_dir().display().to_string();

    ApiResponse::ok(SystemStatus {
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
        audio_available: settings::audio_input_available(),
        capture_available: settings::capture_input_available(),
        compositor_acceleration: render_acceleration_status(&state.render_acceleration),
        render_loop: render_loop_status,
        latest_frame,
        effect_health,
        preview_runtime,
        event_bus_subscribers: subscribers,
    })
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

    ApiError::not_found(format!("sensor '{label}' was not found"))
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
    let device_count = state.device_registry.len().await;

    ApiResponse::ok(ServerInfo {
        identity: state.server_identity.clone(),
        device_count,
        auth_required: state.security_state.security_enabled(),
    })
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

async fn latest_sensor_snapshot(state: &AppState) -> Arc<SystemSnapshot> {
    let input_manager = state.input_manager.lock().await;
    input_manager
        .latest_sensor_snapshot()
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
                backend: probe.backend.to_owned(),
                texture_format: probe.texture_format.to_owned(),
                max_texture_dimension_2d: probe.max_texture_dimension_2d,
                max_storage_textures_per_shader_stage: probe.max_storage_textures_per_shader_stage,
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
    render_cpu_frames_total: u64,
    render_cached_frames_total: u64,
    render_gpu_frames_total: u64,
    render_gpu_import_failures_total: u64,
    render_gpu_import_fallbacks_total: u64,
    render_gpu_import_fallback_reason: Option<&'static str>,
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
        render_cpu_frames_total: snapshot.render_cpu_frames_total,
        render_cached_frames_total: snapshot.render_cached_frames_total,
        render_gpu_frames_total: snapshot.render_gpu_frames_total,
        render_gpu_import_failures_total: snapshot.render_gpu_import_failures_total,
        render_gpu_import_fallbacks_total: snapshot.render_gpu_import_fallbacks_total,
        render_gpu_import_fallback_reason: snapshot.render_gpu_import_fallback_reason,
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
        render_cpu_frames_total: 0,
        render_cached_frames_total: 0,
        render_gpu_frames_total: 0,
        render_gpu_import_failures_total: 0,
        render_gpu_import_fallbacks_total: 0,
        render_gpu_import_fallback_reason: None,
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
    skipped_gpu_source_uploads: u64,
}

fn render_pipeline_health_counts() -> RenderPipelineHealthCounts {
    let producer = crate::render_thread::producer_frame_counts();
    RenderPipelineHealthCounts {
        cpu_producer_frames: producer.cpu_frames_total,
        gpu_producer_frames: producer.gpu_frames_total,
        skipped_gpu_source_uploads: gpu_source_upload_skipped_total(),
    }
}

#[cfg(feature = "wgpu")]
fn gpu_source_upload_skipped_total() -> u64 {
    crate::render_thread::sparkleflinger::gpu::gpu_source_upload_skipped_total()
}

#[cfg(not(feature = "wgpu"))]
const fn gpu_source_upload_skipped_total() -> u64 {
    0
}

fn latest_frame_status(frame: LatestFrameMetrics, render_elapsed_ms: f64) -> LatestFrameStatus {
    let frame_age_ms = if frame.timestamp_ms > 0 {
        (render_elapsed_ms - f64::from(frame.timestamp_ms)).max(0.0)
    } else {
        0.0
    };

    LatestFrameStatus {
        frame_token: frame.timeline.frame_token,
        compositor_backend: frame.compositor_backend.as_str().to_owned(),
        gpu_zone_sampling: frame.gpu_zone_sampling,
        gpu_sample_deferred: frame.gpu_sample_deferred,
        gpu_sample_stale: frame.gpu_sample_stale,
        gpu_sample_retry_hit: frame.gpu_sample_retry_hit,
        gpu_sample_queue_saturated: frame.gpu_sample_queue_saturated,
        gpu_sample_wait_blocked: frame.gpu_sample_wait_blocked,
        gpu_sample_cpu_fallback: frame.gpu_sample_cpu_fallback,
        cpu_sampling_late_readback: frame.cpu_sampling_late_readback,
        cpu_readback_skipped: frame.cpu_readback_skipped,
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
        output_errors: frame.output_errors,
        render_surfaces: RenderSurfaceStatus {
            slot_count: frame.render_surface_slot_count,
            free_slots: frame.render_surface_free_slots,
            published_slots: frame.render_surface_published_slots,
            dequeued_slots: frame.render_surface_dequeued_slots,
            canvas_receivers: frame.canvas_receiver_count,
        },
    }
}

fn preview_runtime_status(runtime: &PreviewRuntime) -> PreviewRuntimeStatus {
    let snapshot = runtime.snapshot();
    PreviewRuntimeStatus {
        canvas_receivers: snapshot.canvas_receivers,
        screen_canvas_receivers: snapshot.screen_canvas_receivers,
        canvas_frames_published: snapshot.canvas_frames_published,
        screen_canvas_frames_published: snapshot.screen_canvas_frames_published,
        latest_canvas_frame_number: snapshot.latest_canvas_frame_number,
        latest_screen_canvas_frame_number: snapshot.latest_screen_canvas_frame_number,
        canvas_demand: preview_demand_status(runtime.canvas_demand()),
        screen_canvas_demand: preview_demand_status(runtime.screen_canvas_demand()),
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

#[cfg(test)]
mod tests {
    use super::{get_sensor, get_sensors, get_status, us_to_ms_f64};
    use crate::api::AppState;
    use crate::performance::{CompositorBackendKind, FrameTimeline, LatestFrameMetrics};
    use crate::preview_runtime::{PreviewPixelFormat, PreviewStreamDemand};
    use axum::body::to_bytes;
    use axum::extract::{Path, State};
    use hypercolor_core::bus::CanvasFrame;
    use hypercolor_types::canvas::Canvas;
    use hypercolor_types::sensor::{SensorReading, SensorUnit, SystemSnapshot};
    use serde_json::Value;
    use std::sync::Arc;
    use tokio::sync::watch;

    #[expect(
        clippy::too_many_lines,
        reason = "Status response assertions cover many nested metrics fields in one scenario"
    )]
    #[tokio::test]
    async fn status_includes_latest_frame_surface_stats() {
        let state = Arc::new(AppState::new());
        state.render_loop.write().await.start();
        let mut preview_rx = state.preview_runtime.canvas_receiver();
        let mut screen_preview_rx = state.preview_runtime.screen_canvas_receiver();
        preview_rx.update_demand(PreviewStreamDemand {
            fps: 24,
            format: PreviewPixelFormat::Jpeg,
            width: 640,
            height: 360,
        });
        screen_preview_rx.update_demand(PreviewStreamDemand {
            fps: 30,
            format: PreviewPixelFormat::Rgba,
            width: 0,
            height: 0,
        });
        let canvas_frame = CanvasFrame::from_canvas(&Canvas::new(2, 1), 88, 44);
        let screen_frame = CanvasFrame::from_canvas(&Canvas::new(1, 1), 45, 21);
        let _ = state.event_bus.canvas_sender().send(canvas_frame.clone());
        let _ = state
            .event_bus
            .screen_canvas_sender()
            .send(screen_frame.clone());
        state
            .preview_runtime
            .record_canvas_publication(canvas_frame.frame_number, canvas_frame.timestamp_ms);
        state
            .preview_runtime
            .record_screen_canvas_publication(screen_frame.frame_number, screen_frame.timestamp_ms);
        {
            let mut performance = state.performance.write().await;
            performance.record_effect_error();
            performance.record_effect_fallback_applied();
            performance.record_frame(LatestFrameMetrics {
                timestamp_ms: 40,
                input_us: 100,
                producer_us: 500,
                producer_render_us: 320,
                producer_scene_compose_us: 60,
                composition_us: 200,
                render_us: 700,
                sample_us: 150,
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
                cpu_sampling_late_readback: true,
                cpu_readback_skipped: true,
                compositor_backend: CompositorBackendKind::GpuFallback,
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
                canvas_receiver_count: 2,
                full_frame_copy_count: 1,
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
            });
        }

        let response = get_status(State(state)).await;
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("status body should read");
        let json: Value = serde_json::from_slice(&body).expect("status should serialize");
        let servo_health = super::servo_effect_health_counts();

        assert_eq!(json["data"]["render_loop"]["target_fps"], 60);
        assert_eq!(json["data"]["render_loop"]["ceiling_fps"], 60);
        assert_eq!(json["data"]["render_loop"]["actual_fps"], 60.0);
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
        assert_eq!(json["data"]["latest_frame"]["frame_token"], 77);
        assert_eq!(
            json["data"]["latest_frame"]["compositor_backend"],
            "gpu_fallback"
        );
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
        assert_eq!(
            json["data"]["latest_frame"]["render_surfaces"]["slot_count"],
            6
        );
        assert_eq!(
            json["data"]["latest_frame"]["render_surfaces"]["canvas_receivers"],
            2
        );
        assert_eq!(json["data"]["latest_frame"]["full_frame_copy_count"], 1);
        assert_eq!(json["data"]["latest_frame"]["full_frame_copy_kb"], 250.0);
        assert_eq!(json["data"]["latest_frame"]["output_errors"], 0);
        assert_eq!(json["data"]["effect_health"]["errors_total"], 1);
        assert_eq!(json["data"]["effect_health"]["fallbacks_applied_total"], 1);
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
            json["data"]["effect_health"]["servo_render_cpu_frames_total"],
            servo_health.render_cpu_frames_total
        );
        assert_eq!(
            json["data"]["effect_health"]["servo_render_cached_frames_total"],
            servo_health.render_cached_frames_total
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
        assert_eq!(
            json["data"]["preview_runtime"]["screen_canvas_receivers"],
            1
        );
        assert_eq!(
            json["data"]["preview_runtime"]["canvas_frames_published"],
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
        let (_tx, rx) = watch::channel(snapshot);
        state
            .input_manager
            .lock()
            .await
            .set_sensor_snapshot_receiver(rx);

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
        let (_tx, rx) = watch::channel(snapshot);
        state
            .input_manager
            .lock()
            .await
            .set_sensor_snapshot_receiver(rx);

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
