use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use axum::response::{IntoResponse, Response};
use hypercolor_types::api::devices::{DeleteDeviceResponse, ForgetDeviceRequest};
use hypercolor_types::device::DeviceId;

use crate::api::envelope;
use crate::app_state::AppState;
use crate::domain::DomainError;

/// Forget a controller addressed by its saved layout binding.
pub async fn forget_device(
    State(state): State<Arc<AppState>>,
    Json(body): Json<ForgetDeviceRequest>,
) -> Response {
    let target = body.layout_device_id.trim();
    if target.is_empty() {
        return DomainError::validation_field("layout_device_id", "must not be empty")
            .into_response();
    }
    let mut physical_id = state
        .logical_devices
        .read()
        .await
        .get(target)
        .map(|entry| entry.physical_device_id);
    if physical_id.is_none() {
        for tracked in state.device_registry.list().await {
            let binding = state
                .domains
                .layout
                .resolved_layout_device_id(&state.domains.devices.layout_runtime(), &tracked.info)
                .await;
            if binding == target {
                physical_id = Some(tracked.info.id);
                break;
            }
        }
    }
    if let Some(physical_id) = physical_id
        && state.device_registry.get(&physical_id).await.is_some()
    {
        return super::delete_device(State(state), Path(physical_id.to_string())).await;
    }
    match forget_saved_content(&state, target, physical_id).await {
        Ok(()) => envelope::ok(DeleteDeviceResponse {
            id: target.to_owned(),
            removed: true,
        }),
        Err(error) => error.into_response(),
    }
}

pub(super) async fn forget_saved_content(
    state: &Arc<AppState>,
    target: &str,
    physical_id: Option<DeviceId>,
) -> Result<(), DomainError> {
    crate::domain::device_removal::forget_saved_content(
        &crate::domain::device_removal::DeviceRemovalContext {
            scene: &state.domains.scene,
            logical_devices: &state.logical_devices,
            logical_devices_path: &state.logical_devices_path,
            attachment_profiles: &state.attachment_profiles,
            usb_protocol_configs: &state.usb_protocol_configs,
        },
        target,
        physical_id,
    )
    .await
}
