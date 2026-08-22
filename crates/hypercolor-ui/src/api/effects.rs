//! Effect-related API types and fetch functions.

use std::collections::HashMap;

use gloo_net::http::Method;
use hypercolor_types::api::scene::{ClearSceneRequest, ReplaceLayerRequest, SceneDocument};
use hypercolor_types::control::ControlValue;
use hypercolor_types::effect::ControlDefinition;
use hypercolor_types::layer::LayerSource;
use hypercolor_types::scene::ZoneRole;
use web_sys::{File, FormData};

use super::client;
use crate::control_surface_api::path_segment;

// ── Types ───────────────────────────────────────────────────────────────────

pub use hypercolor_types::api::effects::{
    EffectCapabilitySet, EffectDetailResponse, EffectListResponse, EffectPresetListResponse,
    EffectPresetOrigin, EffectPresetSummary, EffectSummary, InstalledEffectResponse,
};
pub use hypercolor_types::api::scene::ApplyEffectRequest as ApplyEffectBody;

/// UI projection of the top effect layer in the live scene.
#[derive(Debug, Clone, PartialEq)]
pub struct PrimaryEffectView {
    pub id: String,
    pub name: String,
    pub controls: Vec<ControlDefinition>,
    pub control_values: HashMap<String, ControlValue>,
    pub active_preset_id: Option<String>,
}

// ── Fetch Functions ─────────────────────────────────────────────────────────

/// Fetch all registered effects.
pub async fn fetch_effects() -> Result<Vec<EffectSummary>, String> {
    let list: EffectListResponse = client::fetch_json("/api/v1/effects").await?;
    Ok(list.items.into_iter().map(route_effect_summary).collect())
}

/// Project the primary zone's top effect layer from the live scene tree.
pub async fn fetch_primary_effect_view() -> Result<Option<PrimaryEffectView>, String> {
    let scene: SceneDocument = client::fetch_json("/api/v1/scene").await?;
    let Some((_, _, effect_id, values, _, preset_id)) = effect_target(&scene, None, None) else {
        return Ok(None);
    };
    let detail = fetch_effect_detail(&effect_id.to_string()).await?;
    Ok(Some(PrimaryEffectView {
        id: effect_id,
        name: detail.name,
        controls: detail.controls,
        control_values: values,
        active_preset_id: preset_id,
    }))
}

/// Fetch detailed metadata for one effect.
pub async fn fetch_effect_detail(id: &str) -> Result<EffectDetailResponse, String> {
    let mut detail: EffectDetailResponse =
        client::fetch_json(&format!("/api/v1/effects/{}", path_segment(id)))
            .await
            .map_err(String::from)?;
    detail.cover_image_url = route_cover_image_url(detail.cover_image_url);
    Ok(detail)
}

fn route_effect_summary(mut effect: EffectSummary) -> EffectSummary {
    effect.cover_image_url = route_cover_image_url(effect.cover_image_url);
    effect
}

fn route_cover_image_url(cover_image_url: Option<String>) -> Option<String> {
    cover_image_url.and_then(|url| client::daemon_url(&url))
}

/// Fetch the bundled and saved preset stack for one effect.
pub async fn fetch_effect_presets(id: &str) -> Result<Vec<EffectPresetSummary>, String> {
    let response: EffectPresetListResponse =
        client::fetch_json(&format!("/api/v1/effects/{}/presets", path_segment(id))).await?;
    Ok(response.items)
}

/// Apply a bundled or saved preset to an effect and optional render group.
pub async fn apply_effect_preset(
    effect_id: &str,
    preset_id: &str,
    zone_id: Option<&str>,
) -> Result<(), String> {
    let path = format!(
        "/api/v1/effects/{}/presets/{}/apply",
        path_segment(effect_id),
        path_segment(preset_id)
    );
    let zone = zone_id
        .map(|zone_id| {
            uuid::Uuid::parse_str(zone_id)
                .map(hypercolor_types::scene::ZoneId)
                .map_err(|_| "Target zone must be a UUID".to_owned())
        })
        .transpose()?;
    client::post_json_discard(
        &path,
        &ApplyEffectBody {
            zone,
            ..ApplyEffectBody::default()
        },
    )
    .await
    .map_err(Into::into)
}

/// Apply an effect by ID or name. Pass `None` for a bare start; pass
/// `Some(body)` to deliver preferences atomically.
pub async fn apply_effect(id: &str, body: Option<&ApplyEffectBody>) -> Result<(), String> {
    let path = format!("/api/v1/effects/{}/apply", path_segment(id));
    match body {
        Some(body) => client::post_json_discard(&path, body)
            .await
            .map_err(Into::into),
        None => client::post_empty(&path).await.map_err(Into::into),
    }
}

/// Stop the currently active effect.
pub async fn stop_effect() -> Result<(), String> {
    client::post_json_discard("/api/v1/scene/clear", &ClearSceneRequest::default())
        .await
        .map_err(Into::into)
}

/// Update effect control parameters.
pub async fn update_controls(controls: &HashMap<String, ControlValue>) -> Result<(), String> {
    let scene: SceneDocument = client::fetch_json("/api/v1/scene").await?;
    let Some((zone_id, layer_id, _, _, _, _)) = effect_target(&scene, None, None) else {
        return Err("The active scene has no effect layer".to_owned());
    };
    patch_controls(&zone_id, &layer_id, controls, Vec::new()).await
}

/// Patch controls on the live layer running a specific effect.
pub async fn update_effect_controls(
    effect_id: &str,
    controls: &HashMap<String, ControlValue>,
) -> Result<(), String> {
    let scene: SceneDocument = client::fetch_json("/api/v1/scene").await?;
    let Some((zone_id, layer_id, _, _, _, _)) = effect_target(&scene, None, Some(effect_id)) else {
        return Err("The requested effect is not present in the live scene".to_owned());
    };
    patch_controls(&zone_id, &layer_id, controls, Vec::new()).await
}

/// Reset all controls on the active effect to their defaults.
pub async fn reset_controls() -> Result<(), String> {
    let scene: SceneDocument = client::fetch_json("/api/v1/scene").await?;
    let Some(zone) = scene
        .zones
        .iter()
        .find(|zone| zone.role == ZoneRole::Primary)
        .or_else(|| scene.zones.first())
    else {
        return Err("The active scene has no effect layer".to_owned());
    };
    let Some(layer) = zone
        .layers
        .iter()
        .rev()
        .find(|layer| matches!(layer.source, LayerSource::Effect { .. }))
    else {
        return Err("The active scene has no effect layer".to_owned());
    };
    let LayerSource::Effect {
        effect_id,
        control_bindings,
        ..
    } = &layer.source
    else {
        unreachable!("the selected layer is an effect layer");
    };
    let detail = fetch_effect_detail(&effect_id.to_string()).await?;
    let values: std::collections::HashMap<_, _> = detail
        .controls
        .into_iter()
        .map(|control| (control.control_id().to_owned(), control.default_value))
        .collect();
    client::put_json_discard(
        &format!("/api/v1/scene/zones/{}/layers/{}", zone.id, layer.id),
        &ReplaceLayerRequest {
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
        },
    )
    .await
    .map_err(Into::into)
}

type EffectTarget = (
    String,
    String,
    String,
    HashMap<String, ControlValue>,
    Vec<String>,
    Option<String>,
);

fn effect_target(
    scene: &SceneDocument,
    zone_id: Option<&str>,
    effect_id: Option<&str>,
) -> Option<EffectTarget> {
    if let Some(zone_id) = zone_id {
        return scene
            .zones
            .iter()
            .find(|zone| zone.id.to_string() == zone_id)
            .and_then(|zone| effect_target_in_zone(zone, effect_id));
    }
    scene
        .zones
        .iter()
        .find(|zone| zone.role == ZoneRole::Primary)
        .and_then(|zone| effect_target_in_zone(zone, effect_id))
        .or_else(|| {
            scene
                .zones
                .iter()
                .find_map(|zone| effect_target_in_zone(zone, effect_id))
        })
}

fn effect_target_in_zone(
    zone: &hypercolor_types::api::scene::ZoneResource,
    effect_id: Option<&str>,
) -> Option<EffectTarget> {
    zone.layers.iter().rev().find_map(|layer| {
        let LayerSource::Effect {
            effect_id: current_effect_id,
            controls,
            control_bindings,
            preset_id,
        } = &layer.source
        else {
            return None;
        };
        if effect_id.is_some_and(|expected| current_effect_id.to_string() != expected) {
            return None;
        }
        Some((
            zone.id.to_string(),
            layer.id.to_string(),
            current_effect_id.to_string(),
            controls.clone(),
            control_bindings.keys().cloned().collect(),
            preset_id.map(|preset| preset.to_string()),
        ))
    })
}

async fn patch_controls(
    zone_id: &str,
    layer_id: &str,
    controls: &HashMap<String, ControlValue>,
    clear_bindings: Vec<String>,
) -> Result<(), String> {
    let path = format!(
        "/api/v1/scene/zones/{}/layers/{}/controls",
        path_segment(zone_id),
        path_segment(layer_id)
    );
    client::patch_json_discard(
        &path,
        &super::layers::control_patch_request(controls, clear_bindings),
    )
    .await
    .map_err(Into::into)
}

pub async fn upload_effect(file: File) -> Result<InstalledEffectResponse, String> {
    let form_data = FormData::new().map_err(|error| format!("{error:?}"))?;
    form_data
        .append_with_blob_and_filename("file", &file, &file.name())
        .map_err(|error| format!("{error:?}"))?;

    let request = client::request(Method::POST, "/api/v1/effects/install").map_err(String::from)?;
    let response = request
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
        .json::<hypercolor_types::api::ApiResponse<InstalledEffectResponse>>()
        .await
        .map(|payload| payload.data)
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    #[test]
    fn effect_cover_urls_require_verified_native_route_and_preserve_browser_same_origin() {
        crate::api::client::reset_daemon_transport_for_test();
        let route = Some("/api/v1/effects/prism/cover".to_owned());
        assert_eq!(super::route_cover_image_url(route.clone()), route);

        crate::api::client::begin_native_daemon_verification();
        assert_eq!(super::route_cover_image_url(route.clone()), None);

        crate::api::client::install_verified_daemon_connection(
            "http://127.0.0.1:9420",
            Some("protected"),
        );
        assert_eq!(
            super::route_cover_image_url(route),
            Some("http://127.0.0.1:9420/api/v1/effects/prism/cover".to_owned())
        );
        crate::api::client::reset_daemon_transport_for_test();
    }
}
