//! Dynamic driver and device control-surface endpoints.
#![allow(dead_code)]

use hypercolor_types::controls::{
    ApplyControlChangesRequest, ApplyControlChangesResponse, ControlActionResult,
    ControlSurfaceDocument, ControlSurfaceId, ControlSurfaceRevision, ControlValueMap,
};
pub use hypercolor_ui::control_surface_api::ControlSurfaceListQuery;
use hypercolor_ui::control_surface_api::{
    control_surface_action_url, control_surface_list_url, control_surface_values_url, path_segment,
};
use serde::{Deserialize, Serialize};

use super::client;

/// Response from `GET /api/v1/control-surfaces`.
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct ControlSurfaceListResponse {
    pub surfaces: Vec<ControlSurfaceDocument>,
}

/// Request body for invoking a control-surface action.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct InvokeControlActionRequest {
    #[serde(default)]
    pub input: ControlValueMap,
}

/// Fetch surfaces selected by device, driver, or both.
pub async fn fetch_control_surfaces(
    query: ControlSurfaceListQuery<'_>,
) -> Result<Vec<ControlSurfaceDocument>, String> {
    let response: Option<ControlSurfaceListResponse> =
        client::fetch_json_optional(&control_surface_list_url(query)).await?;
    Ok(response
        .map(|response| response.surfaces)
        .unwrap_or_default())
}

/// Fetch device, driver-owned device, and optional driver-level surfaces.
pub async fn fetch_device_control_surfaces(
    device_id: &str,
    include_driver: bool,
) -> Result<Vec<ControlSurfaceDocument>, String> {
    fetch_control_surfaces(ControlSurfaceListQuery {
        device_id: Some(device_id),
        driver_id: None,
        include_driver,
    })
    .await
}

/// Fetch one control surface by stable surface ID.
pub async fn fetch_control_surface(surface_id: &str) -> Result<ControlSurfaceDocument, String> {
    client::fetch_json(&format!(
        "/api/v1/control-surfaces/{}",
        path_segment(surface_id)
    ))
    .await
    .map_err(Into::into)
}

/// Fetch one driver-level control surface.
pub async fn fetch_driver_control_surface(
    driver_id: &str,
) -> Result<ControlSurfaceDocument, String> {
    client::fetch_json(&format!(
        "/api/v1/drivers/{}/controls",
        path_segment(driver_id)
    ))
    .await
    .map_err(Into::into)
}

/// Fetch one device-level control surface.
pub async fn fetch_device_control_surface(
    device_id: &str,
) -> Result<ControlSurfaceDocument, String> {
    client::fetch_json(&format!(
        "/api/v1/devices/{}/controls",
        path_segment(device_id)
    ))
    .await
    .map_err(Into::into)
}

/// Apply typed control changes to a surface.
pub async fn apply_control_changes(
    request: &ApplyControlChangesRequest,
) -> Result<ApplyControlChangesResponse, String> {
    let url = control_surface_values_url(&request.surface_id);
    client::patch_json(&url, request).await.map_err(Into::into)
}

/// Apply typed field changes with an inline request body.
pub async fn patch_control_values(
    surface_id: ControlSurfaceId,
    expected_revision: Option<ControlSurfaceRevision>,
    changes: Vec<hypercolor_types::controls::ControlChange>,
    dry_run: bool,
) -> Result<ApplyControlChangesResponse, String> {
    apply_control_changes(&ApplyControlChangesRequest {
        surface_id,
        expected_revision,
        changes,
        dry_run,
    })
    .await
}

/// Invoke one typed control-surface action.
pub async fn invoke_control_action(
    surface_id: &str,
    action_id: &str,
    input: ControlValueMap,
) -> Result<ControlActionResult, String> {
    let request = InvokeControlActionRequest { input };
    client::post_json(&control_surface_action_url(surface_id, action_id), &request)
        .await
        .map_err(Into::into)
}
