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
use tracing::warn;

use hypercolor_types::canvas::SurfaceDescriptor;
use hypercolor_types::device::DeviceId;
use hypercolor_types::event::DisconnectReason;

use crate::api::AppState;
use crate::api::envelope::ApiResponse;
use crate::domain::{DomainError, ResourceKind};
use crate::logical_devices;
use crate::scene_transactions::{PreparedLayoutUpdate, apply_prepared_layout_update_under_guard};
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
        Err(error) => return error.into_response(),
    };

    let store = state.simulated_displays.read().await;
    match store.get(device_id) {
        Some(config) => ApiResponse::ok(config),
        None => DomainError::not_found(ResourceKind::SimulatedDisplay, device_id).into_response(),
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
    };

    if let Err(error) = validate_simulator_config(&config) {
        return DomainError::validation(error).into_response();
    }
    let config = config.normalized();

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
        return DomainError::Internal(anyhow::anyhow!(
            "Failed to activate simulated display: {error}"
        ))
        .into_response();
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
        Err(error) => return error.into_response(),
    };

    let updated = {
        let mut store = state.simulated_displays.write().await;
        let Some(existing) = store.get(device_id) else {
            return DomainError::not_found(ResourceKind::SimulatedDisplay, device_id)
                .into_response();
        };

        let updated = SimulatedDisplayConfig {
            id: existing.id,
            name: body.name.unwrap_or(existing.name),
            width: body.width.unwrap_or(existing.width),
            height: body.height.unwrap_or(existing.height),
            circular: body.circular.unwrap_or(existing.circular),
            enabled: body.enabled.unwrap_or(existing.enabled),
        };

        if let Err(error) = validate_simulator_config(&updated) {
            return DomainError::validation(error).into_response();
        }
        let updated = updated.normalized();

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
        return DomainError::Internal(anyhow::anyhow!(
            "Failed to refresh simulated display: {error}"
        ))
        .into_response();
    }

    ApiResponse::ok(updated)
}

pub async fn delete_simulated_display(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Response {
    let device_id = match parse_simulator_id(&id) {
        Ok(id) => id,
        Err(error) => return error.into_response(),
    };

    match tokio::spawn(delete_simulated_display_workflow(state, device_id)).await {
        Ok(response) => response,
        Err(error) => DomainError::Internal(anyhow::anyhow!(
            "Simulated display deletion workflow failed: {error}"
        ))
        .into_response(),
    }
}

async fn delete_simulated_display_workflow(state: Arc<AppState>, device_id: DeviceId) -> Response {
    let removed = {
        let mut store = state.simulated_displays.write().await;
        store.remove(device_id)
    };
    if removed.is_none() {
        return DomainError::not_found(ResourceKind::SimulatedDisplay, device_id).into_response();
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
        return DomainError::Internal(anyhow::anyhow!(
            "Failed to disconnect simulated display: {error}"
        ))
        .into_response();
    }

    {
        let mut store = state.logical_devices.write().await;
        store.retain(|_, entry| entry.physical_device_id != device_id);
        if let Err(error) = logical_devices::save_segments(&state.logical_devices_path, &store) {
            return DomainError::Internal(anyhow::anyhow!(
                "Failed to persist logical devices: {error}"
            ))
            .into_response();
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
    #[cfg(feature = "persistence-test-hooks")]
    state
        .layout_mutation_test_hooks
        .wait(
            crate::api::layouts::LayoutMutationTestPoint::AfterWorkflow,
            crate::api::layouts::LayoutMutationTestOperation::SimulatorPrune,
            &device_id.to_string(),
        )
        .await;
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
        Err(error) => return error.into_response(),
    };

    if state
        .simulated_displays
        .read()
        .await
        .get(device_id)
        .is_none()
    {
        return DomainError::not_found(ResourceKind::SimulatedDisplay, device_id).into_response();
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

    DomainError::not_found(ResourceKind::DisplayPreview, device_id).into_response()
}

fn jpeg_response(body: Bytes) -> Response {
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, HeaderValue::from_static("image/jpeg"))],
        body,
    )
        .into_response()
}

fn parse_simulator_id(raw: &str) -> Result<DeviceId, DomainError> {
    raw.parse::<DeviceId>()
        .map_err(|_| DomainError::validation(format!("Invalid simulator id: {raw}")))
}

fn validate_simulator_config(config: &SimulatedDisplayConfig) -> Result<(), String> {
    if config.name.trim().is_empty() {
        return Err("Simulator name must not be empty".to_owned());
    }
    SurfaceDescriptor::rgba8888(config.width, config.height)
        .try_non_empty_byte_len()
        .map_err(|error| format!("Simulator dimensions are invalid: {error}"))?;
    Ok(())
}

async fn prune_simulator_layout_targets(state: &Arc<AppState>, device_id: DeviceId) {
    let physical_id = device_id.to_string();
    #[cfg(feature = "persistence-test-hooks")]
    state
        .layout_mutation_test_hooks
        .wait(
            crate::api::layouts::LayoutMutationTestPoint::BeforeGuard,
            crate::api::layouts::LayoutMutationTestOperation::SimulatorPrune,
            &physical_id,
        )
        .await;
    let guard = state.scene_transactions.acquire_layout_update_guard().await;
    let mut target_ids: HashSet<String> =
        logical_device_ids_for_simulator(&state.logical_devices, device_id)
            .await
            .into_iter()
            .collect();
    target_ids.insert(physical_id.clone());
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

    #[cfg(feature = "persistence-test-hooks")]
    state
        .layout_mutation_test_hooks
        .wait(
            crate::api::layouts::LayoutMutationTestPoint::AfterMemoryMutation,
            crate::api::layouts::LayoutMutationTestOperation::SimulatorPrune,
            &physical_id,
        )
        .await;
    if let Some(layout) = active_layout {
        match PreparedLayoutUpdate::try_new(layout) {
            Ok(prepared) => {
                if let Err(error) = apply_prepared_layout_update_under_guard(
                    Arc::clone(&state.spatial_engine),
                    Arc::clone(&state.scene_manager),
                    state.scene_transactions.clone(),
                    &guard,
                    prepared,
                )
                .await
                {
                    warn!(%error, "rejected active layout after simulator pruning");
                }
            }
            Err(error) => warn!(%error, "rejected active layout after simulator pruning"),
        }
    }

    crate::api::persist_layouts_best_effort(state).await;
    drop(guard);
}
