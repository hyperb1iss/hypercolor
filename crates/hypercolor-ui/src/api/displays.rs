//! Display and overlay endpoints — `/api/v1/displays/*`.
//!
//! Covers display discovery, per-display overlay stack CRUD, per-slot runtime
//! diagnostics, and the preview JPEG URL. Overlay type definitions live in the
//! shared `hypercolor_types` crate so request/response bodies stay in lockstep
//! with the daemon.

use hypercolor_types::overlay::{
    DisplayOverlayConfig, OverlayBlendMode, OverlayPosition, OverlaySlot, OverlaySlotId,
    OverlaySource,
};
use serde::{Deserialize, Serialize};

use super::client::{self, ApiError};

// ── Types ───────────────────────────────────────────────────────────────────

/// Summary row from `GET /api/v1/displays`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DisplaySummary {
    pub id: String,
    pub name: String,
    pub vendor: String,
    pub family: String,
    pub width: u32,
    pub height: u32,
    pub circular: bool,
    pub overlay_count: usize,
    pub enabled_overlay_count: usize,
}

/// Runtime diagnostic envelope returned by the single-slot GET endpoint.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OverlayRuntimeResponse {
    pub last_rendered_at: Option<String>,
    pub consecutive_failures: u32,
    pub last_error: Option<String>,
    pub status: OverlaySlotStatus,
}

/// Runtime status of a single overlay slot, mirroring the daemon enum.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OverlaySlotStatus {
    Active,
    Disabled,
    Failed,
    HtmlGated,
}

impl OverlaySlotStatus {
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Self::Active => "Active",
            Self::Disabled => "Disabled",
            Self::Failed => "Failed",
            Self::HtmlGated => "HTML gated",
        }
    }
}

/// Response from `GET /api/v1/displays/{id}/overlays/{slot_id}`.
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct OverlaySlotResponse {
    pub slot: OverlaySlot,
    pub runtime: OverlayRuntimeResponse,
}

/// Entry from `GET /api/v1/displays/{id}/overlays/runtime` — a batched
/// per-slot runtime snapshot used to colour stack list rows without
/// issuing one request per slot.
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct OverlayRuntimeEntry {
    pub slot_id: OverlaySlotId,
    pub runtime: OverlayRuntimeResponse,
}

/// Request body for `POST /api/v1/displays/{id}/overlays`.
#[derive(Debug, Clone, Serialize)]
pub struct CreateOverlaySlotRequest {
    pub name: String,
    pub source: OverlaySource,
    pub position: OverlayPosition,
    #[serde(default)]
    pub blend_mode: OverlayBlendMode,
    pub opacity: f32,
    pub enabled: bool,
}

/// Request body for `PATCH /api/v1/displays/{id}/overlays/{slot_id}`.
///
/// All fields are optional; only set fields are updated. The daemon returns
/// the normalized slot on success.
#[derive(Debug, Clone, Default, Serialize)]
pub struct UpdateOverlaySlotRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<OverlaySource>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub position: Option<OverlayPosition>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blend_mode: Option<OverlayBlendMode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub opacity: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
}

/// Request body for `POST /api/v1/displays/{id}/overlays/reorder`.
#[derive(Debug, Clone, Serialize)]
pub struct ReorderOverlaySlotsRequest {
    pub slot_ids: Vec<OverlaySlotId>,
}

// ── Client functions ────────────────────────────────────────────────────────

/// `GET /api/v1/displays` — list displays with overlay capability.
pub async fn fetch_displays() -> Result<Vec<DisplaySummary>, String> {
    client::fetch_json::<Vec<DisplaySummary>>("/api/v1/displays")
        .await
        .map_err(Into::into)
}

/// `GET /api/v1/displays/{id}/overlays` — fetch the full overlay stack.
pub async fn fetch_display_overlays(display_id: &str) -> Result<DisplayOverlayConfig, String> {
    let url = format!("/api/v1/displays/{display_id}/overlays");
    client::fetch_json::<DisplayOverlayConfig>(&url)
        .await
        .map_err(Into::into)
}

/// `GET /api/v1/displays/{id}/overlays/{slot_id}` — fetch slot + runtime.
pub async fn fetch_overlay_slot(
    display_id: &str,
    slot_id: OverlaySlotId,
) -> Result<OverlaySlotResponse, String> {
    let url = format!("/api/v1/displays/{display_id}/overlays/{slot_id}");
    client::fetch_json::<OverlaySlotResponse>(&url)
        .await
        .map_err(Into::into)
}

/// `GET /api/v1/displays/{id}/overlays/runtime` — fetch every slot's
/// runtime state in one call.
pub async fn fetch_overlay_runtimes(
    display_id: &str,
) -> Result<Vec<OverlayRuntimeEntry>, String> {
    let url = format!("/api/v1/displays/{display_id}/overlays/runtime");
    client::fetch_json::<Vec<OverlayRuntimeEntry>>(&url)
        .await
        .map_err(Into::into)
}

/// `POST /api/v1/displays/{id}/overlays` — append a new slot.
pub async fn create_overlay_slot(
    display_id: &str,
    body: &CreateOverlaySlotRequest,
) -> Result<OverlaySlot, String> {
    let url = format!("/api/v1/displays/{display_id}/overlays");
    client::post_json::<CreateOverlaySlotRequest, OverlaySlot>(&url, body)
        .await
        .map_err(Into::into)
}

/// `PATCH /api/v1/displays/{id}/overlays/{slot_id}` — partial update.
pub async fn patch_overlay_slot(
    display_id: &str,
    slot_id: OverlaySlotId,
    body: &UpdateOverlaySlotRequest,
) -> Result<OverlaySlot, String> {
    let url = format!("/api/v1/displays/{display_id}/overlays/{slot_id}");
    client::patch_json::<UpdateOverlaySlotRequest, OverlaySlot>(&url, body)
        .await
        .map_err(Into::into)
}

/// `DELETE /api/v1/displays/{id}/overlays/{slot_id}` — remove a slot.
pub async fn delete_overlay_slot(display_id: &str, slot_id: OverlaySlotId) -> Result<(), String> {
    let url = format!("/api/v1/displays/{display_id}/overlays/{slot_id}");
    client::delete_empty(&url).await.map_err(Into::into)
}

/// `POST /api/v1/displays/{id}/overlays/reorder` — reorder the stack.
pub async fn reorder_overlay_slots(
    display_id: &str,
    slot_ids: &[OverlaySlotId],
) -> Result<DisplayOverlayConfig, String> {
    let url = format!("/api/v1/displays/{display_id}/overlays/reorder");
    let body = ReorderOverlaySlotsRequest {
        slot_ids: slot_ids.to_vec(),
    };
    client::post_json::<ReorderOverlaySlotsRequest, DisplayOverlayConfig>(&url, &body)
        .await
        .map_err(Into::into)
}

/// URL of the latest composited preview JPEG for a display.
///
/// The `cache_buster` query parameter is optional but gives the browser a
/// unique URL per poll cycle when conditional requests aren't enough.
#[must_use]
pub fn display_preview_url(display_id: &str, cache_buster: Option<u64>) -> String {
    cache_buster.map_or_else(
        || format!("/api/v1/displays/{display_id}/preview.jpg"),
        |cb| format!("/api/v1/displays/{display_id}/preview.jpg?ts={cb}"),
    )
}

impl From<ApiError> for ErrorNotice {
    fn from(err: ApiError) -> Self {
        Self(err.to_string())
    }
}

/// Lightweight error container used by UI code paths that want a typed wrapper.
#[derive(Debug, Clone)]
pub struct ErrorNotice(pub String);
