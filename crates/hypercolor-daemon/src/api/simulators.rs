//! Virtual display simulator management endpoints — `/api/v1/simulators/*`.

use std::collections::HashSet;
use std::sync::Arc;

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::IntoResponse;
use axum::response::Response;
use serde::Deserialize;

use hypercolor_types::device::DeviceId;
use hypercolor_types::event::DisconnectReason;

use crate::api::AppState;
use crate::api::envelope::{ApiError, ApiResponse};
use crate::logical_devices;
use crate::scene_transactions::apply_layout_update;
use crate::simulators::{
    SimulatedDisplayConfig, activate_simulated_displays, logical_device_ids_for_simulator,
};

struct OwnedDisplayJpeg(Arc<Vec<u8>>);

impl AsRef<[u8]> for OwnedDisplayJpeg {
    fn as_ref(&self) -> &[u8] {
        self.0.as_ref().as_slice()
    }
}

#[derive(Debug, Deserialize)]
pub struct CreateSimulatedDisplayRequest {
    pub name: String,
    pub width: u32,
    pub height: u32,
    #[serde(default)]
    pub circular: bool,
    pub enabled: Option<bool>,
}

#[derive(Debug, Default, Deserialize)]
pub struct UpdateSimulatedDisplayRequest {
    pub name: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub circular: Option<bool>,
    pub enabled: Option<bool>,
}

pub async fn list_simulated_displays(State(state): State<Arc<AppState>>) -> Response {
    let store = state.simulated_displays.read().await;
    ApiResponse::ok(store.list())
}

pub async fn get_simulated_display(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Response {
    let device_id = match parse_simulator_id(&id) {
        Ok(id) => id,
        Err(response) => return response,
    };

    let store = state.simulated_displays.read().await;
    match store.get(device_id) {
        Some(config) => ApiResponse::ok(config),
        None => ApiError::not_found(format!("Simulated display not found: {device_id}")),
    }
}

pub async fn create_simulated_display(
    State(state): State<Arc<AppState>>,
    Json(body): Json<CreateSimulatedDisplayRequest>,
) -> Response {
    let config = SimulatedDisplayConfig {
        id: DeviceId::new(),
        name: body.name,
        width: body.width,
        height: body.height,
        circular: body.circular,
        enabled: body.enabled.unwrap_or(true),
    }
    .normalized();

    if let Err(error) = validate_simulator_config(&config) {
        return ApiError::validation(error);
    }

    {
        let mut store = state.simulated_displays.write().await;
        store.upsert(config.clone());
    }
    crate::api::persist_simulated_displays(&state).await;

    if let Err(error) = activate_simulated_displays(
        &state.driver_host.discovery_runtime(),
        &state.simulated_displays,
    )
    .await
    {
        return ApiError::internal(format!("Failed to activate simulated display: {error}"));
    }

    ApiResponse::created(config)
}

pub async fn patch_simulated_display(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<UpdateSimulatedDisplayRequest>,
) -> Response {
    let device_id = match parse_simulator_id(&id) {
        Ok(id) => id,
        Err(response) => return response,
    };

    let updated = {
        let mut store = state.simulated_displays.write().await;
        let Some(existing) = store.get(device_id) else {
            return ApiError::not_found(format!("Simulated display not found: {device_id}"));
        };

        let updated = SimulatedDisplayConfig {
            id: existing.id,
            name: body.name.unwrap_or(existing.name),
            width: body.width.unwrap_or(existing.width),
            height: body.height.unwrap_or(existing.height),
            circular: body.circular.unwrap_or(existing.circular),
            enabled: body.enabled.unwrap_or(existing.enabled),
        }
        .normalized();

        if let Err(error) = validate_simulator_config(&updated) {
            return ApiError::validation(error);
        }

        store.upsert(updated.clone());
        updated
    };
    crate::api::persist_simulated_displays(&state).await;

    if let Err(error) = activate_simulated_displays(
        &state.driver_host.discovery_runtime(),
        &state.simulated_displays,
    )
    .await
    {
        return ApiError::internal(format!("Failed to refresh simulated display: {error}"));
    }

    ApiResponse::ok(updated)
}

pub async fn delete_simulated_display(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Response {
    let device_id = match parse_simulator_id(&id) {
        Ok(id) => id,
        Err(response) => return response,
    };

    let removed = {
        let mut store = state.simulated_displays.write().await;
        store.remove(device_id)
    };
    if removed.is_none() {
        return ApiError::not_found(format!("Simulated display not found: {device_id}"));
    }
    crate::api::persist_simulated_displays(&state).await;

    prune_simulator_layout_targets(&state, device_id).await;

    let runtime = state.driver_host.discovery_runtime();
    if let Err(error) = crate::discovery::disconnect_tracked_device(
        &runtime,
        device_id,
        DisconnectReason::User,
        false,
    )
    .await
    {
        return ApiError::internal(format!("Failed to disconnect simulated display: {error}"));
    }

    {
        let mut store = state.logical_devices.write().await;
        store.retain(|_, entry| entry.physical_device_id != device_id);
        if let Err(error) = logical_devices::save_segments(&state.logical_devices_path, &store) {
            return ApiError::internal(format!("Failed to persist logical devices: {error}"));
        }
    }

    let _ = state.device_registry.remove(&device_id).await;
    state
        .simulated_display_runtime
        .write()
        .await
        .remove(device_id);
    state.display_frames.write().await.remove(device_id);
    crate::api::prune_scene_display_groups_for_device(&state, device_id).await;
    ApiResponse::ok(serde_json::json!({
        "id": device_id,
        "deleted": true,
    }))
}

pub async fn get_simulated_display_frame(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Response {
    let device_id = match parse_simulator_id(&id) {
        Ok(id) => id,
        Err(response) => return response,
    };

    if state
        .simulated_displays
        .read()
        .await
        .get(device_id)
        .is_none()
    {
        return ApiError::not_found(format!("Simulated display not found: {device_id}"));
    }

    if let Some(frame) = state
        .simulated_display_runtime
        .read()
        .await
        .frame(device_id)
    {
        return jpeg_response(Bytes::from_owner(OwnedDisplayJpeg(frame.jpeg_data)));
    }

    if let Some(frame) = state.display_frames.read().await.frame(device_id) {
        return jpeg_response(Bytes::from_owner(OwnedDisplayJpeg(Arc::clone(
            &frame.jpeg_data,
        ))));
    }

    ApiError::not_found(format!(
        "Simulated display frame not available: {device_id}"
    ))
}

fn jpeg_response(body: Bytes) -> Response {
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, HeaderValue::from_static("image/jpeg"))],
        body,
    )
        .into_response()
}

#[allow(
    clippy::result_large_err,
    reason = "Axum handlers already return Response values directly, so this helper keeps the hot path linear"
)]
fn parse_simulator_id(raw: &str) -> Result<DeviceId, Response> {
    raw.parse::<DeviceId>()
        .map_err(|_| ApiError::validation(format!("Invalid simulator id: {raw}")))
}

fn validate_simulator_config(config: &SimulatedDisplayConfig) -> Result<(), String> {
    if config.name.trim().is_empty() {
        return Err("Simulator name must not be empty".to_owned());
    }
    if config.width == 0 || config.height == 0 {
        return Err("Simulator width and height must be greater than zero".to_owned());
    }
    if config.width > 4096 || config.height > 4096 {
        return Err("Simulator width and height must be 4096 or less".to_owned());
    }
    Ok(())
}

async fn prune_simulator_layout_targets(state: &Arc<AppState>, device_id: DeviceId) {
    let physical_id = device_id.to_string();
    let mut target_ids: HashSet<String> =
        logical_device_ids_for_simulator(&state.logical_devices, device_id)
            .await
            .into_iter()
            .collect();
    target_ids.insert(physical_id.clone());
    target_ids.insert(format!("device:{physical_id}"));
    target_ids.insert(format!("simulator:{physical_id}"));

    let active_layout_id = {
        let spatial = state.spatial_engine.read().await;
        spatial.layout().id.clone()
    };

    let active_layout = {
        let mut layouts = state.layouts.write().await;
        let mut updated_active = None;

        for layout in layouts.values_mut() {
            let zone_count = layout.zones.len();
            layout
                .zones
                .retain(|zone| !target_ids.contains(zone.device_id.as_str()));
            if layout.zones.len() != zone_count && layout.id == active_layout_id {
                updated_active = Some(layout.clone());
            }
        }

        updated_active
    };

    if let Some(layout) = active_layout {
        apply_layout_update(
            &state.spatial_engine,
            &state.scene_manager,
            &state.scene_transactions,
            layout,
        )
        .await;
    }

    crate::api::persist_layouts(state).await;
}
