//! Effect-related API types and fetch functions.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use gloo_net::http::Request;
use hypercolor_types::effect::{ControlDefinition, ControlValue, PresetTemplate};
use web_sys::{File, FormData};

use super::client;

// ── Types ───────────────────────────────────────────────────────────────────

/// Effect list item from `GET /api/v1/effects`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EffectSummary {
    pub id: String,
    pub name: String,
    pub description: String,
    pub author: String,
    pub category: String,
    pub source: String,
    pub runnable: bool,
    pub tags: Vec<String>,
    pub version: String,
    #[serde(default)]
    pub audio_reactive: bool,
}

/// Paginated effect list response.
#[derive(Debug, Deserialize)]
pub struct EffectListResponse {
    pub items: Vec<EffectSummary>,
}

/// Active effect response from `GET /api/v1/effects/active`.
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct ActiveEffectResponse {
    pub id: String,
    pub name: String,
    pub state: String,
    #[serde(default)]
    pub controls: Vec<ControlDefinition>,
    #[serde(default)]
    pub control_values: HashMap<String, ControlValue>,
    #[serde(default)]
    pub active_preset_id: Option<String>,
    #[serde(default)]
    pub render_group_id: Option<String>,
    /// Server-side controls version (matches the `ETag` header).
    /// `Some` while an effect is running, `None` on the idle response.
    /// Clients that want optimistic concurrency echo this back via
    /// `If-Match` on the effect-id PATCH endpoint.
    #[serde(default)]
    pub controls_version: Option<u64>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
struct WireActiveEffectResponse {
    pub id: Option<String>,
    pub name: Option<String>,
    pub state: String,
    #[serde(default)]
    pub controls: Vec<ControlDefinition>,
    #[serde(default)]
    pub control_values: HashMap<String, ControlValue>,
    #[serde(default)]
    pub active_preset_id: Option<String>,
    #[serde(default)]
    pub render_group_id: Option<String>,
    #[serde(default)]
    pub controls_version: Option<u64>,
}

/// Detailed effect payload from `GET /api/v1/effects/:id`.
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct EffectDetailResponse {
    pub id: String,
    pub name: String,
    pub description: String,
    pub author: String,
    pub category: String,
    pub source: String,
    pub runnable: bool,
    pub tags: Vec<String>,
    pub version: String,
    pub audio_reactive: bool,
    #[serde(default)]
    pub controls: Vec<ControlDefinition>,
    #[serde(default)]
    pub presets: Vec<PresetTemplate>,
    #[serde(default)]
    pub active_control_values: Option<HashMap<String, ControlValue>>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct InstalledEffectResponse {
    pub id: String,
    pub name: String,
    pub source: String,
    pub path: String,
    pub controls: usize,
    pub presets: usize,
}

// ── Fetch Functions ─────────────────────────────────────────────────────────

/// Fetch all registered effects.
pub async fn fetch_effects() -> Result<Vec<EffectSummary>, String> {
    let list: EffectListResponse = client::fetch_json("/api/v1/effects").await?;
    Ok(list.items)
}

/// Fetch effects filtered to a single category.
///
/// The daemon's `/api/v1/effects` endpoint doesn't currently honor a
/// `category` query parameter, so we filter client-side after fetching
/// the full catalog. Kept as a separate function so callers have a
/// single clear entry point and we can move filtering server-side later
/// without touching call sites.
pub async fn fetch_effects_by_category(category: &str) -> Result<Vec<EffectSummary>, String> {
    let list: EffectListResponse = client::fetch_json("/api/v1/effects").await?;
    Ok(list
        .items
        .into_iter()
        .filter(|effect| effect.category.eq_ignore_ascii_case(category))
        .collect())
}

/// Fetch the currently active effect, if any.
pub async fn fetch_active_effect() -> Result<Option<ActiveEffectResponse>, String> {
    let active = client::fetch_json::<Option<WireActiveEffectResponse>>("/api/v1/effects/active")
        .await
        .map_err(|error| error.to_string())?;
    Ok(active.and_then(|effect| {
        if effect.state == "idle" {
            return None;
        }
        Some(ActiveEffectResponse {
            id: effect.id?,
            name: effect.name?,
            state: effect.state,
            controls: effect.controls,
            control_values: effect.control_values,
            active_preset_id: effect.active_preset_id,
            render_group_id: effect.render_group_id,
            controls_version: effect.controls_version,
        })
    }))
}

/// Fetch detailed metadata for one effect.
pub async fn fetch_effect_detail(id: &str) -> Result<EffectDetailResponse, String> {
    client::fetch_json(&format!("/api/v1/effects/{id}"))
        .await
        .map_err(Into::into)
}

/// Fetch the bundled (effect-defined) presets for an effect.
pub async fn fetch_bundled_presets(id: &str) -> Result<Vec<PresetTemplate>, String> {
    let detail = fetch_effect_detail(id).await?;
    Ok(detail.presets)
}

/// Optional body for `apply_effect` — lets callers bake a remembered
/// preset selection and control values into the initial apply request
/// so the effect starts in its final state without a follow-up
/// round-trip (which caused a brief defaults-flash).
#[derive(Debug, Default, Serialize)]
pub struct ApplyEffectBody {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preset_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub controls: Option<serde_json::Value>,
}

/// Apply an effect by ID or name. Pass `None` for a bare start; pass
/// `Some(body)` to deliver preferences atomically.
pub async fn apply_effect(id: &str, body: Option<&ApplyEffectBody>) -> Result<(), String> {
    let path = format!("/api/v1/effects/{id}/apply");
    match body {
        Some(body) => client::post_json_discard(&path, body).await.map_err(Into::into),
        None => client::post_empty(&path).await.map_err(Into::into),
    }
}

/// Stop the currently active effect.
pub async fn stop_effect() -> Result<(), String> {
    client::post_empty("/api/v1/effects/stop")
        .await
        .map_err(Into::into)
}

/// Update effect control parameters.
pub async fn update_controls(controls: &serde_json::Value) -> Result<(), String> {
    let body = serde_json::json!({ "controls": controls });
    client::patch_json_discard("/api/v1/effects/current/controls", &body)
        .await
        .map_err(Into::into)
}

/// Outcome of a scoped control PATCH against an effect id.
///
/// The `Stale` variant is surfaced separately from generic errors so
/// the Viewport Designer modal can drive its reconciliation dialog off
/// a real type rather than HTTP-status string-matching.
pub enum UpdateControlsOutcome {
    /// Applied; the `new_version` is what the caller should echo as the
    /// next `If-Match` header on a subsequent PATCH.
    Applied { new_version: u64 },
    /// Server's current version no longer matches the `If-Match` we
    /// sent. `current` is the fresh version token to rebase against.
    Stale { current: u64 },
}

/// Scoped control PATCH against a specific effect id with optional
/// optimistic-concurrency precondition.
///
/// See Spec 46 § 9.1. Pass `None` for `expected_version` to skip the
/// `If-Match` header (the server then applies unconditionally).
pub async fn update_effect_controls(
    effect_id: &str,
    controls: &serde_json::Value,
    expected_version: Option<u64>,
) -> Result<UpdateControlsOutcome, String> {
    use gloo_net::http::Request;

    let url = format!("/api/v1/effects/{effect_id}/controls");
    let body_str = serde_json::to_string(&serde_json::json!({ "controls": controls }))
        .map_err(|e| e.to_string())?;
    let mut req = Request::patch(&url).header("Content-Type", "application/json");
    if let Some(version) = expected_version {
        req = req.header("If-Match", &version.to_string());
    }
    let resp = req
        .body(body_str)
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;

    match resp.status() {
        200..=299 => {
            // Successful response carries the new `controls_version`
            // both in the body (nested under the envelope's `data`)
            // and in the `ETag` header. Prefer the body because the
            // `gloo_net` Response type makes header parsing marginally
            // clunkier than JSON extraction.
            let body: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
            let new_version = body["data"]["controls_version"]
                .as_u64()
                .ok_or_else(|| "response missing controls_version".to_owned())?;
            Ok(UpdateControlsOutcome::Applied { new_version })
        }
        412 => {
            let body: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
            let current = body["current"]
                .as_u64()
                .ok_or_else(|| "412 body missing `current`".to_owned())?;
            Ok(UpdateControlsOutcome::Stale { current })
        }
        status => Err(format!("HTTP {status}")),
    }
}

/// Reset all controls on the active effect to their defaults.
pub async fn reset_controls() -> Result<(), String> {
    client::post_empty("/api/v1/effects/current/reset")
        .await
        .map_err(Into::into)
}

pub async fn upload_effect(file: File) -> Result<InstalledEffectResponse, String> {
    let form_data = FormData::new().map_err(|error| format!("{error:?}"))?;
    form_data
        .append_with_blob_and_filename("file", &file, &file.name())
        .map_err(|error| format!("{error:?}"))?;

    let response = Request::post("/api/v1/effects/install")
        .body(form_data)
        .map_err(|error| error.to_string())?
        .send()
        .await
        .map_err(|error| error.to_string())?;

    if !(200..300).contains(&response.status()) {
        let fallback = format!("HTTP {}", response.status());
        let payload = response.json::<serde_json::Value>().await.ok();
        let detail_errors = payload
            .as_ref()
            .and_then(|value| value["error"]["details"]["errors"].as_array())
            .map(|errors| {
                errors
                    .iter()
                    .filter_map(serde_json::Value::as_str)
                    .collect::<Vec<_>>()
                    .join("; ")
            })
            .filter(|joined| !joined.is_empty());
        let message = detail_errors
            .or_else(|| {
                payload
                    .as_ref()
                    .and_then(|value| value["error"]["message"].as_str())
                    .map(str::to_owned)
            })
            .unwrap_or(fallback);
        return Err(message);
    }

    response
        .json::<super::ApiEnvelope<InstalledEffectResponse>>()
        .await
        .map(|payload| payload.data)
        .map_err(|error| error.to_string())
}
