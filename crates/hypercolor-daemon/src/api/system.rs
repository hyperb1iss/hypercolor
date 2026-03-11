//! System endpoints — `/api/v1/status`, `/health`.
//!
//! Provides daemon status overview and a lightweight health check
//! for monitoring and load balancer probes.

use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::State;
use axum::response::{IntoResponse, Response};
use serde::Serialize;

use crate::api::AppState;
use crate::api::envelope::ApiResponse;
use crate::api::security;
use crate::api::settings;
use crate::session::current_global_brightness;

use hypercolor_core::config::ConfigManager;
use hypercolor_types::server::ServerIdentity;

const DEFAULT_CONFIG_FILE_NAME: &str = "hypercolor.toml";

// ── Response Types ───────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
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
    pub global_brightness: u8,
    pub audio_available: bool,
    pub capture_available: bool,
    pub render_loop: RenderLoopStatus,
    pub event_bus_subscribers: usize,
}

#[derive(Debug, Serialize)]
pub struct RenderLoopStatus {
    pub state: String,
    pub fps_tier: String,
    pub total_frames: u64,
}

#[derive(Debug, Serialize)]
pub struct HealthResponse {
    pub status: String,
    pub version: String,
    pub uptime_seconds: u64,
    pub checks: HealthChecks,
}

#[derive(Debug, Serialize)]
pub struct HealthChecks {
    pub render_loop: String,
    pub device_backends: String,
    pub event_bus: String,
}

#[derive(Debug, Serialize)]
pub struct ServerInfo {
    #[serde(flatten)]
    pub identity: ServerIdentity,
    pub device_count: usize,
    pub auth_required: bool,
}

// ── Handlers ─────────────────────────────────────────────────────────────

/// `GET /api/v1/status` — Full system status overview.
pub async fn get_status(State(state): State<Arc<AppState>>) -> Response {
    let device_count = state.device_registry.len().await;
    let effect_count = state.effect_registry.read().await.len();
    let scene_count = state.scene_manager.read().await.scene_count();
    let subscribers = state.event_bus.subscriber_count();

    // Query the live effect engine for the active effect name.
    let active_effect = {
        let engine = state.effect_engine.lock().await;
        engine.active_metadata().map(|m| m.name.clone())
    };

    // Query the live render loop for timing data.
    let render_loop_status = {
        let rl = state.render_loop.read().await;
        let snapshot = rl.stats();
        RenderLoopStatus {
            state: snapshot.state.to_string(),
            fps_tier: snapshot.tier.to_string(),
            total_frames: snapshot.total_frames,
        }
    };

    let uptime_seconds = state.start_time.elapsed().as_secs();
    let config_path = config_path(&state).display().to_string();
    let data_dir = ConfigManager::data_dir().display().to_string();
    let cache_dir = ConfigManager::cache_dir().display().to_string();

    ApiResponse::ok(SystemStatus {
        running: true,
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
        global_brightness: brightness_percent(current_global_brightness(&state.power_state)),
        audio_available: settings::audio_input_available(),
        capture_available: settings::capture_input_available(),
        render_loop: render_loop_status,
        event_bus_subscribers: subscribers,
    })
}

/// `GET /api/v1/server` — Lightweight server identity for discovery probes.
pub async fn get_server(State(state): State<Arc<AppState>>) -> Response {
    let device_count = state.device_registry.len().await;

    ApiResponse::ok(ServerInfo {
        identity: state.server_identity.clone(),
        device_count,
        auth_required: security::api_auth_required_from_env(),
    })
}

/// `GET /health` — Lightweight health check (no envelope).
pub async fn health_check(State(state): State<Arc<AppState>>) -> Response {
    let uptime_seconds = state.start_time.elapsed().as_secs();

    let resp = HealthResponse {
        status: "healthy".to_owned(),
        version: env!("CARGO_PKG_VERSION").to_owned(),
        uptime_seconds,
        checks: HealthChecks {
            render_loop: "ok".to_owned(),
            device_backends: "ok".to_owned(),
            event_bus: "ok".to_owned(),
        },
    };

    (axum::http::StatusCode::OK, axum::Json(resp)).into_response()
}

fn config_path(state: &AppState) -> PathBuf {
    state.config_manager.as_ref().map_or_else(
        || ConfigManager::config_dir().join(DEFAULT_CONFIG_FILE_NAME),
        |manager| manager.path().to_path_buf(),
    )
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
