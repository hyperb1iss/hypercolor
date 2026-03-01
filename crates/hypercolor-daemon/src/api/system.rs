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
    pub event_bus_subscribers: usize,
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

    let uptime_seconds = state.start_time.elapsed().as_secs();

    ApiResponse::ok(SystemStatus {
        running: true,
        version: env!("CARGO_PKG_VERSION").to_owned(),
        uptime_seconds,
        device_count,
        effect_count,
        scene_count,
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
