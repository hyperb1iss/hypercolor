//! Dynamic driver and device control-surface endpoints.
#![allow(dead_code)]

use std::collections::BTreeMap;

use super::{ApiResult, client};
pub use crate::control_surface_api::ControlSurfaceListQuery;
use crate::control_surface_api::{
    control_surface_action_url, control_surface_list_url, control_surface_values_url, path_segment,
};
pub use hypercolor_types::api::controls::{ControlSurfaceListResponse, InvokeControlActionRequest};
use hypercolor_types::api::scene::PatchControlsRequest;
use hypercolor_types::control::ControlValue;
use hypercolor_types::controls::{
    ApplyControlChangesResponse, ControlActionResult, ControlSurfaceDocument, ControlValueMap,
};

/// Fetch surfaces selected by device, driver, or both.
pub async fn fetch_control_surfaces(
    query: ControlSurfaceListQuery,
) -> ApiResult<Vec<ControlSurfaceDocument>> {
    let response: Option<ControlSurfaceListResponse> =
        client::fetch_json_optional(&control_surface_list_url(&query)).await?;
    Ok(response
        .map(|response| response.surfaces)
        .unwrap_or_default())
}

/// Fetch device, driver-owned device, and optional driver-level surfaces.
pub async fn fetch_device_control_surfaces(
    device_id: &str,
    include_driver: bool,
) -> ApiResult<Vec<ControlSurfaceDocument>> {
    fetch_control_surfaces(ControlSurfaceListQuery {
        device_id: Some(device_id.to_owned()),
        driver_id: None,
        include_driver: include_driver.then_some(true),
    })
    .await
}

/// Fetch one control surface by stable surface ID.
pub async fn fetch_control_surface(surface_id: &str) -> ApiResult<ControlSurfaceDocument> {
    client::fetch_json(&format!(
        "/api/v1/control-surfaces/{}",
        path_segment(surface_id)
    ))
    .await
}

/// Fetch one driver-level control surface.
pub async fn fetch_driver_control_surface(driver_id: &str) -> ApiResult<ControlSurfaceDocument> {
    client::fetch_json(&format!(
        "/api/v1/drivers/{}/controls",
        path_segment(driver_id)
    ))
    .await
}

/// Fetch one device-level control surface.
pub async fn fetch_device_control_surface(device_id: &str) -> ApiResult<ControlSurfaceDocument> {
    client::fetch_json(&format!(
        "/api/v1/devices/{}/controls",
        path_segment(device_id)
    ))
    .await
}

/// Patch typed field values on a surface.
pub async fn patch_control_values(
    surface_id: &str,
    values: BTreeMap<String, ControlValue>,
) -> ApiResult<ApplyControlChangesResponse> {
    let request = PatchControlsRequest {
        values,
        clear_bindings: Vec::new(),
    };
    client::patch_json(&control_surface_values_url(surface_id), &request).await
}

/// Invoke one typed control-surface action.
pub async fn invoke_control_action(
    surface_id: &str,
    action_id: &str,
    input: ControlValueMap,
) -> ApiResult<ControlActionResult> {
    let request = InvokeControlActionRequest { input };
    client::post_json(&control_surface_action_url(surface_id, action_id), &request).await
}
