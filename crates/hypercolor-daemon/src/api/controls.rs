//! Generic control-surface API endpoints.

use std::sync::Arc;

use crate::api::devices;
use crate::api::envelope;
use crate::app_state::AppState;
use crate::discovery as core_discovery;
use crate::domain::{DomainError, ResourceKind};
use crate::network;
use anyhow::bail;
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::response::{IntoResponse, Response};
use hypercolor_driver_api::{ControlApplyTarget, DriverConfigView, DriverHost, TrackedDeviceCtx};
use hypercolor_types::api::scene::PatchControlsRequest;
use hypercolor_types::config::{DriverConfigEntry, HypercolorConfig};
use hypercolor_types::controls::{
    AppliedControlChange, ApplyControlChangesResponse, ApplyImpact, ControlAccess,
    ControlActionDescriptor, ControlActionResult, ControlAvailability, ControlAvailabilityExpr,
    ControlAvailabilityState, ControlChange, ControlFieldDescriptor, ControlGroupDescriptor,
    ControlGroupKind, ControlObjectField, ControlOwner, ControlPersistence, ControlSurfaceDocument,
    ControlSurfaceEvent, ControlSurfaceScope, ControlValue, ControlValueMap, ControlValueType,
    ControlVisibility,
};
use hypercolor_types::device::{DeviceId, DeviceInfo, DeviceState, DeviceUserSettings};
use hypercolor_types::event::HypercolorEvent;

pub use hypercolor_types::api::controls::{
    ControlSurfaceListQuery, ControlSurfaceListResponse, InvokeControlActionRequest,
};

const DEVICE_FIELD_NAME: &str = "name";
const DEVICE_FIELD_ENABLED: &str = "enabled";
const DEVICE_FIELD_BRIGHTNESS: &str = "brightness";
const DEVICE_ACTION_IDENTIFY: &str = "identify";

/// `GET /api/v1/control-surfaces` - Return control surfaces for a UI view.
pub async fn list_control_surfaces(
    State(state): State<Arc<AppState>>,
    Query(query): Query<ControlSurfaceListQuery>,
) -> Response {
    let selected_surface = query.device_id.is_some() || query.driver_id.is_some();
    let mut surfaces = Vec::new();

    if let Some(device_id_or_name) = query.device_id.as_deref() {
        let device_id = match resolve_device_id(&state, device_id_or_name).await {
            Ok(Some(id)) => id,
            Ok(None) => {
                return DomainError::not_found(ResourceKind::Device, device_id_or_name)
                    .into_response();
            }
            Err(name) => {
                return DomainError::conflict(format!("Device name is ambiguous: {name}"))
                    .into_response();
            }
        };
        let Some(tracked) = state.device_registry.get(&device_id).await else {
            return DomainError::not_found(ResourceKind::Device, device_id).into_response();
        };

        surfaces.push(device_control_surface(
            &tracked.info,
            &tracked.user_settings,
            tracked.revision,
        ));
        match driver_device_control_surface(&state, &tracked.info, tracked.state).await {
            Ok(Some(surface)) => surfaces.push(surface),
            Ok(None) => {}
            Err(error) => return error.into_response(),
        }
        if query.include_driver.unwrap_or(false) {
            let driver_id = tracked.info.driver_id();
            match driver_control_surface_document(&state, driver_id).await {
                Ok(Some(surface)) => surfaces.push(surface),
                Ok(None) => {}
                Err(error) => return error.into_response(),
            }
        }
    }

    if let Some(driver_id) = query.driver_id.as_deref() {
        match driver_control_surface_document(&state, driver_id).await {
            Ok(Some(surface)) => surfaces.push(surface),
            Ok(None) => {}
            Err(error) => return error.into_response(),
        }
    }

    if !selected_surface {
        return DomainError::validation("Query must select at least one control surface")
            .into_response();
    }

    envelope::ok(ControlSurfaceListResponse { surfaces })
}

/// `GET /api/v1/drivers/{id}/controls` - Return a driver-level control surface.
pub async fn get_driver_control_surface(
    State(state): State<Arc<AppState>>,
    Path(driver_id): Path<String>,
) -> Response {
    match driver_control_surface_document(&state, &driver_id).await {
        Ok(Some(surface)) => envelope::ok(surface),
        Ok(None) => {
            DomainError::not_found(ResourceKind::ControlSurface, &driver_id).into_response()
        }
        Err(error) => error.into_response(),
    }
}

/// `GET /api/v1/control-surfaces/{id}` - Return one control surface by
/// its stable surface ID.
pub async fn get_control_surface(
    State(state): State<Arc<AppState>>,
    Path(surface_id): Path<String>,
) -> Response {
    if let Some((driver_id, device_id)) = parse_driver_device_surface_id(&surface_id) {
        let Some(tracked) = state.device_registry.get(&device_id).await else {
            return DomainError::not_found(ResourceKind::ControlSurface, &surface_id)
                .into_response();
        };
        if tracked.info.driver_id() != driver_id {
            return DomainError::not_found(ResourceKind::ControlSurface, &surface_id)
                .into_response();
        }
        return match driver_device_control_surface(&state, &tracked.info, tracked.state).await {
            Ok(Some(surface)) => envelope::ok(surface),
            Ok(None) => {
                DomainError::not_found(ResourceKind::ControlSurface, &surface_id).into_response()
            }
            Err(error) => error.into_response(),
        };
    }

    if let Some(driver_id) = parse_driver_surface_id(&surface_id) {
        return match driver_control_surface_document(&state, &driver_id).await {
            Ok(Some(surface)) => envelope::ok(surface),
            Ok(None) => {
                DomainError::not_found(ResourceKind::ControlSurface, &surface_id).into_response()
            }
            Err(error) => error.into_response(),
        };
    }

    if let Some(device_id) = parse_device_surface_id(&surface_id) {
        let Some(tracked) = state.device_registry.get(&device_id).await else {
            return DomainError::not_found(ResourceKind::ControlSurface, &surface_id)
                .into_response();
        };
        return envelope::ok(device_control_surface(
            &tracked.info,
            &tracked.user_settings,
            tracked.revision,
        ));
    }

    DomainError::not_found(ResourceKind::ControlSurface, &surface_id).into_response()
}

async fn driver_control_surface_document(
    state: &AppState,
    driver_id: &str,
) -> Result<Option<ControlSurfaceDocument>, DomainError> {
    let Some(driver) = state.driver_registry.get(&driver_id) else {
        return Err(DomainError::not_found(ResourceKind::Driver, driver_id));
    };
    let Some(provider) = driver.controls() else {
        return Ok(None);
    };

    let config_entry = state.config_manager.as_ref().map_or_else(
        || network::driver_config_entry(&HypercolorConfig::default(), driver_id),
        |manager| {
            let config = manager.get();
            network::driver_config_entry(&config, driver_id)
        },
    );
    let config_view = DriverConfigView {
        driver_id,
        entry: &config_entry,
    };

    match provider
        .driver_surface(state.driver_host.as_ref(), config_view)
        .await
    {
        Ok(surface) => Ok(surface.map(|mut surface| {
            surface.revision = driver_control_revision(&config_entry);
            surface
        })),
        Err(error) => Err(DomainError::Internal(anyhow::anyhow!(
            "Failed to build driver control surface for {driver_id}: {error}"
        ))),
    }
}

async fn driver_device_control_surface(
    state: &AppState,
    info: &DeviceInfo,
    current_state: DeviceState,
) -> Result<Option<ControlSurfaceDocument>, DomainError> {
    let driver_id = info.driver_id();
    let Some(driver) = state.driver_registry.get(driver_id) else {
        return Ok(None);
    };
    let Some(provider) = driver.controls() else {
        return Ok(None);
    };
    let metadata = state.device_registry.metadata_for_id(&info.id).await;
    let device = TrackedDeviceCtx {
        device_id: info.id,
        info,
        metadata: metadata.as_ref(),
        current_state: &current_state,
    };

    provider
        .device_surface(state.driver_host.as_ref(), &device)
        .await
        .map_err(|error| {
            DomainError::Internal(anyhow::anyhow!(
                "Failed to build device control surface for {}: {error}",
                info.id
            ))
        })
}

/// `GET /api/v1/devices/{id}/controls` — Return the generic device control
/// surface for a tracked device.
pub async fn get_device_control_surface(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Response {
    let device_id = match resolve_device_id(&state, &id).await {
        Ok(Some(id)) => id,
        Ok(None) => return DomainError::not_found(ResourceKind::Device, &id).into_response(),
        Err(name) => {
            return DomainError::conflict(format!("Device name is ambiguous: {name}"))
                .into_response();
        }
    };

    let Some(tracked) = state.device_registry.get(&device_id).await else {
        return DomainError::not_found(ResourceKind::Device, &id).into_response();
    };

    envelope::ok(device_control_surface(
        &tracked.info,
        &tracked.user_settings,
        tracked.revision,
    ))
}

/// `PATCH /api/v1/control-surfaces/{id}/values` — Apply typed control
/// values to a surface.
pub async fn apply_control_surface_values(
    State(state): State<Arc<AppState>>,
    Path(surface_id): Path<String>,
    Json(body): Json<PatchControlsRequest>,
) -> Response {
    if !body.clear_bindings.is_empty() {
        return DomainError::validation_field(
            "clear_bindings",
            "control surfaces do not support input bindings",
        )
        .into_response();
    }
    if body.values.is_empty() {
        return empty_control_values(&surface_id);
    }

    let mut changes = Vec::with_capacity(body.values.len());
    for (field_id, value) in body.values {
        let value = match value.to_driver_wire() {
            Ok(value) => value,
            Err(error) => {
                return DomainError::validation_field(
                    format!("values.{field_id}"),
                    error.to_string(),
                )
                .into_response();
            }
        };
        changes.push(ControlChange { field_id, value });
    }

    if let Some((driver_id, device_id)) = parse_driver_device_surface_id(&surface_id) {
        return apply_driver_device_control_surface_values(
            &state, surface_id, driver_id, device_id, changes,
        )
        .await;
    }

    if let Some(driver_id) = parse_driver_surface_id(&surface_id) {
        return apply_driver_control_surface_values(&state, surface_id, driver_id, changes).await;
    }

    let Some(device_id) = parse_device_surface_id(&surface_id) else {
        return DomainError::not_found(ResourceKind::ControlSurface, &surface_id).into_response();
    };

    let Some(tracked) = state.device_registry.get(&device_id).await else {
        return DomainError::not_found(ResourceKind::Device, device_id).into_response();
    };

    let previous_revision = tracked.revision;
    let normalized = match normalize_device_control_changes(&changes) {
        Ok(changes) => changes,
        Err(error) => return error.into_response(),
    };

    if let Err(error) = apply_device_control_changes(&state, device_id, &normalized).await {
        return error.into_response();
    }

    let Some(tracked) = state.device_registry.get(&device_id).await else {
        return DomainError::not_found(ResourceKind::Device, device_id).into_response();
    };
    let revision = tracked.revision;
    let document = device_control_surface(&tracked.info, &tracked.user_settings, revision);

    let response = ApplyControlChangesResponse {
        surface_id,
        previous_revision,
        revision,
        accepted: normalized
            .accepted
            .into_iter()
            .map(|change| AppliedControlChange {
                field_id: change.field_id,
                value: change.value,
            })
            .collect(),
        rejected: Vec::new(),
        impacts: normalized.impacts,
        values: document.values,
    };
    publish_values_changed(state.as_ref(), &response);
    envelope::ok(response)
}

/// `POST /api/v1/control-surfaces/{id}/actions/{action}` - Invoke a
/// typed control-surface action.
pub async fn invoke_control_surface_action(
    State(state): State<Arc<AppState>>,
    Path((surface_id, action_id)): Path<(String, String)>,
    Json(body): Json<InvokeControlActionRequest>,
) -> Response {
    if let Some((driver_id, device_id)) = parse_driver_device_surface_id(&surface_id) {
        return invoke_driver_device_control_action(
            &state, surface_id, driver_id, device_id, action_id, body,
        )
        .await;
    }

    if let Some(driver_id) = parse_driver_surface_id(&surface_id) {
        return invoke_driver_control_action(&state, surface_id, driver_id, action_id, body).await;
    }

    let Some(device_id) = parse_device_surface_id(&surface_id) else {
        return DomainError::not_found(ResourceKind::ControlSurface, &surface_id).into_response();
    };
    if action_id == DEVICE_ACTION_IDENTIFY {
        return invoke_host_device_control_action(state, surface_id, device_id, action_id, body)
            .await;
    }
    invoke_device_control_action(&state, surface_id, device_id, action_id, body).await
}

async fn invoke_host_device_control_action(
    state: Arc<AppState>,
    surface_id: String,
    device_id: DeviceId,
    action_id: String,
    body: InvokeControlActionRequest,
) -> Response {
    let Some(tracked) = state.device_registry.get(&device_id).await else {
        return DomainError::not_found(ResourceKind::Device, device_id).into_response();
    };
    let request = match host_identify_request(body.input) {
        Ok(request) => request,
        Err(error) => return error.into_response(),
    };
    let identify_response = devices::identify_device(
        State(Arc::clone(&state)),
        Path(device_id.to_string()),
        Some(Json(request)),
    )
    .await;
    if !identify_response.status().is_success() {
        return identify_response;
    }

    let result = ControlActionResult {
        surface_id,
        action_id,
        status: hypercolor_types::controls::ControlActionStatus::Accepted,
        result: None,
        revision: tracked.revision,
    };
    publish_action_progress(&state, &result);
    envelope::ok(result)
}

async fn invoke_driver_control_action(
    state: &AppState,
    surface_id: String,
    driver_id: String,
    action_id: String,
    body: InvokeControlActionRequest,
) -> Response {
    let Some(driver) = state.driver_registry.get(&driver_id) else {
        return DomainError::not_found(ResourceKind::Driver, &driver_id).into_response();
    };
    let Some(provider) = driver.controls() else {
        return DomainError::not_found(ResourceKind::ControlSurface, &driver_id).into_response();
    };

    let config_entry = driver_config_entry_for_state(state, &driver_id);
    let config_view = DriverConfigView {
        driver_id: &driver_id,
        entry: &config_entry,
    };
    let target = ControlApplyTarget::Driver {
        driver_id: &driver_id,
        config: config_view,
    };

    match provider
        .invoke_action(state.driver_host.as_ref(), &target, &action_id, body.input)
        .await
    {
        Ok(result) => {
            let result = normalize_action_result(
                result,
                surface_id,
                action_id,
                driver_control_revision(&driver_config_entry_for_state(state, &driver_id)),
            );
            publish_action_progress(state, &result);
            envelope::ok(result)
        }
        Err(error) => control_action_failed(&surface_id, &action_id, &error.to_string()),
    }
}

async fn invoke_device_control_action(
    state: &AppState,
    surface_id: String,
    device_id: DeviceId,
    action_id: String,
    body: InvokeControlActionRequest,
) -> Response {
    let Some(tracked) = state.device_registry.get(&device_id).await else {
        return DomainError::not_found(ResourceKind::Device, device_id).into_response();
    };
    let driver_id = tracked.info.driver_id();
    let Some(driver) = state.driver_registry.get(driver_id) else {
        return DomainError::not_found(ResourceKind::Driver, driver_id).into_response();
    };
    let Some(provider) = driver.controls() else {
        return DomainError::not_found(ResourceKind::ControlSurface, driver_id).into_response();
    };
    let metadata = state.device_registry.metadata_for_id(&device_id).await;
    let device = TrackedDeviceCtx {
        device_id,
        info: &tracked.info,
        metadata: metadata.as_ref(),
        current_state: &tracked.state,
    };
    let target = ControlApplyTarget::Device { device: &device };

    match provider
        .invoke_action(state.driver_host.as_ref(), &target, &action_id, body.input)
        .await
    {
        Ok(result) => {
            let result = normalize_action_result(result, surface_id, action_id, tracked.revision);
            publish_action_progress(state, &result);
            envelope::ok(result)
        }
        Err(error) => control_action_failed(&surface_id, &action_id, &error.to_string()),
    }
}

async fn invoke_driver_device_control_action(
    state: &AppState,
    surface_id: String,
    driver_id: String,
    device_id: DeviceId,
    action_id: String,
    body: InvokeControlActionRequest,
) -> Response {
    let Some(tracked) = state.device_registry.get(&device_id).await else {
        return DomainError::not_found(ResourceKind::Device, device_id).into_response();
    };
    let Some(driver) = state.driver_registry.get(&driver_id) else {
        return DomainError::not_found(ResourceKind::Driver, &driver_id).into_response();
    };
    let Some(provider) = driver.controls() else {
        return DomainError::not_found(ResourceKind::ControlSurface, &driver_id).into_response();
    };
    let metadata = state.device_registry.metadata_for_id(&device_id).await;
    let device = TrackedDeviceCtx {
        device_id,
        info: &tracked.info,
        metadata: metadata.as_ref(),
        current_state: &tracked.state,
    };
    let revision = match provider
        .device_surface(state.driver_host.as_ref(), &device)
        .await
    {
        Ok(Some(surface)) => surface.revision,
        Ok(None) => {
            return DomainError::not_found(ResourceKind::ControlSurface, &surface_id)
                .into_response();
        }
        Err(error) => {
            return DomainError::Internal(anyhow::anyhow!(
                "Failed to build device control surface for {device_id}: {error}"
            ))
            .into_response();
        }
    };
    let target = ControlApplyTarget::Device { device: &device };

    match provider
        .invoke_action(state.driver_host.as_ref(), &target, &action_id, body.input)
        .await
    {
        Ok(result) => {
            let result = normalize_action_result(result, surface_id, action_id, revision);
            publish_action_progress(state, &result);
            envelope::ok(result)
        }
        Err(error) => control_action_failed(&surface_id, &action_id, &error.to_string()),
    }
}

fn normalize_action_result(
    mut result: ControlActionResult,
    surface_id: String,
    action_id: String,
    revision: u64,
) -> ControlActionResult {
    result.surface_id = surface_id;
    result.action_id = action_id;
    result.revision = revision;
    result
}

fn publish_values_changed(state: &AppState, response: &ApplyControlChangesResponse) {
    state
        .event_bus
        .publish(HypercolorEvent::ControlSurfaceChanged(
            ControlSurfaceEvent::ValuesChanged {
                surface_id: response.surface_id.clone(),
                revision: response.revision,
                values: response.values.clone(),
            },
        ));
}

fn publish_action_progress(state: &AppState, result: &ControlActionResult) {
    state
        .event_bus
        .publish(HypercolorEvent::ControlSurfaceChanged(
            ControlSurfaceEvent::ActionProgress {
                surface_id: result.surface_id.clone(),
                action_id: result.action_id.clone(),
                status: result.status,
                progress: None,
            },
        ));
}

fn empty_control_values(surface_id: &str) -> Response {
    DomainError::validation_details(
        "At least one control value is required",
        serde_json::json!({
            "kind": "empty_control_values",
            "surface_id": surface_id,
        }),
    )
    .into_response()
}

fn control_action_failed(surface_id: &str, action_id: &str, detail: &str) -> Response {
    DomainError::validation_details(
        format!("Control action failed: {detail}"),
        serde_json::json!({
            "kind": "control_action_failed",
            "surface_id": surface_id,
            "action_id": action_id,
            "detail": detail,
        }),
    )
    .into_response()
}

fn driver_control_validation_failed(surface_id: &str, driver_id: &str, detail: &str) -> Response {
    DomainError::validation_details(
        format!("Invalid driver controls: {detail}"),
        serde_json::json!({
            "kind": "driver_control_validation_failed",
            "surface_id": surface_id,
            "driver_id": driver_id,
            "detail": detail,
        }),
    )
    .into_response()
}

fn driver_device_control_validation_failed(
    surface_id: &str,
    driver_id: &str,
    device_id: DeviceId,
    detail: &str,
) -> Response {
    DomainError::validation_details(
        format!("Invalid device controls: {detail}"),
        serde_json::json!({
            "kind": "driver_device_control_validation_failed",
            "surface_id": surface_id,
            "driver_id": driver_id,
            "device_id": device_id,
            "detail": detail,
        }),
    )
    .into_response()
}

fn host_identify_request(input: ControlValueMap) -> Result<devices::IdentifyRequest, DomainError> {
    let mut duration_ms = None;
    let mut color = None;

    for (field_id, value) in input {
        match (field_id.as_str(), value) {
            ("duration_ms", ControlValue::DurationMs(value)) => {
                duration_ms = Some(value);
            }
            ("color", ControlValue::ColorRgb([red, green, blue])) => {
                color = Some(format!("{red:02x}{green:02x}{blue:02x}"));
            }
            ("duration_ms", _) => {
                return Err(DomainError::validation(
                    "identify duration_ms must be a duration_ms value",
                ));
            }
            ("color", _) => {
                return Err(DomainError::validation(
                    "identify color must be a color_rgb value",
                ));
            }
            _ => {
                return Err(DomainError::validation(format!(
                    "Unknown identify action input: {field_id}"
                )));
            }
        }
    }

    Ok(devices::IdentifyRequest { duration_ms, color })
}

async fn apply_driver_control_surface_values(
    state: &AppState,
    surface_id: String,
    driver_id: String,
    changes: Vec<ControlChange>,
) -> Response {
    let Some(driver) = state.driver_registry.get(&driver_id) else {
        return DomainError::not_found(ResourceKind::Driver, &driver_id).into_response();
    };
    let Some(provider) = driver.controls() else {
        return DomainError::not_found(ResourceKind::ControlSurface, &driver_id).into_response();
    };

    if state.config_manager.is_none() {
        return DomainError::Internal(anyhow::anyhow!(
            "Config manager unavailable in this runtime"
        ))
        .into_response();
    }

    let config_entry = driver_config_entry_for_state(state, &driver_id);
    let previous_revision = driver_control_revision(&config_entry);
    let config_view = DriverConfigView {
        driver_id: &driver_id,
        entry: &config_entry,
    };
    let target = ControlApplyTarget::Driver {
        driver_id: &driver_id,
        config: config_view,
    };
    let validated = match provider
        .validate_changes(state.driver_host.as_ref(), &target, &changes)
        .await
    {
        Ok(changes) => changes,
        Err(error) => {
            return driver_control_validation_failed(&surface_id, &driver_id, &error.to_string());
        }
    };
    if let Err(error) = ensure_driver_level_impacts_supported(&driver_id, &validated.impacts) {
        return DomainError::Internal(anyhow::anyhow!(
            "Driver controls for {driver_id} returned unsupported impacts: {error}"
        ))
        .into_response();
    }

    let mut response = match provider
        .apply_changes(state.driver_host.as_ref(), &target, validated)
        .await
    {
        Ok(response) => response,
        Err(error) => {
            return DomainError::Internal(anyhow::anyhow!(
                "Failed to apply driver controls for {driver_id}: {error}"
            ))
            .into_response();
        }
    };
    let updated_entry = driver_config_entry_for_state(state, &driver_id);
    if let Err(error) = ensure_driver_level_impacts_supported(&driver_id, &response.impacts) {
        return DomainError::Internal(anyhow::anyhow!(
            "Driver controls for {driver_id} applied unsupported impacts: {error}"
        ))
        .into_response();
    }
    if let Err(error) = apply_driver_control_impacts(state, &driver_id, &response.impacts).await {
        return DomainError::Internal(anyhow::anyhow!(
            "Applied driver controls for {driver_id}, but dynamic impact handling failed: {error}"
        ))
        .into_response();
    }
    response.surface_id = surface_id;
    response.previous_revision = previous_revision;
    response.revision = driver_control_revision(&updated_entry);
    response.values = driver_surface_values(provider, state, &driver_id, &updated_entry).await;
    publish_values_changed(state, &response);
    envelope::ok(response)
}

async fn apply_driver_device_control_surface_values(
    state: &AppState,
    surface_id: String,
    driver_id: String,
    device_id: DeviceId,
    changes: Vec<ControlChange>,
) -> Response {
    let Some(tracked) = state.device_registry.get(&device_id).await else {
        return DomainError::not_found(ResourceKind::Device, device_id).into_response();
    };
    let Some(driver) = state.driver_registry.get(&driver_id) else {
        return DomainError::not_found(ResourceKind::Driver, &driver_id).into_response();
    };
    let Some(provider) = driver.controls() else {
        return DomainError::not_found(ResourceKind::ControlSurface, &driver_id).into_response();
    };
    let metadata = state.device_registry.metadata_for_id(&device_id).await;
    let device = TrackedDeviceCtx {
        device_id,
        info: &tracked.info,
        metadata: metadata.as_ref(),
        current_state: &tracked.state,
    };
    let surface = match provider
        .device_surface(state.driver_host.as_ref(), &device)
        .await
    {
        Ok(Some(surface)) => surface,
        Ok(None) => {
            return DomainError::not_found(ResourceKind::ControlSurface, &surface_id)
                .into_response();
        }
        Err(error) => {
            return DomainError::Internal(anyhow::anyhow!(
                "Failed to build device control surface for {device_id}: {error}"
            ))
            .into_response();
        }
    };
    let previous_revision = surface.revision;
    let target = ControlApplyTarget::Device { device: &device };
    let validated = match provider
        .validate_changes(state.driver_host.as_ref(), &target, &changes)
        .await
    {
        Ok(changes) => changes,
        Err(error) => {
            return driver_device_control_validation_failed(
                &surface_id,
                &driver_id,
                device_id,
                &error.to_string(),
            );
        }
    };
    if let Err(error) = ensure_driver_device_impacts_supported(&driver_id, &validated.impacts) {
        return DomainError::Internal(anyhow::anyhow!(
            "Driver controls for {surface_id} returned unsupported impacts: {error}"
        ))
        .into_response();
    }

    match provider
        .apply_changes(state.driver_host.as_ref(), &target, validated)
        .await
    {
        Ok(mut response) => {
            response.surface_id = surface_id.clone();
            let refreshed = provider
                .device_surface(state.driver_host.as_ref(), &device)
                .await
                .ok()
                .flatten();
            response.previous_revision = previous_revision;
            if let Some(surface) = refreshed {
                response.revision = surface.revision;
                response.values = surface.values;
            }
            if let Err(error) = apply_driver_device_control_impacts(
                state,
                &driver_id,
                device_id,
                tracked.info.output_backend_id(),
                &response.impacts,
            )
            .await
            {
                return DomainError::Internal(anyhow::anyhow!(
                    "Applied device controls for {surface_id}, but dynamic impact handling failed: {error}"
                ))
                .into_response();
            }
            publish_values_changed(state, &response);
            envelope::ok(response)
        }
        Err(error) => DomainError::Internal(anyhow::anyhow!(
            "Failed to apply device controls for {surface_id}: {error}"
        ))
        .into_response(),
    }
}

/// Build the host-owned base device control surface.
#[must_use]
pub(crate) fn device_control_surface(
    info: &DeviceInfo,
    user_settings: &DeviceUserSettings,
    revision: u64,
) -> ControlSurfaceDocument {
    let mut document = ControlSurfaceDocument::empty(
        format!("device:{}", info.id),
        ControlSurfaceScope::Device {
            device_id: info.id,
            driver_id: info.driver_id().to_owned(),
        },
    );
    document.revision = revision;
    document.groups.push(ControlGroupDescriptor {
        id: "general".to_owned(),
        label: "General".to_owned(),
        description: None,
        kind: ControlGroupKind::General,
        ordering: 0,
    });
    document.groups.push(ControlGroupDescriptor {
        id: "diagnostics".to_owned(),
        label: "Diagnostics".to_owned(),
        description: None,
        kind: ControlGroupKind::Diagnostics,
        ordering: 100,
    });

    document.fields.extend([
        host_field(
            DEVICE_FIELD_NAME,
            "Name",
            ControlValueType::String {
                min_len: Some(1),
                max_len: Some(80),
                pattern: None,
            },
            ControlPersistence::DeviceConfig,
            ApplyImpact::Live,
            0,
        ),
        host_field(
            DEVICE_FIELD_ENABLED,
            "Enabled",
            ControlValueType::Bool,
            ControlPersistence::DeviceConfig,
            ApplyImpact::DeviceReconnect,
            10,
        ),
        host_field(
            DEVICE_FIELD_BRIGHTNESS,
            "Brightness",
            ControlValueType::Float {
                min: Some(0.0),
                max: Some(1.0),
                step: Some(0.01),
            },
            ControlPersistence::DeviceConfig,
            ApplyImpact::Live,
            20,
        ),
    ]);
    document.actions.push(ControlActionDescriptor {
        id: DEVICE_ACTION_IDENTIFY.to_owned(),
        owner: ControlOwner::Host,
        group_id: Some("diagnostics".to_owned()),
        label: "Identify".to_owned(),
        description: Some("Flash this device so it can be found physically.".to_owned()),
        input_fields: vec![
            ControlObjectField {
                id: "duration_ms".to_owned(),
                label: "Duration".to_owned(),
                value_type: ControlValueType::DurationMs {
                    min: Some(1),
                    max: Some(120_000),
                    step: Some(100),
                },
                required: false,
                default_value: Some(ControlValue::DurationMs(3000)),
            },
            ControlObjectField {
                id: "color".to_owned(),
                label: "Color".to_owned(),
                value_type: ControlValueType::ColorRgb,
                required: false,
                default_value: None,
            },
        ],
        result_type: None,
        confirmation: None,
        apply_impact: ApplyImpact::Live,
        availability: ControlAvailabilityExpr::Always,
        ordering: 0,
    });

    document.values = ControlValueMap::from([
        (
            DEVICE_FIELD_NAME.to_owned(),
            ControlValue::String(
                user_settings
                    .name
                    .clone()
                    .unwrap_or_else(|| info.name.clone()),
            ),
        ),
        (
            DEVICE_FIELD_ENABLED.to_owned(),
            ControlValue::Bool(user_settings.enabled),
        ),
        (
            DEVICE_FIELD_BRIGHTNESS.to_owned(),
            ControlValue::Float(f64::from(user_settings.brightness.clamp(0.0, 1.0))),
        ),
    ]);
    document.availability = document
        .fields
        .iter()
        .map(|field| {
            (
                field.id.clone(),
                ControlAvailability {
                    state: ControlAvailabilityState::Available,
                    reason: None,
                },
            )
        })
        .collect();
    document.action_availability = document
        .actions
        .iter()
        .map(|action| {
            (
                action.id.clone(),
                ControlAvailability {
                    state: ControlAvailabilityState::Available,
                    reason: None,
                },
            )
        })
        .collect();
    document
}

fn host_field(
    id: &str,
    label: &str,
    value_type: ControlValueType,
    persistence: ControlPersistence,
    apply_impact: ApplyImpact,
    ordering: i32,
) -> ControlFieldDescriptor {
    ControlFieldDescriptor {
        id: id.to_owned(),
        owner: ControlOwner::Host,
        group_id: Some("general".to_owned()),
        label: label.to_owned(),
        description: None,
        value_type,
        default_value: None,
        access: ControlAccess::ReadWrite,
        persistence,
        apply_impact,
        visibility: ControlVisibility::Standard,
        availability: ControlAvailabilityExpr::Always,
        ordering,
    }
}

fn parse_device_surface_id(surface_id: &str) -> Option<DeviceId> {
    surface_id
        .strip_prefix("device:")
        .and_then(|id| id.parse::<DeviceId>().ok())
}

fn parse_driver_surface_id(surface_id: &str) -> Option<String> {
    surface_id
        .strip_prefix("driver:")
        .filter(|id| !id.is_empty())
        .map(ToOwned::to_owned)
}

fn parse_driver_device_surface_id(surface_id: &str) -> Option<(String, DeviceId)> {
    let (driver_id, device_id) = surface_id.strip_prefix("driver:")?.split_once(":device:")?;
    if driver_id.is_empty() {
        return None;
    }
    Some((driver_id.to_owned(), device_id.parse().ok()?))
}

fn driver_config_entry_for_state(state: &AppState, driver_id: &str) -> DriverConfigEntry {
    state.config_manager.as_ref().map_or_else(
        || network::driver_config_entry(&HypercolorConfig::default(), driver_id),
        |manager| {
            let config = manager.get();
            network::driver_config_entry(&config, driver_id)
        },
    )
}

fn driver_control_revision(entry: &DriverConfigEntry) -> u64 {
    let payload = serde_json::to_vec(entry).unwrap_or_default();
    payload.iter().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

async fn driver_surface_values(
    provider: &dyn hypercolor_driver_api::DriverControlProvider,
    state: &AppState,
    driver_id: &str,
    config_entry: &DriverConfigEntry,
) -> ControlValueMap {
    let config_view = DriverConfigView {
        driver_id,
        entry: config_entry,
    };
    provider
        .driver_surface(state.driver_host.as_ref(), config_view)
        .await
        .ok()
        .flatten()
        .map_or_else(ControlValueMap::new, |surface| surface.values)
}

async fn apply_driver_control_impacts(
    state: &AppState,
    driver_id: &str,
    impacts: &[ApplyImpact],
) -> anyhow::Result<()> {
    let Some(host) = state.driver_host.control_host() else {
        return Ok(());
    };
    for impact in impacts {
        match impact {
            ApplyImpact::None | ApplyImpact::Live => {}
            ApplyImpact::BackendRebind => host.backend_rebind().rebind_backend(driver_id).await?,
            ApplyImpact::DiscoveryRescan => host.lifecycle().rescan_driver(driver_id).await?,
            ApplyImpact::DeviceReconnect
            | ApplyImpact::TopologyRebuild
            | ApplyImpact::HardwarePersist
            | ApplyImpact::Custom(_) => {
                bail!("unsupported driver-level control impact for {driver_id}: {impact:?}")
            }
        }
    }
    Ok(())
}

async fn apply_driver_device_control_impacts(
    state: &AppState,
    driver_id: &str,
    device_id: DeviceId,
    backend_id: &str,
    impacts: &[ApplyImpact],
) -> anyhow::Result<()> {
    let Some(host) = state.driver_host.control_host() else {
        return Ok(());
    };
    for impact in impacts {
        match impact {
            ApplyImpact::None | ApplyImpact::Live => {}
            ApplyImpact::DeviceReconnect => {
                host.lifecycle()
                    .reconnect_device(device_id, backend_id)
                    .await?;
            }
            ApplyImpact::BackendRebind => host.backend_rebind().rebind_backend(driver_id).await?,
            ApplyImpact::DiscoveryRescan => host.lifecycle().rescan_driver(driver_id).await?,
            ApplyImpact::TopologyRebuild
            | ApplyImpact::HardwarePersist
            | ApplyImpact::Custom(_) => {
                bail!("unsupported device-level control impact for {driver_id}: {impact:?}")
            }
        }
    }
    Ok(())
}

fn ensure_driver_level_impacts_supported(
    driver_id: &str,
    impacts: &[ApplyImpact],
) -> anyhow::Result<()> {
    for impact in impacts {
        match impact {
            ApplyImpact::None
            | ApplyImpact::Live
            | ApplyImpact::BackendRebind
            | ApplyImpact::DiscoveryRescan => {}
            ApplyImpact::DeviceReconnect
            | ApplyImpact::TopologyRebuild
            | ApplyImpact::HardwarePersist
            | ApplyImpact::Custom(_) => {
                bail!("unsupported driver-level control impact for {driver_id}: {impact:?}")
            }
        }
    }
    Ok(())
}

fn ensure_driver_device_impacts_supported(
    driver_id: &str,
    impacts: &[ApplyImpact],
) -> anyhow::Result<()> {
    for impact in impacts {
        match impact {
            ApplyImpact::None
            | ApplyImpact::Live
            | ApplyImpact::DeviceReconnect
            | ApplyImpact::BackendRebind
            | ApplyImpact::DiscoveryRescan => {}
            ApplyImpact::TopologyRebuild
            | ApplyImpact::HardwarePersist
            | ApplyImpact::Custom(_) => {
                bail!("unsupported device-level control impact for {driver_id}: {impact:?}")
            }
        }
    }
    Ok(())
}

async fn resolve_device_id(state: &AppState, id_or_name: &str) -> Result<Option<DeviceId>, String> {
    if let Ok(id) = id_or_name.parse::<DeviceId>() {
        return Ok(Some(id));
    }

    let devices = state.device_registry.list().await;
    let matches: Vec<DeviceId> = devices
        .iter()
        .filter(|device| device.info.name.eq_ignore_ascii_case(id_or_name))
        .map(|device| device.info.id)
        .collect();

    if matches.len() > 1 {
        return Err(id_or_name.to_owned());
    }
    Ok(matches.first().copied())
}

struct NormalizedDeviceControlChanges {
    name: Option<String>,
    enabled: Option<bool>,
    brightness: Option<f32>,
    accepted: Vec<ControlChange>,
    impacts: Vec<ApplyImpact>,
}

fn normalize_device_control_changes(
    changes: &[ControlChange],
) -> Result<NormalizedDeviceControlChanges, DomainError> {
    let mut accepted = Vec::with_capacity(changes.len());
    let mut impacts = Vec::new();
    let mut name = None;
    let mut enabled = None;
    let mut brightness = None;

    for change in changes {
        match (change.field_id.as_str(), &change.value) {
            (DEVICE_FIELD_NAME, ControlValue::String(value)) => {
                let trimmed = value.trim();
                if trimmed.is_empty() {
                    return Err(invalid_control_value(
                        &change.field_id,
                        "Device name must not be empty",
                    ));
                }
                let value = trimmed.to_owned();
                name = Some(value.clone());
                accepted.push(ControlChange {
                    field_id: change.field_id.clone(),
                    value: ControlValue::String(value),
                });
                push_unique_impact(&mut impacts, ApplyImpact::Live);
            }
            (DEVICE_FIELD_ENABLED, ControlValue::Bool(value)) => {
                enabled = Some(*value);
                accepted.push(change.clone());
                push_unique_impact(&mut impacts, ApplyImpact::DeviceReconnect);
            }
            (DEVICE_FIELD_BRIGHTNESS, ControlValue::Float(value)) => {
                if !(0.0..=1.0).contains(value) {
                    return Err(out_of_range_control_value(
                        &change.field_id,
                        "Device brightness must be between 0.0 and 1.0",
                    ));
                }
                brightness = Some(*value as f32);
                accepted.push(change.clone());
                push_unique_impact(&mut impacts, ApplyImpact::Live);
            }
            (DEVICE_FIELD_NAME | DEVICE_FIELD_ENABLED | DEVICE_FIELD_BRIGHTNESS, _) => {
                return Err(type_mismatch_control_value(&change.field_id));
            }
            _ => {
                return Err(unknown_control_field(&change.field_id));
            }
        }
    }

    Ok(NormalizedDeviceControlChanges {
        name,
        enabled,
        brightness,
        accepted,
        impacts,
    })
}

fn unknown_control_field(field_id: &str) -> DomainError {
    DomainError::validation_details(
        format!("Unknown control field: {field_id}"),
        serde_json::json!({
            "kind": "unknown_control_field",
            "field_id": field_id,
        }),
    )
}

fn type_mismatch_control_value(field_id: &str) -> DomainError {
    DomainError::validation_details(
        format!("Invalid value type for control field: {field_id}"),
        serde_json::json!({
            "kind": "control_value_type_mismatch",
            "field_id": field_id,
        }),
    )
}

fn invalid_control_value(field_id: &str, message: &str) -> DomainError {
    DomainError::validation_details(
        message,
        serde_json::json!({
            "kind": "invalid_control_value",
            "field_id": field_id,
        }),
    )
}

fn out_of_range_control_value(field_id: &str, message: &str) -> DomainError {
    DomainError::validation_details(
        message,
        serde_json::json!({
            "kind": "control_value_out_of_range",
            "field_id": field_id,
        }),
    )
}

fn push_unique_impact(impacts: &mut Vec<ApplyImpact>, impact: ApplyImpact) {
    if !impacts.contains(&impact) {
        impacts.push(impact);
    }
}

async fn apply_device_control_changes(
    state: &AppState,
    device_id: DeviceId,
    changes: &NormalizedDeviceControlChanges,
) -> Result<(), DomainError> {
    let enabled_handled_by_lifecycle = if let Some(enabled) = changes.enabled {
        let runtime = super::discovery_runtime(state);
        match core_discovery::apply_user_enabled_state(&runtime, device_id, enabled).await {
            Ok(core_discovery::UserEnabledStateResult::Applied) => true,
            Ok(core_discovery::UserEnabledStateResult::MissingLifecycle) => false,
            Err(error) => {
                return Err(DomainError::Internal(anyhow::anyhow!(
                    "Failed to update device enabled state for {device_id}: {error}"
                )));
            }
        }
    } else {
        false
    };

    let Some(mut updated) = state
        .device_registry
        .update_user_settings(
            &device_id,
            changes.name.clone(),
            changes.enabled,
            changes.brightness,
        )
        .await
    else {
        return Err(DomainError::not_found(ResourceKind::Device, device_id));
    };

    if !enabled_handled_by_lifecycle && let Some(enabled) = changes.enabled {
        let fallback_state = if enabled {
            DeviceState::Known
        } else {
            DeviceState::Disabled
        };
        let _ = state
            .device_registry
            .set_state(&device_id, fallback_state)
            .await;
        if let Some(tracked) = state.device_registry.get(&device_id).await {
            updated = tracked;
        }
    }

    if let Err(error) =
        devices::persist_device_settings_for(state, device_id, &updated.user_settings).await
    {
        return Err(DomainError::Internal(anyhow::anyhow!(
            "Failed to persist device settings: {error}"
        )));
    }
    devices::sync_device_output_brightness(state, device_id, &updated.user_settings).await;
    devices::publish_device_settings_changed(state, device_id, &updated.user_settings);
    if changes.enabled == Some(true) {
        devices::activate_reenabled_layout_device(state, device_id, &updated.info).await;
    }
    Ok(())
}
