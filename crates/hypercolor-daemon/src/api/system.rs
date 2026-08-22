//! System endpoints for daemon identity, status, sensors, and health.
//!
//! Provides daemon status overview and a lightweight health check
//! for monitoring and load balancer probes.

use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::{Extension, State};
use axum::response::{IntoResponse, Response};
use hypercolor_core::engine::RenderLoopState;
use hypercolor_core::input::screen::{
    PixelExtent, ScreenAnalysisComputeCapacity, ScreenAnalysisResourcePlan, ScreenAnalysisWorkPlan,
};
use hypercolor_types::api::system::{
    FullFrameCopySessionStatus, HealthChecks, HealthResponse, LatencyHistogramBucketStatus,
    LatencyHistogramStatus, LatencyPercentilesStatus, RenderLoopStatus,
    ScreenCaptureCapacityStatus, ServerInfo, SessionPerformanceStatus, SystemResource,
    SystemStatus,
};
use hypercolor_types::sensor::SystemSnapshot;

use crate::api::envelope;
use crate::api::security::RequestAuthContext;
use crate::app_state::AppState;
use crate::domain::output::brightness_percent;

use hypercolor_core::config::ConfigManager;

mod audio;
mod input;
mod metrics;

pub(crate) use audio::capture_input_available;
pub use audio::{list_audio_devices, should_offer_named_audio_device};
pub(crate) use input::{actionable_input_diagnostics, input_status_snapshot};
use input::{input_status_snapshot_with_privacy, macos_daemon_ownership};
use metrics::{
    effect_health_status, latest_frame_status, paced_fps, preview_runtime_status,
    render_acceleration_status, round_1, round_2,
};

#[cfg(test)]
use input::{input_source_status, macos_selection_state, macos_tahoe_selection_capabilities};
#[cfg(test)]
mod tests;

const DEFAULT_CONFIG_FILE_NAME: &str = "hypercolor.toml";
const MULTI_ZONE_CAPABILITIES: &[&str] = &[
    "multi-zone-sampling",
    "zone-crud",
    "zone-device-assignment",
    "zone-layout-edit",
    "zone-preview-frames",
    "scene-unassigned-behavior-write",
];

// ── Handlers ─────────────────────────────────────────────────────────────

/// Build a full status response for trusted in-process callers.
pub async fn get_status(State(state): State<Arc<AppState>>) -> Response {
    envelope::ok(system_status_with_privacy(state, true).await)
}

async fn system_status_with_privacy(
    state: Arc<AppState>,
    include_private_selection_ids: bool,
) -> SystemStatus {
    let device_count = state.device_registry.len().await;
    let effect_count = state.domains.effects.len().await;
    let scene_count = state.scene_manager.snapshot().await.scene_count();
    let subscribers = state.event_bus.subscriber_count();

    // Query the live effect engine for the active effect name.
    let active_effect = crate::api::effects::active_primary_effect(state.as_ref())
        .await
        .map(|(_, effect)| effect.name);
    let (active_scene, active_scene_snapshot_locked) = {
        let scene_manager = state.scene_manager.snapshot().await;
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
    let effect_health = effect_health_status(performance.effect_health);
    let preview_runtime = preview_runtime_status(&state.preview_runtime);

    let input_status = input_status_snapshot_with_privacy(&state, include_private_selection_ids);
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
        global_brightness: brightness_percent(state.output_power.global_brightness()),
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

    envelope::ok(SystemResource { identity, status })
}

/// `GET /api/v1/system/sensors` — Latest system sensor snapshot.
pub async fn get_sensors(State(state): State<Arc<AppState>>) -> Response {
    envelope::ok(latest_sensor_snapshot(&state).await.as_ref().clone())
}

async fn server_info(state: &AppState) -> ServerInfo {
    ServerInfo {
        instance_id: state.server_identity.instance_id.clone(),
        instance_name: state.server_identity.instance_name.clone(),
        version: state.server_identity.version.clone(),
        server_session_id: state.server_session_id.clone(),
        device_count: state.device_registry.len().await,
        auth_required: state.security_state.security_enabled(),
    }
}

/// `GET /health` — Lightweight health check (no envelope).
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
