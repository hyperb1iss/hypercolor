//! Generic control-surface API endpoints.

use std::collections::BTreeSet;
use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use axum::response::Response;
use hypercolor_types::controls::{
    AppliedControlChange, ApplyControlChangesRequest, ApplyControlChangesResponse, ApplyImpact,
    ControlAccess, ControlAvailability, ControlAvailabilityExpr, ControlAvailabilityState,
    ControlChange, ControlFieldDescriptor, ControlGroupDescriptor, ControlGroupKind, ControlOwner,
    ControlPersistence, ControlSurfaceDocument, ControlSurfaceScope, ControlValue, ControlValueMap,
    ControlValueType, ControlVisibility,
};
use hypercolor_types::device::{DeviceId, DeviceInfo, DeviceState, DeviceUserSettings};

use crate::api::AppState;
use crate::api::devices;
use crate::api::envelope::{ApiError, ApiResponse};
use crate::discovery as core_discovery;

const DEVICE_FIELD_NAME: &str = "name";
const DEVICE_FIELD_ENABLED: &str = "enabled";
const DEVICE_FIELD_BRIGHTNESS: &str = "brightness";
type ControlApiResult<T> = Result<T, Box<Response>>;

/// `GET /api/v1/devices/:id/controls` — Return the generic device control
/// surface for a tracked device.
pub async fn get_device_control_surface(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Response {
    let device_id = match resolve_device_id(&state, &id).await {
        Ok(Some(id)) => id,
        Ok(None) => return ApiError::not_found(format!("Device not found: {id}")),
        Err(name) => return ApiError::conflict(format!("Device name is ambiguous: {name}")),
    };

    let Some(tracked) = state.device_registry.get(&device_id).await else {
        return ApiError::not_found(format!("Device not found: {id}"));
    };

    ApiResponse::ok(device_control_surface(
        &tracked.info,
        &tracked.user_settings,
        state.device_registry.generation(),
    ))
}

/// `PATCH /api/v1/control-surfaces/:surface_id/values` — Apply typed control
/// values to a surface.
pub async fn apply_control_surface_values(
    State(state): State<Arc<AppState>>,
    Path(surface_id): Path<String>,
    Json(body): Json<ApplyControlChangesRequest>,
) -> Response {
    if body.surface_id != surface_id {
        return ApiError::validation("Request surface_id must match the route surface id");
    }
    if body.changes.is_empty() {
        return ApiError::validation("At least one control change is required");
    }

    let Some(device_id) = parse_device_surface_id(&surface_id) else {
        return ApiError::not_found(format!("Unknown control surface: {surface_id}"));
    };

    let previous_revision = state.device_registry.generation();
    if let Some(expected) = body.expected_revision
        && expected != previous_revision
    {
        return ApiError::conflict(format!(
            "Control surface revision conflict: expected {expected}, current {previous_revision}"
        ));
    }

    let Some(tracked) = state.device_registry.get(&device_id).await else {
        return ApiError::not_found(format!("Device not found: {device_id}"));
    };

    let normalized = match normalize_device_control_changes(&body.changes) {
        Ok(changes) => changes,
        Err(response) => return *response,
    };

    if !body.dry_run
        && let Err(response) = apply_device_control_changes(&state, device_id, &normalized).await
    {
        return *response;
    }

    let tracked = if body.dry_run {
        tracked
    } else {
        match state.device_registry.get(&device_id).await {
            Some(tracked) => tracked,
            None => return ApiError::not_found(format!("Device not found: {device_id}")),
        }
    };
    let revision = if body.dry_run {
        previous_revision
    } else {
        state.device_registry.generation()
    };
    let document = device_control_surface(&tracked.info, &tracked.user_settings, revision);

    ApiResponse::ok(ApplyControlChangesResponse {
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
    })
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
            driver_id: info.origin.driver_id.clone(),
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
) -> ControlApiResult<NormalizedDeviceControlChanges> {
    let mut seen = BTreeSet::new();
    let mut accepted = Vec::with_capacity(changes.len());
    let mut impacts = Vec::new();
    let mut name = None;
    let mut enabled = None;
    let mut brightness = None;

    for change in changes {
        if !seen.insert(change.field_id.as_str()) {
            return Err(Box::new(ApiError::validation(format!(
                "Duplicate control field: {}",
                change.field_id
            ))));
        }

        match (change.field_id.as_str(), &change.value) {
            (DEVICE_FIELD_NAME, ControlValue::String(value)) => {
                let trimmed = value.trim();
                if trimmed.is_empty() {
                    return Err(Box::new(ApiError::validation(
                        "Device name must not be empty",
                    )));
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
                    return Err(Box::new(ApiError::validation(
                        "Device brightness must be between 0.0 and 1.0",
                    )));
                }
                brightness = Some(*value as f32);
                accepted.push(change.clone());
                push_unique_impact(&mut impacts, ApplyImpact::Live);
            }
            (DEVICE_FIELD_NAME | DEVICE_FIELD_ENABLED | DEVICE_FIELD_BRIGHTNESS, _) => {
                return Err(Box::new(ApiError::validation(format!(
                    "Invalid value type for control field: {}",
                    change.field_id
                ))));
            }
            _ => {
                return Err(Box::new(ApiError::validation(format!(
                    "Unknown control field: {}",
                    change.field_id
                ))));
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

fn push_unique_impact(impacts: &mut Vec<ApplyImpact>, impact: ApplyImpact) {
    if !impacts.contains(&impact) {
        impacts.push(impact);
    }
}

async fn apply_device_control_changes(
    state: &AppState,
    device_id: DeviceId,
    changes: &NormalizedDeviceControlChanges,
) -> ControlApiResult<()> {
    let enabled_handled_by_lifecycle = if let Some(enabled) = changes.enabled {
        let runtime = super::discovery_runtime(state);
        match core_discovery::apply_user_enabled_state(&runtime, device_id, enabled).await {
            Ok(core_discovery::UserEnabledStateResult::Applied) => true,
            Ok(core_discovery::UserEnabledStateResult::MissingLifecycle) => false,
            Err(error) => {
                return Err(Box::new(ApiError::internal(format!(
                    "Failed to update device enabled state for {device_id}: {error}"
                ))));
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
        return Err(Box::new(ApiError::not_found(format!(
            "Device not found: {device_id}"
        ))));
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
        return Err(Box::new(ApiError::internal(format!(
            "Failed to persist device settings: {error}"
        ))));
    }
    devices::sync_device_output_brightness(state, device_id, &updated.user_settings).await;
    devices::publish_device_settings_changed(state, device_id, &updated.user_settings);
    Ok(())
}
