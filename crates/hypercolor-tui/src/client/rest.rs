//! REST client for the Hypercolor daemon HTTP API.

use anyhow::{Context, Result};
use bytes::Bytes;
use hypercolor_types::api::ApiResponse;
use hypercolor_types::api::controls::InvokeControlActionRequest;
use hypercolor_types::api::devices::{
    DeviceListResponse as ApiDeviceListResponse, DeviceSummary as ApiDeviceSummary,
};
use hypercolor_types::api::effects::{
    EffectDetailResponse, EffectListResponse as ApiEffectListResponse,
    EffectSummary as ApiEffectSummary,
};
use hypercolor_types::api::envelope::ApiErrorBody;
use hypercolor_types::api::library::{AddFavoriteRequest, FavoriteListResponse};
use hypercolor_types::api::scene::{
    ApplyEffectRequest, PatchControlsRequest, PatchZoneRequest, ReplaceLayerRequest, SceneDocument,
};
use hypercolor_types::api::scenes::SceneListResponse as ApiSceneListResponse;
use hypercolor_types::api::system::SystemResource;
use hypercolor_types::controls::{
    ApplyControlChangesResponse, ControlActionResult, ControlSurfaceDocument, ControlValueMap,
};
use hypercolor_types::effect::{
    ControlDefinition as ApiControlDefinition, ControlType as ApiControlType,
    PresetTemplate as ApiPresetTemplate,
};
use hypercolor_types::layer::{LayerSource, SceneLayer};
use hypercolor_types::scene::ZoneRole;
use reqwest::StatusCode;
use serde::Deserialize;
use serde::de::DeserializeOwned;

use crate::state::{
    CanvasFrame, ControlDefinition, ControlValue, DaemonState, DeviceSummary, EffectSummary,
    PresetTemplate, SceneSummary, SimulatedDisplaySummary,
};

/// HTTP client for the daemon REST API.
#[derive(Debug, Clone)]
pub struct DaemonClient {
    base_url: String,
    http: reqwest::Client,
    api_key: Option<String>,
}

impl DaemonClient {
    /// Create a client targeting the given host and port.
    #[must_use]
    pub fn new(host: &str, port: u16, api_key: Option<&str>) -> Self {
        let base_url = format!("http://{host}:{port}");
        Self {
            base_url,
            http: reqwest::Client::new(),
            api_key: api_key.map(ToOwned::to_owned),
        }
    }

    /// The base URL for the daemon.
    #[must_use]
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Fetch the daemon's current state.
    pub async fn get_status(&self) -> Result<DaemonState> {
        let system = self.get_data::<SystemResource>("/system").await?;
        let status = system
            .status
            .context("System status requires daemon read access")?;

        #[allow(clippy::cast_possible_truncation, clippy::as_conversions)]
        let device_count = status.device_count as u32;
        #[allow(clippy::cast_precision_loss, clippy::as_conversions)]
        let fps_target = status.render_loop.target_fps as f32;
        #[allow(clippy::cast_possible_truncation, clippy::as_conversions)]
        let fps_actual = status.render_loop.actual_fps as f32;

        Ok(DaemonState {
            running: status.running,
            brightness: status.global_brightness,
            fps_target,
            fps_actual,
            device_count,
            total_leds: 0,
        })
    }

    /// Fetch all available effects, controls and presets included.
    ///
    /// The catalog arrives fully hydrated in one response: the daemon
    /// expands each summary on request, so browsing the library costs a
    /// single round trip rather than one per effect.
    pub async fn get_effects(&self) -> Result<Vec<EffectSummary>> {
        let response: ApiEffectListResponse =
            self.get_data("/effects?include=controls,presets").await?;

        Ok(response.items.into_iter().map(map_effect_summary).collect())
    }

    /// Fetch all connected devices.
    pub async fn get_devices(&self) -> Result<Vec<DeviceSummary>> {
        let response: ApiDeviceListResponse = self.get_data("/devices").await?;
        Ok(response.items.into_iter().map(map_device_summary).collect())
    }

    /// Fetch control surfaces selected by device, driver, or both.
    pub async fn get_control_surfaces(
        &self,
        query: ControlSurfaceQuery<'_>,
    ) -> Result<Vec<ControlSurfaceDocument>> {
        let response: Option<ControlSurfaceListResponse> = self
            .get_optional_data(&control_surface_list_path(query))
            .await?;
        Ok(response.map_or_else(Vec::new, |response| response.surfaces))
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

    /// Fetch one control surface by stable surface ID.
    pub async fn get_control_surface(&self, surface_id: &str) -> Result<ControlSurfaceDocument> {
        self.get_data(&format!("/control-surfaces/{}", path_segment(surface_id)))
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
        surface_id: &str,
        request: &PatchControlsRequest,
    ) -> Result<ApplyControlChangesResponse> {
        let path = format!("/control-surfaces/{}/values", path_segment(surface_id));
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
            .auth_request(self.http.get(&url))
            .send()
            .await
            .with_context(|| format!("Failed to fetch simulator frame for {simulator_id}"))?;

        if response.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }

        if !response.status().is_success() {
            return Err(daemon_error("Simulator frame request failed", response).await);
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

    /// Apply an effect by ID, optionally with control overrides and a
    /// target zone (`zone_id`). No target = the scene's primary zone.
    pub async fn apply_effect(
        &self,
        effect_id: &str,
        controls: Option<&std::collections::HashMap<String, ControlValue>>,
        zone_id: Option<&str>,
    ) -> Result<()> {
        let url = format!(
            "{}/api/v1/effects/{}/apply",
            self.base_url,
            path_segment(effect_id)
        );
        let controls = controls.map(|values| {
            values
                .iter()
                .map(|(name, value)| (name.clone(), value.clone()))
                .collect()
        });
        let zone = zone_id
            .map(|zone_id| serde_json::from_value(serde_json::Value::String(zone_id.to_owned())))
            .transpose()
            .with_context(|| "Target zone must be a UUID")?;
        let body = ApplyEffectRequest {
            controls,
            zone,
            ..ApplyEffectRequest::default()
        };
        let response = self
            .auth_request(self.http.post(&url))
            .json(&body)
            .send()
            .await
            .with_context(|| {
                format!("Failed to apply effect {effect_id}. Is the daemon running?")
            })?;
        ensure_success(response, &format!("Apply effect failed for {effect_id}")).await
    }

    // ── Scenes & zones ──────────────────────────────────────

    /// Fetch all saved scenes.
    pub async fn get_scenes(&self) -> Result<Vec<SceneSummary>> {
        let response: ApiSceneListResponse = self.get_data("/scenes").await?;
        Ok(response.items)
    }

    /// Fetch the canonical live scene tree.
    pub async fn get_active_scene(&self) -> Result<SceneDocument> {
        let document: SceneDocument = self.get_data("/scene").await?;
        Ok(document)
    }

    /// Activate a saved scene by ID.
    pub async fn activate_scene(&self, scene_id: &str) -> Result<()> {
        let url = format!(
            "{}/api/v1/scenes/{}/activate",
            self.base_url,
            path_segment(scene_id)
        );
        let response = self
            .auth_request(self.http.post(&url))
            .send()
            .await
            .with_context(|| format!("Failed to activate scene {scene_id}"))?;
        ensure_success(response, &format!("Activate scene failed for {scene_id}")).await
    }

    /// Deactivate the active scene, returning to the ephemeral default.
    pub async fn deactivate_scene(&self) -> Result<()> {
        let url = format!("{}/api/v1/scene/deactivate", self.base_url);
        let response = self
            .auth_request(self.http.post(&url))
            .send()
            .await
            .context("Failed to deactivate scene")?;
        ensure_success(response, "Deactivate scene failed").await
    }

    /// Update zone metadata (enabled, brightness). Guarded by the scene's
    /// scene `revision` via `If-Match`; the daemon answers 412 when stale.
    pub async fn update_zone(
        &self,
        zone_id: &str,
        revision: u64,
        enabled: Option<bool>,
        brightness: Option<f32>,
    ) -> Result<()> {
        let url = format!(
            "{}/api/v1/scene/zones/{}",
            self.base_url,
            path_segment(zone_id)
        );
        let body = PatchZoneRequest {
            enabled,
            brightness,
            ..PatchZoneRequest::default()
        };
        let response = self
            .auth_request(self.http.patch(&url))
            .header(reqwest::header::IF_MATCH, revision.to_string())
            .json(&body)
            .send()
            .await
            .with_context(|| format!("Failed to update zone {zone_id}"))?;
        ensure_success(response, &format!("Zone update failed for {zone_id}")).await
    }

    /// Patch effect controls through the real layer identity read from `/scene`.
    pub async fn patch_zone_controls(
        &self,
        zone_id: &str,
        layer_id: &str,
        controls: &std::collections::BTreeMap<String, ControlValue>,
    ) -> Result<()> {
        let zone = path_segment(zone_id);
        let layer = path_segment(layer_id);
        let url = format!(
            "{}/api/v1/scene/zones/{zone}/layers/{layer}/controls",
            self.base_url
        );
        let response = self
            .auth_request(self.http.patch(&url))
            .json(&PatchControlsRequest {
                values: controls.clone(),
                clear_bindings: Vec::new(),
            })
            .send()
            .await
            .with_context(|| format!("Failed to update controls for zone {zone_id}"))?;
        ensure_success(
            response,
            &format!("Zone control update failed for {zone_id}"),
        )
        .await
    }

    /// Toggle favorite for an effect.
    pub async fn toggle_favorite(&self, effect_id: &str, is_favorite: bool) -> Result<()> {
        if is_favorite {
            let url = format!("{}/api/v1/library/favorites/{effect_id}", self.base_url);
            let response = self.auth_request(self.http.delete(&url)).send().await?;
            ensure_success(response, &format!("Failed to remove favorite {effect_id}")).await?;
        } else {
            let url = format!("{}/api/v1/library/favorites", self.base_url);
            let response = self
                .auth_request(self.http.post(&url))
                .json(&AddFavoriteRequest {
                    effect: effect_id.to_owned(),
                })
                .send()
                .await?;
            ensure_success(response, &format!("Failed to add favorite {effect_id}")).await?;
        }
        Ok(())
    }

    /// Update a control value on the active effect.
    pub async fn update_control(&self, control_id: &str, value: &ControlValue) -> Result<()> {
        let (zone_id, layer, _, _) = self.effect_layer_target(None).await?;
        self.patch_zone_controls(
            &zone_id,
            &layer.id.to_string(),
            &std::collections::BTreeMap::from([(control_id.to_owned(), value.clone())]),
        )
        .await
    }

    /// Reset one real effect layer to catalog defaults.
    pub async fn reset_controls(&self, zone_id: Option<&str>) -> Result<()> {
        let (zone_id, layer, effect_id, _) = self.effect_layer_target(zone_id).await?;
        let detail: EffectDetailResponse = self
            .get_data(&format!("/effects/{}", path_segment(&effect_id)))
            .await?;
        let values: std::collections::HashMap<_, _> = detail
            .controls
            .into_iter()
            .map(|control| (control.control_id().to_owned(), control.default_value))
            .collect();
        let LayerSource::Effect {
            effect_id,
            control_bindings,
            ..
        } = &layer.source
        else {
            anyhow::bail!("The target zone has no effect layer");
        };
        let url = format!(
            "{}/api/v1/scene/zones/{}/layers/{}",
            self.base_url,
            path_segment(&zone_id),
            layer.id
        );
        let response = self
            .auth_request(self.http.put(&url))
            .json(&ReplaceLayerRequest {
                source: LayerSource::Effect {
                    effect_id: *effect_id,
                    controls: values,
                    control_bindings: control_bindings.clone(),
                    preset_id: None,
                },
                name: layer.name.clone(),
                blend: Some(layer.blend),
                opacity: Some(layer.opacity),
                transform: Some(layer.transform),
                adjust: Some(layer.adjust),
                bindings: Some(layer.bindings.clone()),
                enabled: Some(layer.enabled),
            })
            .send()
            .await
            .context("Failed to reset controls")?;
        ensure_success(response, "Failed to reset controls").await
    }

    async fn effect_layer_target(
        &self,
        zone_id: Option<&str>,
    ) -> Result<(String, SceneLayer, String, Vec<String>)> {
        let document: SceneDocument = self.get_data("/scene").await?;
        let zone = zone_id.map_or_else(
            || {
                document
                    .zones
                    .iter()
                    .find(|zone| zone.role == ZoneRole::Primary)
                    .or_else(|| document.zones.first())
            },
            |zone_id| {
                document
                    .zones
                    .iter()
                    .find(|zone| zone.id.to_string() == zone_id)
            },
        );
        let zone = zone.context("The active scene has no target zone")?;
        let layer = zone
            .layers
            .iter()
            .rev()
            .find_map(|layer| match &layer.source {
                LayerSource::Effect {
                    effect_id,
                    control_bindings,
                    ..
                } => Some((
                    layer.clone(),
                    effect_id.to_string(),
                    control_bindings.keys().cloned().collect(),
                )),
                _ => None,
            })
            .context("The target zone has no effect layer")?;
        Ok((zone.id.to_string(), layer.0, layer.1, layer.2))
    }

    // ── Internal helpers ────────────────────────────────────

    async fn get_data<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        let url = format!("{}/api/v1{path}", self.base_url);
        let response = self
            .auth_request(self.http.get(&url))
            .send()
            .await
            .with_context(|| format!("Failed to connect to daemon at {url}"))?;

        if !response.status().is_success() {
            return Err(daemon_error("API request failed", response).await);
        }

        let envelope: ApiResponse<T> = response.json().await?;
        Ok(envelope.data)
    }

    async fn get_optional_data<T: DeserializeOwned>(&self, path: &str) -> Result<Option<T>> {
        let url = format!("{}/api/v1{path}", self.base_url);
        let response = self
            .auth_request(self.http.get(&url))
            .send()
            .await
            .with_context(|| format!("Failed to connect to daemon at {url}"))?;

        if response.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }

        response_data(response).await.map(Some)
    }

    async fn post_data<Req, Res>(&self, path: &str, body: &Req) -> Result<Res>
    where
        Req: serde::Serialize + ?Sized,
        Res: DeserializeOwned,
    {
        let url = format!("{}/api/v1{path}", self.base_url);
        let response = self
            .auth_request(self.http.post(&url))
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
            .auth_request(self.http.patch(&url))
            .json(body)
            .send()
            .await
            .with_context(|| format!("Failed to connect to daemon at {url}"))?;
        response_data(response).await
    }

    fn auth_request(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        if let Some(api_key) = &self.api_key {
            request.bearer_auth(api_key)
        } else {
            request
        }
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

fn map_effect_summary(summary: ApiEffectSummary) -> EffectSummary {
    EffectSummary {
        id: summary.id,
        name: summary.name,
        description: summary.description,
        author: summary.author,
        category: summary.category,
        source: summary.source,
        audio_reactive: summary.audio_reactive,
        tags: summary.tags,
        controls: summary
            .controls
            .unwrap_or_default()
            .iter()
            .map(map_control_definition)
            .collect(),
        presets: summary
            .presets
            .unwrap_or_default()
            .iter()
            .map(map_preset_template)
            .collect(),
    }
}

/// Map a control definition, preserving the effect's TRUE defaults.
///
/// Live values are deliberately NOT merged in here — they are per-zone
/// (selected from the canonical zone resource) and overlaying the primary zone's values onto
/// "defaults" made reset-to-default and zone-scoped editing impossible.
fn map_control_definition(control: &ApiControlDefinition) -> ControlDefinition {
    let control_id = control.control_id().to_owned();
    let default_value = control.default_value.clone();

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
        ApiControlType::Asset => "asset",
        ApiControlType::Rect => "rect",
    }
    .to_string()
}

fn map_preset_template(template: &ApiPresetTemplate) -> PresetTemplate {
    PresetTemplate {
        id: template.id.to_string(),
        name: template.name.clone(),
        description: template.description.clone(),
        controls: template.controls.clone(),
    }
}

fn map_device_summary(device: ApiDeviceSummary) -> DeviceSummary {
    DeviceSummary {
        id: device.id,
        name: device.name,
        family: device.origin.driver_id,
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

    Ok(CanvasFrame {
        frame_number: 0,
        timestamp_ms: 0,
        width,
        height,
        pixels: Bytes::from(rgb.into_raw()),
    })
}

async fn ensure_success(response: reqwest::Response, context: &str) -> Result<()> {
    if response.status().is_success() {
        return Ok(());
    }

    Err(daemon_error(context, response).await)
}

/// Turn a failed daemon response into one line a user can act on.
///
/// The daemon answers every error with the canonical envelope
/// `{ error: { code, message, details }, meta }`, so the code and message
/// are what a toast should carry. The raw body is the fallback for the
/// surfaces that bypass the envelope (Axum's own rejections, binary
/// routes).
async fn daemon_error(context: &str, response: reqwest::Response) -> anyhow::Error {
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    if let Ok(envelope) = serde_json::from_str::<ApiErrorBody>(&body) {
        return anyhow::anyhow!(
            "{context} ({status}, {}): {}",
            envelope.error.code,
            envelope.error.message
        );
    }
    let trimmed = body.trim();
    if trimmed.is_empty() {
        anyhow::anyhow!("{context} ({status})")
    } else {
        anyhow::anyhow!("{context} ({status}): {trimmed}")
    }
}

async fn response_data<T: DeserializeOwned>(response: reqwest::Response) -> Result<T> {
    if !response.status().is_success() {
        return Err(daemon_error("API request failed", response).await);
    }

    let envelope: ApiResponse<T> = response.json().await?;
    Ok(envelope.data)
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
