//! Driver module endpoints — `/api/v1/drivers`.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::response::Response;
use serde::Serialize;
use utoipa::ToSchema;

use hypercolor_types::config::DriverConfigEntry;
use hypercolor_types::config::HypercolorConfig;
use hypercolor_types::device::{
    DriverModuleDescriptor, DriverPresentation, DriverProtocolDescriptor,
};

use crate::api::AppState;
use crate::api::envelope::{ApiError, ApiResponse};
use crate::network;

#[derive(Debug, Serialize, ToSchema)]
pub struct DriverListResponse {
    pub items: Vec<DriverSummary>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct DriverSummary {
    pub descriptor: DriverModuleDescriptor,
    pub presentation: DriverPresentation,
    pub enabled: bool,
    pub config_key: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub protocols: Vec<DriverProtocolDescriptor>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub control_surface_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub control_surface_path: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct DriverConfigResponse {
    pub driver_id: String,
    pub config_key: String,
    pub configurable: bool,
    pub current: DriverConfigEntry,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<DriverConfigEntry>,
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

    let descriptors = network::module_descriptors(state.driver_registry.as_ref());

    let items = descriptors
        .into_iter()
        .map(|descriptor| {
            let enabled = network::module_enabled(&config, &descriptor);
            let config_key = format!("drivers.{}", descriptor.id);
            let control_surface_id = descriptor
                .capabilities
                .controls
                .then(|| format!("driver:{}", descriptor.id));
            let control_surface_path = descriptor
                .capabilities
                .controls
                .then(|| format!("/api/v1/drivers/{}/controls", descriptor.id));
            let protocols = if descriptor.capabilities.protocol_catalog {
                network::protocol_descriptors(state.driver_registry.as_ref(), &descriptor.id)
            } else {
                Vec::new()
            };

            DriverSummary {
                presentation: network::module_presentation(
                    state.driver_registry.as_ref(),
                    &descriptor.id,
                )
                .unwrap_or_else(|| network::descriptor_presentation(&descriptor)),
                descriptor,
                enabled,
                config_key,
                protocols,
                control_surface_id,
                control_surface_path,
            }
        })
        .collect();

    ApiResponse::ok(DriverListResponse { items })
}

/// `GET /api/v1/drivers/{id}/config` — Get one driver module's config entry.
#[utoipa::path(
    get,
    path = "/api/v1/drivers/{id}/config",
    params(
        ("id" = String, Path, description = "Driver module identifier")
    ),
    responses(
        (
            status = 200,
            description = "Driver module config",
            body = crate::api::envelope::ApiResponse<DriverConfigResponse>
        ),
        (
            status = 404,
            description = "Driver module not found",
            body = crate::api::envelope::ApiErrorResponse
        )
    ),
    tag = "drivers"
)]
pub async fn get_driver_config(
    State(state): State<Arc<AppState>>,
    Path(driver_id): Path<String>,
) -> Response {
    let Some(driver) = state.driver_registry.get(&driver_id) else {
        return ApiError::not_found(format!("Driver not found: {driver_id}"));
    };

    let config = state.config_manager.as_ref().map_or_else(
        || Arc::new(HypercolorConfig::default()),
        |manager| Arc::clone(&manager.get()),
    );
    let current = network::driver_config_entry(&config, &driver_id);
    let default = driver
        .config()
        .map(hypercolor_driver_api::DriverConfigProvider::default_config);

    ApiResponse::ok(DriverConfigResponse {
        config_key: format!("drivers.{driver_id}"),
        configurable: default.is_some(),
        driver_id,
        current,
        default,
    })
}
