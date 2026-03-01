//! System endpoints — `/api/v1/status`, `/health`.
//!
//! Provides daemon status overview and a lightweight health check
//! for monitoring and load balancer probes.

use std::sync::Arc;

use axum::extract::State;
use axum::response::{IntoResponse, Response};
use serde::Serialize;

use crate::api::AppState;
use crate::api::envelope::ApiResponse;

// ── Response Types ───────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct SystemStatus {
    pub running: bool,
    pub version: String,
    pub uptime_seconds: u64,
    pub device_count: usize,
    pub effect_count: usize,
    pub scene_count: usize,
    pub active_effect: Option<String>,
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

    ApiResponse::ok(SystemStatus {
        running: true,
        version: env!("CARGO_PKG_VERSION").to_owned(),
        uptime_seconds,
        device_count,
        effect_count,
        scene_count,
        active_effect,
        render_loop: render_loop_status,
        event_bus_subscribers: subscribers,
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
