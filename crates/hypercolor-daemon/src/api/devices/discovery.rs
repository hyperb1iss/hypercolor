//! Discovery endpoints — `/api/v1/devices/discover`.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use axum::Json;
use axum::extract::State;
use axum::response::Response;
use serde::Deserialize;
use utoipa::ToSchema;

use hypercolor_types::config::HypercolorConfig;

use crate::api::AppState;
use crate::api::envelope::{ApiError, ApiResponse};
use crate::discovery;

#[derive(Debug, Deserialize, ToSchema)]
pub struct DiscoverRequest {
    pub targets: Option<Vec<String>>,
    pub timeout_ms: Option<u64>,
    pub wait: Option<bool>,
}

/// `POST /api/v1/devices/discover` — Trigger device discovery.
pub async fn discover_devices(
    State(state): State<Arc<AppState>>,
    body: Option<Json<DiscoverRequest>>,
) -> Response {
    let config = state.config_manager.as_ref().map_or_else(
        || Arc::new(HypercolorConfig::default()),
        |manager| Arc::clone(&manager.get()),
    );
    let requested_targets = body.as_ref().and_then(|request| request.targets.as_ref());
    let resolved_targets = match discovery::resolve_targets(
        requested_targets.map(Vec::as_slice),
        config.as_ref(),
        state.driver_registry.as_ref(),
    ) {
        Ok(targets) => targets,
        Err(error) => return ApiError::validation(error),
    };
    let timeout = discovery::normalize_timeout_ms(body.as_ref().and_then(|b| b.timeout_ms));
    let wait_for_completion = body.as_ref().and_then(|b| b.wait).unwrap_or(false);

    if state
        .discovery_in_progress
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return ApiError::conflict("A discovery scan is already in progress");
    }

    let scan_id = format!("scan_{}", uuid::Uuid::now_v7());
    let target_names = discovery::target_names(&resolved_targets);
    if wait_for_completion {
        let runtime = state.driver_host.discovery_runtime();
        let result = discovery::execute_discovery_scan(
            runtime,
            Arc::clone(&state.driver_registry),
            Arc::clone(&state.driver_host),
            config,
            resolved_targets,
            timeout,
        )
        .await;

        return ApiResponse::ok(serde_json::json!({
            "scan_id": scan_id,
            "status": "completed",
            "result": result,
        }));
    }

    let state_for_task = Arc::clone(&state);
    tokio::spawn(async move {
        let runtime = state_for_task.driver_host.discovery_runtime();
        let _ = discovery::execute_discovery_scan(
            runtime,
            Arc::clone(&state_for_task.driver_registry),
            Arc::clone(&state_for_task.driver_host),
            config,
            resolved_targets,
            timeout,
        )
        .await;
    });

    ApiResponse::accepted(serde_json::json!({
        "scan_id": scan_id,
        "status": "scanning",
        "targets": target_names,
        "timeout_ms": u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX),
    }))
}
