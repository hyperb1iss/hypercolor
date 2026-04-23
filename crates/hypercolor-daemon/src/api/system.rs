//! System endpoints — `/api/v1/status`, `/health`.
//!
//! Provides daemon status overview and a lightweight health check
//! for monitoring and load balancer probes.

use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::response::{IntoResponse, Response};
use hypercolor_core::engine::RenderLoopState;
use hypercolor_types::sensor::SystemSnapshot;
use serde::Serialize;
use utoipa::ToSchema;

use crate::api::AppState;
use crate::api::envelope::{ApiError, ApiResponse};
use crate::api::security;
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
pub struct LatestFrameStatus {
    pub frame_token: u64,
    pub compositor_backend: String,
    pub gpu_zone_sampling: bool,
    pub gpu_sample_deferred: bool,
    pub gpu_sample_retry_hit: bool,
    pub gpu_sample_queue_saturated: bool,
    pub gpu_sample_wait_blocked: bool,
    pub cpu_sampling_late_readback: bool,
    pub cpu_readback_skipped: bool,
    pub total_ms: f64,
    pub wake_late_ms: f64,
    pub frame_age_ms: f64,
    pub producer_render_ms: f64,
    #[serde(rename = "producer_preview_compose_ms")]
    pub producer_scene_compose_ms: f64,
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
        RenderLoopStatus {
            state: snapshot.state.to_string(),
            fps_tier: snapshot.tier.to_string(),
            target_fps: snapshot.tier.fps(),
            ceiling_fps: snapshot.max_tier.fps(),
            actual_fps: round_1(paced_fps(
                snapshot.avg_frame_time.as_secs_f64(),
                snapshot.tier.fps(),
            )),
            consecutive_misses: snapshot.consecutive_misses,
            total_frames: snapshot.total_frames,
        }
    };
    let running = render_loop_is_operational(render_loop_status.state.as_str());
    let performance = state.performance.read().await.snapshot();
    let latest_frame = performance
        .latest_frame
        .map(|frame| latest_frame_status(frame, state.start_time.elapsed().as_secs_f64() * 1000.0));
    let (servo_soft_stalls_total, servo_breaker_opens_total) = servo_effect_health_counts();
    let effect_health = EffectHealthStatus {
        errors_total: performance.effect_health.errors_total,
        fallbacks_applied_total: performance.effect_health.fallbacks_applied_total,
        servo_soft_stalls_total,
        servo_breaker_opens_total,
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
        auth_required: security::api_auth_required_from_env(),
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

#[cfg(feature = "servo")]
fn servo_effect_health_counts() -> (u64, u64) {
    let snapshot = hypercolor_core::effect::servo_telemetry_snapshot();
    (snapshot.soft_stalls_total, snapshot.breaker_opens_total)
}

#[cfg(not(feature = "servo"))]
const fn servo_effect_health_counts() -> (u64, u64) {
    (0, 0)
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
        gpu_sample_retry_hit: frame.gpu_sample_retry_hit,
        gpu_sample_queue_saturated: frame.gpu_sample_queue_saturated,
        gpu_sample_wait_blocked: frame.gpu_sample_wait_blocked,
        cpu_sampling_late_readback: frame.cpu_sampling_late_readback,
        cpu_readback_skipped: frame.cpu_readback_skipped,
        total_ms: round_2(us_to_ms(frame.total_us)),
        wake_late_ms: round_2(us_to_ms(frame.wake_late_us)),
        frame_age_ms: round_2(frame_age_ms),
        producer_render_ms: round_2(us_to_ms(frame.producer_render_us)),
        producer_scene_compose_ms: round_2(us_to_ms(frame.producer_scene_compose_us)),
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
    use super::{get_sensor, get_sensors, get_status};
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
                gpu_sample_retry_hit: true,
                gpu_sample_queue_saturated: true,
                gpu_sample_wait_blocked: true,
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
        let (servo_soft_stalls_total, servo_breaker_opens_total) =
            super::servo_effect_health_counts();

        assert_eq!(json["data"]["render_loop"]["target_fps"], 60);
        assert_eq!(json["data"]["render_loop"]["ceiling_fps"], 60);
        assert_eq!(json["data"]["render_loop"]["actual_fps"], 60.0);
        assert_eq!(json["data"]["latest_frame"]["frame_token"], 77);
        assert_eq!(
            json["data"]["latest_frame"]["compositor_backend"],
            "gpu_fallback"
        );
        assert_eq!(json["data"]["latest_frame"]["gpu_zone_sampling"], true);
        assert_eq!(json["data"]["latest_frame"]["gpu_sample_deferred"], true);
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
            json["data"]["latest_frame"]["cpu_sampling_late_readback"],
            true
        );
        assert_eq!(json["data"]["latest_frame"]["producer_render_ms"], 0.32);
        assert_eq!(
            json["data"]["latest_frame"]["producer_preview_compose_ms"],
            0.06
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
            servo_soft_stalls_total
        );
        assert_eq!(
            json["data"]["effect_health"]["servo_breaker_opens_total"],
            servo_breaker_opens_total
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
