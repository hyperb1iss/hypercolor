//! Virtual display simulator endpoints — `/api/v1/simulators/*`.
//!
//! The Displays page uses these helpers to create daemon-owned virtual LCDs
//! without bouncing out to the preview helper scripts.

use serde::{Deserialize, Serialize};

use super::client;

/// Summary row from `GET /api/v1/simulators/displays`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SimulatedDisplaySummary {
    pub id: String,
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub circular: bool,
    pub enabled: bool,
}

/// Request body for `POST /api/v1/simulators/displays`.
#[derive(Debug, Clone, Serialize)]
pub struct CreateSimulatedDisplayRequest {
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub circular: bool,
    pub enabled: bool,
}

/// Request body for `PATCH /api/v1/simulators/displays/{id}`.
#[derive(Debug, Clone, Default, Serialize)]
pub struct UpdateSimulatedDisplayRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub circular: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
}

/// `GET /api/v1/simulators/displays` — list persisted virtual displays.
pub async fn fetch_simulated_displays() -> Result<Vec<SimulatedDisplaySummary>, String> {
    client::fetch_json::<Vec<SimulatedDisplaySummary>>("/api/v1/simulators/displays")
        .await
        .map_err(Into::into)
}

/// `POST /api/v1/simulators/displays` — create a new virtual display.
pub async fn create_simulated_display(
    body: &CreateSimulatedDisplayRequest,
) -> Result<SimulatedDisplaySummary, String> {
    client::post_json::<CreateSimulatedDisplayRequest, SimulatedDisplaySummary>(
        "/api/v1/simulators/displays",
        body,
    )
    .await
    .map_err(Into::into)
}

/// `PATCH /api/v1/simulators/displays/{id}` — update a virtual display.
pub async fn patch_simulated_display(
    id: &str,
    body: &UpdateSimulatedDisplayRequest,
) -> Result<SimulatedDisplaySummary, String> {
    let url = format!("/api/v1/simulators/displays/{id}");
    client::patch_json::<UpdateSimulatedDisplayRequest, SimulatedDisplaySummary>(&url, body)
        .await
        .map_err(Into::into)
}

/// `DELETE /api/v1/simulators/displays/{id}` — remove a virtual display.
pub async fn delete_simulated_display(id: &str) -> Result<(), String> {
    let url = format!("/api/v1/simulators/displays/{id}");
    client::delete_empty(&url).await.map_err(Into::into)
}
