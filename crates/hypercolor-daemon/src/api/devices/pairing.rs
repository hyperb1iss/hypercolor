//! Pairing endpoints — `/api/v1/devices/{id}/pair`.

use std::collections::HashMap;
use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use axum::response::Response;
use serde::Serialize;
use tracing::warn;

use hypercolor_driver_api::{
    DeviceAuthState, DeviceAuthSummary, PairDeviceStatus as GenericPairDeviceStatus,
    TrackedDeviceCtx,
};
use hypercolor_types::device::{DeviceId, DeviceInfo, DeviceState};
use hypercolor_types::event::HypercolorEvent;

use crate::api::AppState;
use crate::api::envelope::{ApiError, ApiResponse};

use super::{DeviceSummary, refreshed_device_summary, resolve_device_id_or_response};

pub type GenericPairDeviceRequest = hypercolor_driver_api::PairDeviceRequest;

#[derive(Debug, Serialize)]
pub struct GenericPairDeviceResponse {
    pub status: GenericPairDeviceStatus,
    pub message: String,
    pub activated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device: Option<DeviceSummary>,
}

#[derive(Debug, Serialize)]
struct DeletePairingResponse {
    status: String,
    message: String,
    disconnected: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    device: Option<DeviceSummary>,
}

/// `POST /api/v1/devices/:id/pair` — pair a discovered network device.
pub async fn pair_device(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(payload): Json<GenericPairDeviceRequest>,
) -> Response {
    let device_id = match resolve_device_id_or_response(&state, &id).await {
        Ok(id) => id,
        Err(response) => return response,
    };

    match pair_device_for_ui(&state, device_id, payload).await {
        Ok(response) => ApiResponse::ok(response),
        Err(response) => response,
    }
}

/// `DELETE /api/v1/devices/:id/pair` — forget stored pairing credentials.
pub async fn delete_pairing(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Response {
    let device_id = match resolve_device_id_or_response(&state, &id).await {
        Ok(id) => id,
        Err(response) => return response,
    };

    match delete_device_pairing(&state, device_id).await {
        Ok(response) => ApiResponse::ok(response),
        Err(response) => response,
    }
}

pub(super) async fn build_device_auth_summary(
    state: &AppState,
    info: &DeviceInfo,
    device_state: &DeviceState,
    metadata: Option<&HashMap<String, String>>,
) -> Option<DeviceAuthSummary> {
    let driver_id = info.origin.driver_id.as_str();
    let driver = state.driver_registry.get(driver_id)?;
    let pairing = driver.pairing()?;
    let device = TrackedDeviceCtx {
        device_id: info.id,
        info,
        metadata,
        current_state: device_state,
    };

    pairing
        .auth_summary(state.driver_host.as_ref(), &device)
        .await
}

fn pairing_state_label(state: DeviceAuthState) -> &'static str {
    match state {
        DeviceAuthState::Open => "open",
        DeviceAuthState::Required => "required",
        DeviceAuthState::Configured => "configured",
        DeviceAuthState::Error => "error",
    }
}

fn publish_pairing_state_changed(
    state: &AppState,
    device_id: DeviceId,
    auth_state: DeviceAuthState,
    activated: bool,
) {
    let mut changes = HashMap::new();
    changes.insert(
        "auth_state".to_owned(),
        serde_json::json!(pairing_state_label(auth_state)),
    );
    changes.insert(
        "pairing_required".to_owned(),
        serde_json::json!(matches!(auth_state, DeviceAuthState::Required)),
    );
    changes.insert("activated".to_owned(), serde_json::json!(activated));
    state
        .event_bus
        .publish(HypercolorEvent::DeviceStateChanged {
            device_id: device_id.to_string(),
            changes,
        });
}

async fn pair_device_for_ui(
    state: &Arc<AppState>,
    device_id: DeviceId,
    request: GenericPairDeviceRequest,
) -> Result<GenericPairDeviceResponse, Response> {
    let Some(tracked) = state.device_registry.get(&device_id).await else {
        return Err(ApiError::not_found(format!(
            "Device not found: {device_id}"
        )));
    };
    let metadata = state.device_registry.metadata_for_id(&device_id).await;
    let driver_id = tracked.info.origin.driver_id.as_str();
    let Some(driver) = state.driver_registry.get(driver_id) else {
        return Err(ApiError::validation(format!(
            "Pairing is not supported for driver '{driver_id}'"
        )));
    };
    let Some(pairing) = driver.pairing() else {
        return Err(ApiError::validation(format!(
            "Pairing is not supported for driver '{driver_id}'"
        )));
    };
    let device = TrackedDeviceCtx {
        device_id,
        info: &tracked.info,
        metadata: metadata.as_ref(),
        current_state: &tracked.state,
    };
    let outcome = pairing
        .pair(state.driver_host.as_ref(), &device, &request)
        .await
        .map_err(|error| {
            warn!(
                error = %error,
                device_id = %device_id,
                driver_id = %driver_id,
                "device pairing request failed"
            );
            ApiError::internal(format!(
                "Failed to pair with {}",
                driver.descriptor().display_name
            ))
        })?;

    if matches!(outcome.status, GenericPairDeviceStatus::InvalidInput) {
        return Err(ApiError::validation(outcome.message));
    }

    publish_pairing_state_changed(
        state.as_ref(),
        device_id,
        outcome.auth_state,
        outcome.activated,
    );

    Ok(GenericPairDeviceResponse {
        status: outcome.status,
        message: outcome.message,
        activated: outcome.activated,
        device: refreshed_device_summary(state.as_ref(), device_id).await?,
    })
}

async fn delete_device_pairing(
    state: &Arc<AppState>,
    device_id: DeviceId,
) -> Result<DeletePairingResponse, Response> {
    let Some(tracked) = state.device_registry.get(&device_id).await else {
        return Err(ApiError::not_found(format!(
            "Device not found: {device_id}"
        )));
    };
    let metadata = state.device_registry.metadata_for_id(&device_id).await;
    let driver_id = tracked.info.origin.driver_id.as_str();
    let Some(driver) = state.driver_registry.get(driver_id) else {
        return Err(ApiError::validation(format!(
            "Pairing is not supported for driver '{driver_id}'"
        )));
    };
    let Some(pairing) = driver.pairing() else {
        return Err(ApiError::validation(format!(
            "Pairing is not supported for driver '{driver_id}'"
        )));
    };
    let device = TrackedDeviceCtx {
        device_id,
        info: &tracked.info,
        metadata: metadata.as_ref(),
        current_state: &tracked.state,
    };
    let outcome = pairing
        .clear_credentials(state.driver_host.as_ref(), &device)
        .await
        .map_err(|error| {
            warn!(
                error = %error,
                device_id = %device_id,
                driver_id = %driver_id,
                "failed to clear pairing credentials"
            );
            ApiError::internal(format!(
                "Failed to clear {} credentials",
                driver.descriptor().display_name
            ))
        })?;

    publish_pairing_state_changed(state.as_ref(), device_id, outcome.auth_state, false);

    Ok(DeletePairingResponse {
        status: "unpaired".to_owned(),
        message: outcome.message,
        disconnected: outcome.disconnected,
        device: refreshed_device_summary(state.as_ref(), device_id).await?,
    })
}
