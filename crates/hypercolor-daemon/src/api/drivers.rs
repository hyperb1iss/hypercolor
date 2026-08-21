//! Driver module endpoints — `/api/v1/drivers`.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::response::{IntoResponse, Response};
use hypercolor_types::api::drivers::{DriverConfigResponse, DriverListResponse, DriverSummary};
use hypercolor_types::config::HypercolorConfig;

use crate::api::envelope;
use crate::app_state::AppState;
use crate::domain::{DomainError, ResourceKind};
use crate::network;

/// `GET /api/v1/drivers` — List registered driver modules.
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

    envelope::ok(DriverListResponse { items })
}

/// `GET /api/v1/drivers/{id}/config` — Get one driver module's config entry.
pub async fn get_driver_config(
    State(state): State<Arc<AppState>>,
    Path(driver_id): Path<String>,
) -> Response {
    let Some(driver) = state.driver_registry.get(&driver_id) else {
        return DomainError::not_found(ResourceKind::Driver, &driver_id).into_response();
    };

    let config = state.config_manager.as_ref().map_or_else(
        || Arc::new(HypercolorConfig::default()),
        |manager| Arc::clone(&manager.get()),
    );
    let current = network::driver_config_entry(&config, &driver_id);
    let default = driver
        .config()
        .map(hypercolor_driver_api::DriverConfigProvider::default_config);

    envelope::ok(DriverConfigResponse {
        config_key: format!("drivers.{driver_id}"),
        configurable: default.is_some(),
        driver_id,
        current,
        default,
    })
}
