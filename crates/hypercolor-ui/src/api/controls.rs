//! Dynamic driver and device control-surface endpoints.
#![allow(dead_code)]

use std::fmt::Write as _;

use hypercolor_types::controls::{
    ApplyControlChangesRequest, ApplyControlChangesResponse, ControlActionResult,
    ControlSurfaceDocument, ControlSurfaceId, ControlSurfaceRevision, ControlValueMap,
};
use serde::{Deserialize, Serialize};

use super::client;

/// Response from `GET /api/v1/control-surfaces`.
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct ControlSurfaceListResponse {
    pub surfaces: Vec<ControlSurfaceDocument>,
}

/// Query parameters for `GET /api/v1/control-surfaces`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ControlSurfaceListQuery<'a> {
    pub device_id: Option<&'a str>,
    pub driver_id: Option<&'a str>,
    pub include_driver: bool,
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

pub fn control_surface_list_url(query: ControlSurfaceListQuery<'_>) -> String {
    let mut parts = Vec::new();
    if let Some(device_id) = query.device_id {
        parts.push(format!("device_id={}", query_value(device_id)));
    }
    if let Some(driver_id) = query.driver_id {
        parts.push(format!("driver_id={}", query_value(driver_id)));
    }
    if query.include_driver {
        parts.push("include_driver=true".to_string());
    }

    if parts.is_empty() {
        "/api/v1/control-surfaces".to_string()
    } else {
        format!("/api/v1/control-surfaces?{}", parts.join("&"))
    }
}

pub fn control_surface_values_url(surface_id: &str) -> String {
    format!(
        "/api/v1/control-surfaces/{}/values",
        path_segment(surface_id)
    )
}

pub fn control_surface_action_url(surface_id: &str, action_id: &str) -> String {
    format!(
        "/api/v1/control-surfaces/{}/actions/{}",
        path_segment(surface_id),
        path_segment(action_id)
    )
}

fn path_segment(input: &str) -> String {
    percent_encode(input)
}

fn query_value(input: &str) -> String {
    percent_encode(input)
}

fn percent_encode(input: &str) -> String {
    let mut encoded = String::with_capacity(input.len());
    for byte in input.bytes() {
        let unreserved = byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~');
        if unreserved {
            encoded.push(char::from(byte));
        } else {
            let _ = write!(encoded, "%{byte:02X}");
        }
    }
    encoded
}
