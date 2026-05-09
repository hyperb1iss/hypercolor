//! Diagnostics endpoint — `/api/v1/diagnose`.

use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::response::Response;
use serde::{Deserialize, Serialize};

use crate::api::AppState;
use crate::api::envelope::{ApiError, ApiResponse};

#[derive(Debug, Deserialize)]
pub struct DiagnoseRequest {
    pub checks: Option<Vec<String>>,
    pub system: Option<bool>,
}

#[derive(Debug, Serialize)]
struct DiagnoseResponse {
    checks: Vec<DiagnoseCheck>,
    summary: DiagnoseSummary,
}

#[derive(Debug, Serialize)]
struct DiagnoseCheck {
    category: String,
    name: String,
    status: String,
    detail: String,
}

#[derive(Debug, Serialize)]
struct DiagnoseSummary {
    passed: usize,
    warnings: usize,
    failed: usize,
}

/// `POST /api/v1/diagnose` — Run lightweight daemon diagnostics.
pub async fn run_diagnostics(
    State(state): State<Arc<AppState>>,
    body: Option<Json<DiagnoseRequest>>,
) -> Response {
    let requested = body
        .as_ref()
        .and_then(|b| b.checks.as_ref())
        .cloned()
        .unwrap_or_else(|| {
            vec![
                "daemon".to_owned(),
                "render".to_owned(),
                "devices".to_owned(),
                "config".to_owned(),
            ]
        });

    let include_system = body.as_ref().and_then(|b| b.system).unwrap_or(false);

    let mut checks = Vec::new();

    for check in requested {
        match check.as_str() {
            "daemon" => {
                checks.push(DiagnoseCheck {
                    category: "system".to_owned(),
                    name: "daemon_running".to_owned(),
                    status: "pass".to_owned(),
                    detail: env!("CARGO_PKG_VERSION").to_owned(),
                });
            }
            "render" => {
                let loop_guard = state.render_loop.read().await;
                let running = loop_guard.is_running();
                let render_loop_stats = loop_guard.stats();
                checks.push(DiagnoseCheck {
                    category: "render".to_owned(),
                    name: "render_loop".to_owned(),
                    status: if running { "pass" } else { "warning" }.to_owned(),
                    detail: format!(
                        "state={}, tier={}",
                        render_loop_stats.state, render_loop_stats.tier
                    ),
                });
            }
            "devices" => {
                let count = state.device_registry.len().await;
                checks.push(DiagnoseCheck {
                    category: "devices".to_owned(),
                    name: "registry".to_owned(),
                    status: "pass".to_owned(),
                    detail: format!("{count} tracked"),
                });
            }
            "config" => {
                let has_manager = state.config_manager.is_some();
                checks.push(DiagnoseCheck {
                    category: "config".to_owned(),
                    name: "config_manager".to_owned(),
                    status: if has_manager { "pass" } else { "warning" }.to_owned(),
                    detail: if has_manager {
                        "available".to_owned()
                    } else {
                        "using defaults/test state".to_owned()
                    },
                });
            }
            other => {
                checks.push(DiagnoseCheck {
                    category: "custom".to_owned(),
                    name: other.to_owned(),
                    status: "warning".to_owned(),
                    detail: "unknown check".to_owned(),
                });
            }
        }
    }

    if include_system {
        checks.push(DiagnoseCheck {
            category: "system".to_owned(),
            name: "uptime_seconds".to_owned(),
            status: "pass".to_owned(),
            detail: state.start_time.elapsed().as_secs().to_string(),
        });
    }

    let mut passed = 0usize;
    let mut warnings = 0usize;
    let mut failed = 0usize;

    for check in &checks {
        match check.status.as_str() {
            "pass" => passed += 1,
            "fail" => failed += 1,
            _ => warnings += 1,
        }
    }

    ApiResponse::ok(DiagnoseResponse {
        checks,
        summary: DiagnoseSummary {
            passed,
            warnings,
            failed,
        },
    })
}

/// `GET /api/v1/diagnose/memory` — Capture Servo memory profiler output.
pub async fn memory_diagnostics() -> Response {
    #[cfg(feature = "servo")]
    {
        match hypercolor_core::effect::servo_memory_report_snapshot() {
            Ok(snapshot) => ApiResponse::ok(snapshot),
            Err(error) => {
                ApiError::internal(format!("Failed to collect Servo memory report: {error}"))
            }
        }
    }

    #[cfg(not(feature = "servo"))]
    {
        ApiError::not_found("Servo memory diagnostics are not available in this build")
    }
}
