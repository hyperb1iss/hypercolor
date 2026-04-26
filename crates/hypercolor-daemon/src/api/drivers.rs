//! Driver module endpoints — `/api/v1/drivers`.

use std::sync::Arc;

use axum::extract::State;
use axum::response::Response;
use serde::Serialize;
use utoipa::ToSchema;

use hypercolor_types::config::HypercolorConfig;
use hypercolor_types::device::DriverModuleDescriptor;

use crate::api::AppState;
use crate::api::envelope::ApiResponse;

#[derive(Debug, Serialize, ToSchema)]
pub struct DriverListResponse {
    pub items: Vec<DriverSummary>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct DriverSummary {
    pub descriptor: DriverModuleDescriptor,
    pub enabled: bool,
    pub config_key: String,
}

/// `GET /api/v1/drivers` — List registered driver modules.
#[utoipa::path(
    get,
    path = "/api/v1/drivers",
    responses(
        (
            status = 200,
            description = "Registered driver modules",
            body = crate::api::envelope::ApiResponse<DriverListResponse>
        )
    ),
    tag = "drivers"
)]
pub async fn list_drivers(State(state): State<Arc<AppState>>) -> Response {
    let config = state.config_manager.as_ref().map_or_else(
        || Arc::new(HypercolorConfig::default()),
        |manager| Arc::clone(&manager.get()),
    );

    let items = state
        .driver_registry
        .module_descriptors()
        .into_iter()
        .map(|descriptor| {
            let enabled = config
                .drivers
                .get(&descriptor.id)
                .map_or(descriptor.default_enabled, |entry| entry.enabled);
            let config_key = format!("drivers.{}", descriptor.id);

            DriverSummary {
                descriptor,
                enabled,
                config_key,
            }
        })
        .collect();

    ApiResponse::ok(DriverListResponse { items })
}
