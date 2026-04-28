//! REST client for the Hypercolor daemon HTTP API.

use std::collections::HashMap;

use anyhow::{Context, Result};
use bytes::Bytes;
use futures_util::stream::{self, StreamExt};
use hypercolor_types::controls::{
    ApplyControlChangesRequest, ApplyControlChangesResponse, ControlActionResult,
    ControlSurfaceDocument, ControlValueMap,
};
use hypercolor_types::effect::{
    ControlDefinition as ApiControlDefinition, ControlType as ApiControlType,
    ControlValue as ApiControlValue, PresetTemplate as ApiPresetTemplate,
};
use reqwest::StatusCode;
use serde::Deserialize;
use serde::de::DeserializeOwned;

use crate::state::{
    CanvasFrame, ControlDefinition, ControlValue, DaemonState, DeviceSummary, EffectSummary,
    PresetTemplate, SimulatedDisplaySummary,
};

/// HTTP client for the daemon REST API.
#[derive(Debug, Clone)]
pub struct DaemonClient {
    base_url: String,
    http: reqwest::Client,
}

impl DaemonClient {
    /// Create a client targeting the given host and port.
    #[must_use]
    pub fn new(host: &str, port: u16) -> Self {
        let base_url = format!("http://{host}:{port}");
        Self {
            base_url,
            http: reqwest::Client::new(),
        }
    }

    /// The base URL for the daemon.
    #[must_use]
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Fetch the daemon's current state.
    pub async fn get_status(&self) -> Result<DaemonState> {
        let status: SystemStatusResponse = self.get_data("/status").await?;
        let active_effect = self.get_active_effect().await.ok();

        #[allow(clippy::cast_possible_truncation, clippy::as_conversions)]
        let device_count = status.device_count as u32;

        Ok(DaemonState {
            running: status.running,
            brightness: status.global_brightness,
            fps_target: 0.0,
            fps_actual: 0.0,
            effect_name: active_effect
                .as_ref()
                .map(|effect| effect.name.clone())
                .or(status.active_effect),
            effect_id: active_effect.map(|effect| effect.id),
            scene_name: status.active_scene,
            scene_snapshot_locked: status.active_scene_snapshot_locked,
            profile_name: None,
            device_count,
            total_leds: 0,
        })
    }

    /// Fetch all available effects.
    pub async fn get_effects(&self) -> Result<Vec<EffectSummary>> {
        let response: EffectListResponse = self.get_data("/effects").await?;

        let mut effects = stream::iter(response.items.into_iter().map(|summary| {
            let client = self.clone();
            async move {
                let detail = client
                    .get_effect_detail(&summary.id)
                    .await
                    .map_err(|error| {
                        tracing::warn!(
                            effect_id = %summary.id,
                            %error,
                            "Failed to fetch effect details; using summary only"
                        );
                        error
                    });

                map_effect_summary(summary, detail.ok())
            }
        }))
        .buffer_unordered(8)
        .collect::<Vec<_>>()
        .await;

        effects.sort_by(|left, right| {
            let left_norm = left.name.to_ascii_lowercase();
            let right_norm = right.name.to_ascii_lowercase();
            left_norm
                .cmp(&right_norm)
                .then_with(|| left.name.cmp(&right.name))
        });

        Ok(effects)
    }

    /// Fetch all connected devices.
    pub async fn get_devices(&self) -> Result<Vec<DeviceSummary>> {
        let response: DeviceListResponse = self.get_data("/devices").await?;
        Ok(response.items.into_iter().map(map_device_summary).collect())
    }

    /// Fetch control surfaces selected by device, driver, or both.
    pub async fn get_control_surfaces(
        &self,
        query: ControlSurfaceQuery<'_>,
    ) -> Result<Vec<ControlSurfaceDocument>> {
        let response: ControlSurfaceListResponse =
            self.get_data(&control_surface_list_path(query)).await?;
        Ok(response.surfaces)
    }

    /// Fetch device-owned and optional driver-owned control surfaces.
    pub async fn get_device_control_surfaces(
        &self,
        device_id: &str,
        include_driver: bool,
    ) -> Result<Vec<ControlSurfaceDocument>> {
        self.get_control_surfaces(ControlSurfaceQuery {
            device_id: Some(device_id),
            driver_id: None,
            include_driver,
        })
        .await
    }

    /// Fetch one driver-level control surface through the direct endpoint.
    pub async fn get_driver_control_surface(
        &self,
        driver_id: &str,
    ) -> Result<ControlSurfaceDocument> {
        self.get_data(&format!("/drivers/{}/controls", path_segment(driver_id)))
            .await
    }

    /// Apply typed changes to a dynamic control surface.
    pub async fn apply_control_changes(
        &self,
        request: &ApplyControlChangesRequest,
    ) -> Result<ApplyControlChangesResponse> {
        let path = format!(
            "/control-surfaces/{}/values",
            path_segment(&request.surface_id)
        );
        self.patch_data(&path, request).await
    }

    /// Invoke a typed dynamic control-surface action.
    pub async fn invoke_control_action(
        &self,
        surface_id: &str,
        action_id: &str,
        input: ControlValueMap,
    ) -> Result<ControlActionResult> {
        let path = format!(
            "/control-surfaces/{}/actions/{}",
            path_segment(surface_id),
            path_segment(action_id)
        );
        self.post_data(&path, &InvokeControlActionRequest { input })
            .await
    }

    /// Fetch all configured virtual display simulators.
    pub async fn get_simulated_displays(&self) -> Result<Vec<SimulatedDisplaySummary>> {
        self.get_data("/simulators/displays").await
    }

    /// Fetch the latest rendered frame for a virtual display simulator.
    pub async fn get_simulated_display_frame(
        &self,
        simulator_id: &str,
    ) -> Result<Option<CanvasFrame>> {
        let url = format!(
            "{}/api/v1/simulators/displays/{simulator_id}/frame",
            self.base_url
        );
        let response = self
            .http
            .get(&url)
            .send()
            .await
            .with_context(|| format!("Failed to fetch simulator frame for {simulator_id}"))?;

        if response.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("Simulator frame request failed ({status}): {body}");
        }

        let bytes = response.bytes().await?;
        decode_simulated_display_frame(bytes.as_ref()).map(Some)
    }

    /// Fetch the favorites list (effect IDs).
    pub async fn get_favorites(&self) -> Result<Vec<String>> {
        let response: FavoriteListResponse = self.get_data("/library/favorites").await?;
        Ok(response
            .items
            .into_iter()
            .map(|favorite| favorite.effect_id)
            .collect())
    }

    /// Apply an effect by ID.
    pub async fn apply_effect(
        &self,
        effect_id: &str,
        controls: Option<&serde_json::Value>,
    ) -> Result<()> {
        let url = format!("{}/api/v1/effects/{effect_id}/apply", self.base_url);
        let mut req = self.http.post(&url);
        if let Some(body) = controls {
            req = req.json(body);
        } else {
            req = req.json(&serde_json::json!({}));
        }
        let response = req.send().await.with_context(|| {
            format!("Failed to apply effect {effect_id}. Is the daemon running?")
        })?;
        ensure_success(response, &format!("Apply effect failed for {effect_id}")).await
    }

    /// Toggle favorite for an effect.
    pub async fn toggle_favorite(&self, effect_id: &str, is_favorite: bool) -> Result<()> {
        if is_favorite {
            let url = format!("{}/api/v1/library/favorites/{effect_id}", self.base_url);
            let response = self.http.delete(&url).send().await?;
            ensure_success(response, &format!("Failed to remove favorite {effect_id}")).await?;
        } else {
            let url = format!("{}/api/v1/library/favorites", self.base_url);
            let response = self
                .http
                .post(&url)
                .json(&serde_json::json!({ "effect": effect_id }))
                .send()
                .await?;
            ensure_success(response, &format!("Failed to add favorite {effect_id}")).await?;
        }
        Ok(())
    }

    /// Update a control value on the active effect.
    pub async fn update_control(&self, control_id: &str, value: &serde_json::Value) -> Result<()> {
        let url = format!("{}/api/v1/effects/current/controls", self.base_url);
        let response = self
            .http
            .patch(&url)
            .json(&serde_json::json!({ "controls": { control_id: value } }))
            .send()
            .await
            .with_context(|| "Failed to update control")?;
        ensure_success(response, &format!("Failed to update control {control_id}")).await
    }

    /// Reset all controls on the active effect to their defaults.
    pub async fn reset_controls(&self) -> Result<()> {
        let url = format!("{}/api/v1/effects/current/reset", self.base_url);
        let response = self
            .http
            .post(&url)
            .send()
            .await
            .context("Failed to reset controls")?;
        ensure_success(response, "Failed to reset controls").await
    }

    // ── Internal helpers ────────────────────────────────────

    async fn get_data<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        let url = format!("{}/api/v1{path}", self.base_url);
        let response = self
            .http
            .get(&url)
            .send()
            .await
            .with_context(|| format!("Failed to connect to daemon at {url}"))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("API request failed ({status}): {body}");
        }

        // The API wraps responses in { "data": T, "meta": {...} }
        let envelope: serde_json::Value = response.json().await?;
        if let Some(data) = envelope.get("data") {
            Ok(serde_json::from_value(data.clone())?)
        } else {
            // Some endpoints return the data directly
            Ok(serde_json::from_value(envelope)?)
        }
    }

    async fn post_data<Req, Res>(&self, path: &str, body: &Req) -> Result<Res>
    where
        Req: serde::Serialize + ?Sized,
        Res: DeserializeOwned,
    {
        let url = format!("{}/api/v1{path}", self.base_url);
        let response = self
            .http
            .post(&url)
            .json(body)
            .send()
            .await
            .with_context(|| format!("Failed to connect to daemon at {url}"))?;
        response_data(response).await
    }

    async fn patch_data<Req, Res>(&self, path: &str, body: &Req) -> Result<Res>
    where
        Req: serde::Serialize + ?Sized,
        Res: DeserializeOwned,
    {
        let url = format!("{}/api/v1{path}", self.base_url);
        let response = self
            .http
            .patch(&url)
            .json(body)
            .send()
            .await
            .with_context(|| format!("Failed to connect to daemon at {url}"))?;
        response_data(response).await
    }

    async fn get_effect_detail(&self, effect_id: &str) -> Result<EffectDetailResponse> {
        self.get_data(&format!("/effects/{effect_id}")).await
    }

    async fn get_active_effect(&self) -> Result<ActiveEffectResponse> {
        self.get_data("/effects/active").await
    }
}

/// Query parameters for the aggregate control-surface endpoint.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ControlSurfaceQuery<'a> {
    pub device_id: Option<&'a str>,
    pub driver_id: Option<&'a str>,
    pub include_driver: bool,
}

#[derive(Debug, Deserialize)]
struct ControlSurfaceListResponse {
    surfaces: Vec<ControlSurfaceDocument>,
}

#[derive(Debug, serde::Serialize)]
struct InvokeControlActionRequest {
    input: ControlValueMap,
}

#[derive(Debug, Deserialize)]
struct EffectListResponse {
    items: Vec<ApiEffectSummary>,
}

#[derive(Debug, Deserialize)]
struct ApiEffectSummary {
    id: String,
    name: String,
    description: String,
    author: String,
    category: String,
    source: String,
    #[serde(default)]
    audio_reactive: bool,
    #[serde(default)]
    tags: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct EffectDetailResponse {
    id: String,
    name: String,
    description: String,
    author: String,
    category: String,
    source: String,
    #[serde(default)]
    audio_reactive: bool,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    controls: Vec<ApiControlDefinition>,
    #[serde(default)]
    presets: Vec<ApiPresetTemplate>,
    #[serde(default)]
    active_control_values: Option<HashMap<String, ApiControlValue>>,
}

#[derive(Debug, Deserialize)]
struct DeviceListResponse {
    items: Vec<ApiDeviceSummary>,
}

#[derive(Debug, Deserialize)]
struct ApiDeviceSummary {
    id: String,
    name: String,
    backend: String,
    status: String,
    total_leds: u32,
}

#[derive(Debug, Deserialize)]
struct FavoriteListResponse {
    items: Vec<FavoriteSummaryResponse>,
}

#[derive(Debug, Deserialize)]
struct FavoriteSummaryResponse {
    effect_id: String,
}

#[derive(Debug, Deserialize)]
struct SystemStatusResponse {
    running: bool,
    global_brightness: u8,
    device_count: usize,
    active_effect: Option<String>,
    active_scene: Option<String>,
    #[serde(default)]
    active_scene_snapshot_locked: bool,
}

#[derive(Debug, Deserialize)]
struct ActiveEffectResponse {
    id: String,
    name: String,
}

fn map_effect_summary(
    summary: ApiEffectSummary,
    detail: Option<EffectDetailResponse>,
) -> EffectSummary {
    if let Some(detail) = detail {
        let overrides = detail.active_control_values.as_ref();
        return EffectSummary {
            id: detail.id,
            name: detail.name,
            description: detail.description,
            author: detail.author,
            category: detail.category,
            source: detail.source,
            audio_reactive: detail.audio_reactive,
            tags: detail.tags,
            controls: detail
                .controls
                .iter()
                .map(|control| map_control_definition(control, overrides))
                .collect(),
            presets: detail.presets.iter().map(map_preset_template).collect(),
        };
    }

    EffectSummary {
        id: summary.id,
        name: summary.name,
        description: summary.description,
        author: summary.author,
        category: summary.category,
        source: summary.source,
        audio_reactive: summary.audio_reactive,
        tags: summary.tags,
        controls: Vec::new(),
        presets: Vec::new(),
    }
}

fn map_control_definition(
    control: &ApiControlDefinition,
    overrides: Option<&HashMap<String, ApiControlValue>>,
) -> ControlDefinition {
    let control_id = control.control_id().to_owned();
    let default_value = overrides
        .and_then(|values| values.get(&control_id))
        .map_or_else(
            || map_control_value(&control.default_value),
            map_control_value,
        );

    ControlDefinition {
        id: control_id,
        name: control.name.clone(),
        control_type: map_control_type(&control.control_type),
        default_value,
        min: control.min,
        max: control.max,
        step: control.step,
        labels: control.labels.clone(),
        group: control.group.clone(),
        tooltip: control.tooltip.clone(),
    }
}

fn map_control_type(control_type: &ApiControlType) -> String {
    match control_type {
        ApiControlType::Slider => "slider",
        ApiControlType::Toggle => "toggle",
        ApiControlType::ColorPicker => "color",
        ApiControlType::GradientEditor => "gradient",
        ApiControlType::Dropdown => "dropdown",
        ApiControlType::TextInput => "text",
        ApiControlType::Rect => "rect",
    }
    .to_string()
}

fn map_control_value(value: &ApiControlValue) -> ControlValue {
    match value {
        ApiControlValue::Float(v) => ControlValue::Float(*v),
        ApiControlValue::Integer(v) => ControlValue::Integer(*v),
        ApiControlValue::Boolean(v) => ControlValue::Boolean(*v),
        ApiControlValue::Color(v) => ControlValue::Color(*v),
        ApiControlValue::Enum(v) | ApiControlValue::Text(v) => ControlValue::Text(v.clone()),
        ApiControlValue::Gradient(stops) => {
            ControlValue::Text(format!("{} gradient stops", stops.len()))
        }
        ApiControlValue::Rect(rect) => ControlValue::Text(format!(
            "{:.2},{:.2} {:.2}×{:.2}",
            rect.x, rect.y, rect.width, rect.height,
        )),
    }
}

fn map_preset_template(template: &ApiPresetTemplate) -> PresetTemplate {
    PresetTemplate {
        name: template.name.clone(),
        description: template.description.clone(),
        controls: template
            .controls
            .iter()
            .map(|(name, value)| (name.clone(), map_control_value(value)))
            .collect(),
    }
}

fn map_device_summary(device: ApiDeviceSummary) -> DeviceSummary {
    DeviceSummary {
        id: device.id,
        name: device.name,
        family: device.backend,
        led_count: device.total_leds,
        state: device.status,
        fps: None,
    }
}

fn decode_simulated_display_frame(bytes: &[u8]) -> Result<CanvasFrame> {
    let image =
        image::load_from_memory(bytes).context("Failed to decode simulator preview image")?;
    let rgb = image.to_rgb8();
    let width = rgb.width();
    let height = rgb.height();

    if width > u32::from(u16::MAX) || height > u32::from(u16::MAX) {
        anyhow::bail!("Simulator preview dimensions exceed TUI limits: {width}x{height}");
    }

    #[allow(clippy::cast_possible_truncation, clippy::as_conversions)]
    Ok(CanvasFrame {
        frame_number: 0,
        timestamp_ms: 0,
        width: width as u16,
        height: height as u16,
        pixels: Bytes::from(rgb.into_raw()),
    })
}

async fn ensure_success(response: reqwest::Response, context: &str) -> Result<()> {
    if response.status().is_success() {
        return Ok(());
    }

    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    anyhow::bail!("{context} ({status}): {body}");
}

async fn response_data<T: DeserializeOwned>(response: reqwest::Response) -> Result<T> {
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        anyhow::bail!("API request failed ({status}): {body}");
    }

    let envelope: serde_json::Value = response.json().await?;
    if let Some(data) = envelope.get("data") {
        Ok(serde_json::from_value(data.clone())?)
    } else {
        Ok(serde_json::from_value(envelope)?)
    }
}

fn control_surface_list_path(query: ControlSurfaceQuery<'_>) -> String {
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
        "/control-surfaces".to_string()
    } else {
        format!("/control-surfaces?{}", parts.join("&"))
    }
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
            use std::fmt::Write as _;
            let _ = write!(encoded, "%{byte:02X}");
        }
    }
    encoded
}
