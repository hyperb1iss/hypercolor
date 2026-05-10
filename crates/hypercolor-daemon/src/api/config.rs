//! Config endpoints — `/api/v1/config*`.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Query, State};
use axum::response::Response;
use serde::Deserialize;
use tracing::{info, warn};
use utoipa::ToSchema;

use hypercolor_core::config::canonical_audio_device_id;
use hypercolor_core::engine::FpsTier;
use hypercolor_types::audio::{AudioPipelineConfig, AudioSourceType};
use hypercolor_types::config::HypercolorConfig;

use crate::api::AppState;
use crate::api::envelope::{ApiError, ApiResponse};
use crate::scene_transactions::apply_layout_update;

#[derive(Debug, Deserialize)]
pub struct GetConfigQuery {
    pub key: String,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct SetConfigRequest {
    pub key: String,
    pub value: String,
    pub live: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct ResetConfigRequest {
    pub key: Option<String>,
    pub live: Option<bool>,
}

/// `GET /api/v1/config` — Show full effective config.
pub async fn show_config(State(state): State<Arc<AppState>>) -> Response {
    ApiResponse::ok(config_snapshot(&state))
}

/// `GET /api/v1/config/get?key=...` — Read a dotted config key.
pub async fn get_config_value(
    State(state): State<Arc<AppState>>,
    Query(query): Query<GetConfigQuery>,
) -> Response {
    let key = normalize_config_key(&query.key);
    let config = config_snapshot(&state);
    let value = match serde_json::to_value(config) {
        Ok(v) => v,
        Err(e) => return ApiError::internal(format!("Failed to serialize config: {e}")),
    };

    let Some(found) = get_json_path(&value, &key) else {
        return ApiError::not_found(format!("Unknown config key: {}", query.key));
    };

    ApiResponse::ok(serde_json::json!({
        "key": key,
        "value": found,
    }))
}

/// `POST /api/v1/config/set` — Set a dotted config key and persist.
pub async fn set_config_value(
    State(state): State<Arc<AppState>>,
    Json(body): Json<SetConfigRequest>,
) -> Response {
    let Some(manager) = state.config_manager.as_ref() else {
        return ApiError::internal("Config manager unavailable in this runtime");
    };

    let current = config_snapshot(&state);
    let mut root = match serde_json::to_value(current) {
        Ok(v) => v,
        Err(e) => return ApiError::internal(format!("Failed to serialize config: {e}")),
    };

    let key = normalize_config_key(&body.key);
    let parsed_value = serde_json::from_str::<serde_json::Value>(&body.value)
        .unwrap_or_else(|_| serde_json::Value::String(body.value.clone()));
    let parsed_value = canonicalize_config_value(&key, parsed_value);

    if get_json_path(&root, &key).is_some_and(|current| current == &parsed_value) {
        info!(
            key,
            live_requested = body.live.unwrap_or(false),
            "Skipping config update because value is unchanged"
        );
        return ApiResponse::ok(serde_json::json!({
            "key": key,
            "value": parsed_value,
            "live": false,
            "path": manager.path().display().to_string(),
        }));
    }

    if !set_json_path(&mut root, &key, parsed_value.clone()) {
        return ApiError::validation(format!("Invalid config key path: {}", body.key));
    }

    let updated: HypercolorConfig = match serde_json::from_value(root) {
        Ok(cfg) => cfg,
        Err(e) => {
            return ApiError::validation(format!(
                "Config update failed validation for '{}': {e}",
                key
            ));
        }
    };
    if let Err(error) = validate_driver_config_scope(&state, Some(&key), &updated) {
        return ApiError::validation(error);
    }

    manager.update(updated);
    if let Err(e) = manager.save() {
        return ApiError::internal(format!("Failed to persist config: {e}"));
    }
    let effective_config = manager.get();
    let effective_root = match serde_json::to_value(&**effective_config) {
        Ok(value) => value,
        Err(error) => {
            return ApiError::internal(format!(
                "Failed to serialize canonicalized config: {error}"
            ));
        }
    };
    let Some(effective_value) = get_json_path(&effective_root, &key).cloned() else {
        return ApiError::internal(format!(
            "Canonicalized config is missing expected key: {}",
            key
        ));
    };

    let audio_live_applied =
        maybe_apply_audio_config_change(&state, Some(&key), body.live.unwrap_or(false)).await;
    let render_live_applied = maybe_apply_render_config_change(&state, Some(&key)).await;
    let live_applied = audio_live_applied || render_live_applied;

    ApiResponse::ok(serde_json::json!({
        "key": key,
        "value": effective_value,
        "live": live_applied,
        "path": manager.path().display().to_string(),
    }))
}

fn canonicalize_config_value(key: &str, value: serde_json::Value) -> serde_json::Value {
    if key == "audio.device" {
        value.as_str().map_or(value.clone(), |device| {
            serde_json::Value::String(canonical_audio_device_id(device))
        })
    } else {
        value
    }
}

/// `POST /api/v1/config/reset` — Reset one key or the full config to defaults.
pub async fn reset_config_value(
    State(state): State<Arc<AppState>>,
    Json(body): Json<ResetConfigRequest>,
) -> Response {
    let Some(manager) = state.config_manager.as_ref() else {
        return ApiError::internal("Config manager unavailable in this runtime");
    };

    let mut current = match serde_json::to_value(config_snapshot(&state)) {
        Ok(v) => v,
        Err(e) => return ApiError::internal(format!("Failed to serialize config: {e}")),
    };
    let defaults = match serde_json::to_value(HypercolorConfig::default()) {
        Ok(v) => v,
        Err(e) => return ApiError::internal(format!("Failed to serialize default config: {e}")),
    };

    let normalized_key = body.key.as_deref().map(normalize_config_key);
    if let Some(key) = normalized_key.as_deref() {
        let Some(default_value) = get_json_path(&defaults, key) else {
            return ApiError::not_found(format!(
                "Unknown config key: {}",
                body.key.as_deref().unwrap_or(key)
            ));
        };

        if !set_json_path(&mut current, key, default_value.clone()) {
            return ApiError::validation(format!("Invalid config key path: {key}"));
        }
    } else {
        current = defaults;
    }

    let updated: HypercolorConfig = match serde_json::from_value(current) {
        Ok(cfg) => cfg,
        Err(e) => return ApiError::internal(format!("Failed to build reset config: {e}")),
    };
    if let Err(error) = validate_driver_config_scope(&state, normalized_key.as_deref(), &updated) {
        return ApiError::validation(error);
    }

    manager.update(updated);
    if let Err(e) = manager.save() {
        return ApiError::internal(format!("Failed to persist config: {e}"));
    }

    let audio_live_applied = maybe_apply_audio_config_change(
        &state,
        normalized_key.as_deref(),
        body.live.unwrap_or(false),
    )
    .await;
    let render_live_applied =
        maybe_apply_render_config_change(&state, normalized_key.as_deref()).await;
    let live_applied = audio_live_applied || render_live_applied;

    ApiResponse::ok(serde_json::json!({
        "key": normalized_key,
        "reset": true,
        "live": live_applied,
        "path": manager.path().display().to_string(),
    }))
}

fn config_snapshot(state: &AppState) -> HypercolorConfig {
    if let Some(manager) = state.config_manager.as_ref() {
        let current = manager.get();
        (**current).clone()
    } else {
        HypercolorConfig::default()
    }
}

fn validate_driver_config_scope(
    state: &AppState,
    key: Option<&str>,
    config: &HypercolorConfig,
) -> Result<(), String> {
    let driver_ids = match key {
        None | Some("drivers") => state.driver_registry.ids(),
        Some(value) => value
            .strip_prefix("drivers.")
            .and_then(|rest| rest.split('.').next())
            .filter(|driver_id| !driver_id.is_empty())
            .map_or_else(Vec::new, |driver_id| vec![driver_id.to_owned()]),
    };

    for driver_id in driver_ids {
        let Some(driver) = state.driver_registry.get(&driver_id) else {
            continue;
        };
        let Some(provider) = driver.config() else {
            continue;
        };
        let entry = config.drivers.get(&driver_id).cloned().unwrap_or_default();
        provider.validate_config(&entry).map_err(|error| {
            format!("Config update failed validation for 'drivers.{driver_id}': {error}")
        })?;
    }

    Ok(())
}

fn normalize_config_key(key: &str) -> String {
    match key {
        "effect_engine.render_acceleration_mode" => {
            "effect_engine.compositor_acceleration_mode".to_owned()
        }
        _ => key.to_owned(),
    }
}

fn get_json_path<'a>(value: &'a serde_json::Value, key: &str) -> Option<&'a serde_json::Value> {
    let mut cursor = value;
    for part in key.split('.') {
        cursor = cursor.get(part)?;
    }
    Some(cursor)
}

fn set_json_path(root: &mut serde_json::Value, key: &str, value: serde_json::Value) -> bool {
    let mut cursor = root;
    let mut parts = key.split('.').peekable();

    while let Some(part) = parts.next() {
        if parts.peek().is_none() {
            let Some(obj) = cursor.as_object_mut() else {
                return false;
            };
            obj.insert(part.to_owned(), value);
            return true;
        }

        let Some(obj) = cursor.as_object_mut() else {
            return false;
        };
        cursor = obj
            .entry(part.to_owned())
            .or_insert_with(|| serde_json::json!({}));
    }

    false
}

fn should_reconfigure_audio_inputs(key: Option<&str>) -> bool {
    key.is_none_or(|value| value == "audio" || value.starts_with("audio."))
}

async fn maybe_apply_audio_config_change(
    state: &Arc<AppState>,
    key: Option<&str>,
    live_requested: bool,
) -> bool {
    if !should_reconfigure_audio_inputs(key) {
        return false;
    }

    if !live_requested {
        info!(
            key = key.unwrap_or("<all>"),
            "Persisted audio config change without live apply; restart the daemon to activate it"
        );
        return false;
    }

    info!(
        key = key.unwrap_or("<all>"),
        "Applying live audio config change"
    );

    match reconfigure_input_manager(state).await {
        Ok(()) => true,
        Err(error) => {
            warn!(
                key = key.unwrap_or("<all>"),
                %error,
                "Failed to apply live audio config; change will take effect after daemon restart"
            );
            false
        }
    }
}

async fn reconfigure_input_manager(state: &Arc<AppState>) -> anyhow::Result<()> {
    let Some(manager) = state.config_manager.as_ref() else {
        return Ok(());
    };

    let latest_config = manager.get();
    let capture_active = current_live_audio_capture_demand(state).await;
    let mut input_manager = state.input_manager.lock().await;
    let previous_sources = input_manager.source_names();
    let audio_device = latest_config.audio.device.clone();
    let effective_config = audio_pipeline_config(latest_config.as_ref());
    let replacement_sources = if latest_config.audio.enabled {
        vec![format!("AudioInput({audio_device})")]
    } else {
        Vec::new()
    };

    info!(
        audio_enabled = latest_config.audio.enabled,
        audio_device = %audio_device,
        capture_active,
        previous_sources = ?previous_sources,
        replacement_sources = ?replacement_sources,
        "Applying targeted live audio config change"
    );

    input_manager.apply_audio_runtime_config(
        latest_config.audio.enabled,
        &effective_config,
        &format!("AudioInput({audio_device})"),
        capture_active,
    )?;

    info!(
        audio_device = %audio_device,
        sources = ?input_manager.source_names(),
        "Live audio config change applied"
    );
    Ok(())
}

async fn current_live_audio_capture_demand(state: &Arc<AppState>) -> bool {
    let power_state = *state.power_state.borrow();
    if power_state.sleeping {
        return false;
    }

    let active_effect_ids = {
        let scene_manager = state.scene_manager.read().await;
        scene_manager
            .active_render_groups()
            .iter()
            .filter(|group| group.enabled)
            .filter_map(|group| group.effect_id)
            .collect::<Vec<_>>()
    };
    if active_effect_ids.is_empty() {
        return false;
    }

    let registry = state.effect_registry.read().await;
    if active_effect_ids.into_iter().any(|effect_id| {
        registry
            .get(&effect_id)
            .is_some_and(|entry| entry.metadata.audio_reactive)
    }) {
        return true;
    }

    false
}

fn audio_pipeline_config(config: &HypercolorConfig) -> AudioPipelineConfig {
    AudioPipelineConfig {
        source: audio_source_from_device(&config.audio.device, config.audio.enabled),
        fft_size: usize::try_from(config.audio.fft_size).unwrap_or(1024),
        smoothing: config.audio.smoothing.clamp(0.0, 1.0),
        gain: 1.0,
        noise_floor: noise_gate_to_db(config.audio.noise_gate),
        beat_sensitivity: config.audio.beat_sensitivity.max(0.01),
    }
}

fn audio_source_from_device(device: &str, enabled: bool) -> AudioSourceType {
    if !enabled {
        return AudioSourceType::None;
    }

    let normalized = device.trim();
    if normalized.eq_ignore_ascii_case("none") {
        AudioSourceType::None
    } else if normalized.eq_ignore_ascii_case("default") {
        AudioSourceType::SystemMonitor
    } else if normalized.eq_ignore_ascii_case("microphone") {
        AudioSourceType::Microphone
    } else {
        AudioSourceType::Named(normalized.to_owned())
    }
}

fn noise_gate_to_db(noise_gate: f32) -> f32 {
    let linear = noise_gate.clamp(0.000_001, 1.0);
    20.0 * linear.log10()
}

fn should_reconfigure_render(key: Option<&str>) -> bool {
    matches!(
        key,
        Some("daemon.target_fps" | "daemon.canvas_width" | "daemon.canvas_height")
    )
}

/// Apply render config changes live: FPS retune and canvas resize.
///
/// FPS changes go directly to the `RenderLoop`. Canvas dimension changes
/// are pushed as a `SceneTransaction::ResizeCanvas` and take effect at
/// the next frame boundary without blocking the pipeline.
async fn maybe_apply_render_config_change(state: &Arc<AppState>, key: Option<&str>) -> bool {
    if !should_reconfigure_render(key) {
        return false;
    }

    let Some(manager) = state.config_manager.as_ref() else {
        return false;
    };

    let config = manager.get();
    let mut applied = false;

    if key.is_none_or(|k| k == "daemon.target_fps") {
        let tier = FpsTier::from_fps(config.daemon.target_fps);
        state.configured_max_fps_tier.set(tier);
        let mut loop_guard = state.render_loop.write().await;
        loop_guard.fps_controller_mut().set_max_tier(tier);
        loop_guard.set_tier(tier);
        info!(
            target_fps = config.daemon.target_fps,
            resolved_tier = %tier,
            "Applied live render FPS change"
        );
        applied = true;
    }

    if key.is_none_or(|k| k == "daemon.canvas_width" || k == "daemon.canvas_height") {
        let resize_queued = sync_active_layout_canvas_size(
            state,
            config.daemon.canvas_width,
            config.daemon.canvas_height,
        )
        .await;
        info!(
            canvas_width = config.daemon.canvas_width,
            canvas_height = config.daemon.canvas_height,
            resize_queued,
            "Applied live canvas dimension config"
        );
        applied = true;
    }

    applied
}

const fn canvas_dimensions_differ(
    current_width: u32,
    current_height: u32,
    next_width: u32,
    next_height: u32,
) -> bool {
    current_width != next_width || current_height != next_height
}

async fn sync_active_layout_canvas_size(state: &Arc<AppState>, width: u32, height: u32) -> bool {
    let updated_layout = {
        let spatial = state.spatial_engine.read().await;
        let current = spatial.layout().as_ref().clone();
        if !canvas_dimensions_differ(current.canvas_width, current.canvas_height, width, height) {
            None
        } else {
            let mut updated = current;
            updated.canvas_width = width;
            updated.canvas_height = height;
            Some(updated)
        }
    };

    let Some(updated_layout) = updated_layout else {
        return false;
    };

    apply_layout_update(
        &state.spatial_engine,
        &state.scene_manager,
        &state.scene_transactions,
        updated_layout.clone(),
    )
    .await;

    let persisted_layout_updated = {
        let mut layouts = state.layouts.write().await;
        if let Some(saved_layout) = layouts.get_mut(&updated_layout.id) {
            saved_layout.canvas_width = width;
            saved_layout.canvas_height = height;
            true
        } else {
            false
        }
    };

    if persisted_layout_updated {
        crate::api::persist_layouts(state).await;
    }

    true
}

#[cfg(test)]
mod tests {
    use super::canvas_dimensions_differ;

    #[test]
    fn canvas_dimensions_differ_only_when_size_changes() {
        assert!(!canvas_dimensions_differ(800, 600, 800, 600));
        assert!(canvas_dimensions_differ(800, 600, 801, 600));
        assert!(canvas_dimensions_differ(800, 600, 800, 601));
    }
}
