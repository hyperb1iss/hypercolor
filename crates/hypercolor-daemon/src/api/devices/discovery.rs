//! Discovery endpoints — `/api/v1/devices/discover`.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use axum::Json;
use axum::extract::State;
use axum::response::{IntoResponse, Response};

use hypercolor_types::config::HypercolorConfig;

use crate::api::envelope;
use crate::app_state::AppState;
use crate::discovery;
use crate::domain::DomainError;

pub use hypercolor_types::api::devices::{
    DiscoverRequest, DiscoverResponse, DiscoveryCompletedResponse, DiscoveryStartedResponse,
};

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
        Err(error) => return DomainError::validation(error).into_response(),
    };
    let timeout = discovery::normalize_timeout_ms(body.as_ref().and_then(|b| b.timeout_ms));
    let wait_for_completion = body.as_ref().and_then(|b| b.wait).unwrap_or(false);

    if state
        .discovery_in_progress
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return DomainError::conflict("A discovery scan is already in progress").into_response();
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

        return envelope::ok(DiscoverResponse::Completed(DiscoveryCompletedResponse {
            scan_id,
            status: "completed".to_owned(),
            result,
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

    envelope::accepted(DiscoverResponse::Started(DiscoveryStartedResponse {
        scan_id,
        status: "scanning".to_owned(),
        targets: target_names,
        timeout_ms: u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX),
    }))
}
