//! Driver module endpoints — `/api/v1/drivers`.

use std::sync::Arc;

use axum::extract::State;
use axum::response::Response;
use serde::Serialize;
use utoipa::ToSchema;

use hypercolor_types::config::HypercolorConfig;
use hypercolor_types::device::{
    DriverModuleDescriptor, DriverPresentation, DriverProtocolDescriptor,
};

use crate::api::AppState;
use crate::api::envelope::ApiResponse;
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
