//! Config endpoints — `/api/v1/config*`.

use std::sync::Arc;

use anyhow::Context;
use axum::Json;
use axum::extract::{Query, State};
use axum::response::Response;
use serde::Deserialize;
use tracing::{info, warn};
use utoipa::ToSchema;

use hypercolor_core::config::canonical_audio_device_id;
use hypercolor_core::engine::FpsTier;
use hypercolor_core::input::{
    AudioReconfigurationConflict, InputSource, ScreenReconfigurationConflict, SourceKind,
    SourceState,
};
use hypercolor_types::audio::{AudioPipelineConfig, AudioSourceType};
use hypercolor_types::config::{CaptureConfig, HypercolorConfig};

use crate::api::AppState;
use crate::api::envelope::{ApiError, ApiResponse};
use crate::scene_transactions::{
    PreparedLayoutUpdate, SceneTransaction, apply_prepared_layout_update_under_guard,
};

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

    let current_snapshot = Arc::clone(&manager.get());
    let current = (*current_snapshot).clone();
    let mut root = match serde_json::to_value(&current) {
        Ok(v) => v,
        Err(e) => return ApiError::internal(format!("Failed to serialize config: {e}")),
    };

    let key = normalize_config_key(&body.key);
    let parsed_value = serde_json::from_str::<serde_json::Value>(&body.value)
        .unwrap_or_else(|_| serde_json::Value::String(body.value.clone()));
    let parsed_value = canonicalize_config_value(&key, parsed_value);

    let value_is_unchanged =
        get_json_path(&root, &key).is_some_and(|current| current == &parsed_value);
    let capture_runtime_matches = if value_is_unchanged && should_reconfigure_capture(Some(&key)) {
        capture_runtime_matches(&state, &current_snapshot).await
    } else {
        true
    };

    if value_is_unchanged && capture_runtime_matches {
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
    if let Err(error) = updated.capture.validate() {
        return ApiError::validation(error.to_string());
    }

    if should_reconfigure_capture(Some(&key)) {
        match apply_capture_config_transaction(&state, &current_snapshot, updated.capture.clone())
            .await
        {
            Ok(()) => {
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
                        "Canonicalized config is missing expected key: {key}"
                    ));
                };
                return ApiResponse::ok(serde_json::json!({
                    "key": key,
                    "value": effective_value,
                    "live": true,
                    "path": manager.path().display().to_string(),
                }));
            }
            Err(CaptureConfigTransactionError::Conflict) => {
                return ApiError::conflict(
                    "Capture config changed while its live runtime was prepared; retry the update",
                );
            }
            Err(CaptureConfigTransactionError::Prepare(error)) => {
                return ApiError::validation(format!(
                    "Failed to prepare live screen capture config: {error}"
                ));
            }
            Err(CaptureConfigTransactionError::Persist(error)) => {
                return ApiError::internal(format!("Failed to persist config: {error}"));
            }
            Err(CaptureConfigTransactionError::Commit(error)) => {
                return ApiError::conflict(format!(
                    "Screen capture graph changed during live apply: {error}"
                ));
            }
        }
    }

    // Re-apply the validated key against the freshest config under the
    // manager's write lock, so a concurrent targeted writer (e.g. the
    // capture restore-token sink) is not clobbered by this handler's
    // earlier snapshot.
    manager.modify(|config| {
        let reapplied = serde_json::to_value(&*config).ok().and_then(|mut root| {
            set_json_path(&mut root, &key, parsed_value.clone())
                .then(|| serde_json::from_value::<HypercolorConfig>(root).ok())
                .flatten()
        });
        *config = reapplied.unwrap_or_else(|| updated.clone());
    });
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
    let input_live_applied = maybe_apply_input_config_change(&state, Some(&key)).await;
    let live_applied = audio_live_applied || render_live_applied || input_live_applied;

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

    let current_snapshot = Arc::clone(&manager.get());
    let mut current = match serde_json::to_value(&*current_snapshot) {
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
    if let Err(error) = updated.capture.validate() {
        return ApiError::validation(error.to_string());
    }

    let capture_live_applied = if normalized_key
        .as_deref()
        .is_none_or(|key| should_reconfigure_capture(Some(key)))
    {
        match apply_capture_config_transaction(&state, &current_snapshot, updated.capture.clone())
            .await
        {
            Ok(()) => true,
            Err(CaptureConfigTransactionError::Conflict) => {
                return ApiError::conflict(
                    "Capture config changed while its live runtime was prepared; retry the reset",
                );
            }
            Err(CaptureConfigTransactionError::Prepare(error)) => {
                return ApiError::validation(format!(
                    "Failed to prepare live screen capture config: {error}"
                ));
            }
            Err(CaptureConfigTransactionError::Persist(error)) => {
                return ApiError::internal(format!("Failed to persist config: {error}"));
            }
            Err(CaptureConfigTransactionError::Commit(error)) => {
                return ApiError::conflict(format!(
                    "Screen capture graph changed during live apply: {error}"
                ));
            }
        }
    } else {
        false
    };

    if normalized_key
        .as_deref()
        .is_some_and(|key| should_reconfigure_capture(Some(key)))
    {
        return ApiResponse::ok(serde_json::json!({
            "key": normalized_key,
            "reset": true,
            "live": true,
            "path": manager.path().display().to_string(),
        }));
    }

    // Keyed resets re-apply the default at the key against the freshest
    // config under the write lock (same race protection as set); a full
    // reset replaces wholesale by design.
    let reset_key = normalized_key.clone();
    manager.modify(move |config| {
        let reapplied = reset_key.as_deref().and_then(|key| {
            let default_value = serde_json::to_value(HypercolorConfig::default())
                .ok()
                .and_then(|defaults| get_json_path(&defaults, key).cloned())?;
            let mut root = serde_json::to_value(&*config).ok()?;
            set_json_path(&mut root, key, default_value)
                .then(|| serde_json::from_value::<HypercolorConfig>(root).ok())
                .flatten()
        });
        *config = reapplied.unwrap_or(updated);
    });
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
    let input_live_applied =
        maybe_apply_input_config_change(&state, normalized_key.as_deref()).await;
    let live_applied =
        audio_live_applied || render_live_applied || capture_live_applied || input_live_applied;

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

    let mut conflict_count = 0_u64;
    loop {
        let latest_config = Arc::clone(&manager.get());
        let capture_active = current_live_audio_capture_demand(state).await;
        let audio_device = latest_config.audio.device.clone();
        let audio_name = format!("AudioInput({audio_device})");
        let effective_config = audio_pipeline_config(latest_config.as_ref());
        let (plan, previous_sources) = {
            let input_manager = state.input_manager.lock().await;
            (
                input_manager.plan_audio_runtime_config(
                    latest_config.audio.enabled,
                    &effective_config,
                    &audio_name,
                    capture_active,
                )?,
                input_manager.source_names(),
            )
        };
        let replacement_sources = if latest_config.audio.enabled {
            vec![audio_name]
        } else {
            Vec::new()
        };

        info!(
            audio_enabled = latest_config.audio.enabled,
            audio_device = %audio_device,
            capture_active,
            conflict_count,
            previous_sources = ?previous_sources,
            replacement_sources = ?replacement_sources,
            "Applying targeted live audio config change"
        );

        let mut prepared = tokio::task::spawn_blocking(move || plan.prepare())
            .await
            .context("audio reconfiguration preparation task failed")??;
        if !manager.is_current(&latest_config) {
            anyhow::bail!("audio config changed while live reconfiguration was prepared");
        }
        let mut input_manager = state.input_manager.lock().await;
        match input_manager.commit_audio_runtime_config(&mut prepared) {
            Ok(retirement) => {
                let sources = input_manager.source_names();
                drop(input_manager);
                drop(prepared);
                retirement.retire();
                info!(
                    audio_device = %audio_device,
                    conflict_count,
                    sources = ?sources,
                    "Live audio config change applied"
                );
                return Ok(());
            }
            Err(error)
                if error
                    .downcast_ref::<AudioReconfigurationConflict>()
                    .is_some()
                    && manager.is_current(&latest_config) =>
            {
                conflict_count = conflict_count.saturating_add(1);
                drop(input_manager);
                drop(prepared);
            }
            Err(error) => {
                drop(input_manager);
                drop(prepared);
                return Err(error);
            }
        }
    }
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

fn should_reconfigure_capture(key: Option<&str>) -> bool {
    key.is_none_or(|value| value == "capture" || value.starts_with("capture."))
}

#[derive(Debug, thiserror::Error)]
enum CaptureConfigTransactionError {
    #[error("capture config identity changed during preparation")]
    Conflict,
    #[error(transparent)]
    Prepare(anyhow::Error),
    #[error(transparent)]
    Persist(anyhow::Error),
    #[error(transparent)]
    Commit(ScreenReconfigurationConflict),
}

async fn apply_capture_config_transaction(
    state: &Arc<AppState>,
    expected_config: &Arc<HypercolorConfig>,
    capture: CaptureConfig,
) -> Result<(), CaptureConfigTransactionError> {
    let Some(manager) = state.config_manager.as_ref() else {
        return Err(CaptureConfigTransactionError::Prepare(anyhow::anyhow!(
            "config manager unavailable"
        )));
    };
    #[cfg(target_os = "windows")]
    let (plan, capacity_plan, capacity_preparation, admission_coordinator) = {
        let input_manager = state.input_manager.lock().await;
        let installed_capacity = input_manager.screen_resource_capacity();
        let plan = input_manager.plan_screen_runtime_config(capture.enabled);
        let capacity_plan = crate::startup::services::screen_capacity_plan_for_backend(
            &capture,
            installed_capacity.backend_capacity(),
        )
        .map_err(CaptureConfigTransactionError::Prepare)?;
        let analysis = crate::startup::services::screen_analysis_plan_for_demand(
            &capture,
            plan.capture_demand(),
            capacity_plan.total_capacity(),
        )
        .map_err(CaptureConfigTransactionError::Prepare)?;
        let capacity_preparation = input_manager
            .prepare_screen_capacity_plan(
                capacity_plan.total_capacity(),
                analysis.map_or(
                    0,
                    hypercolor_core::input::screen::ScreenAnalysisResourcePlan::peak_bytes,
                ),
            )
            .map_err(|error| CaptureConfigTransactionError::Prepare(anyhow::anyhow!(error)))?
            .ok_or_else(|| {
                CaptureConfigTransactionError::Prepare(anyhow::anyhow!(
                    "screen capacity admission is not installed"
                ))
            })?;
        (
            plan,
            capacity_plan,
            capacity_preparation,
            input_manager.screen_admission_coordinator(),
        )
    };
    #[cfg(not(target_os = "windows"))]
    let plan = {
        let input_manager = state.input_manager.lock().await;
        input_manager.plan_screen_runtime_config(capture.enabled)
    };
    let (mut replacement, persistence) = if plan.enabled() {
        #[cfg(target_os = "windows")]
        let (mut source, persistence) =
            crate::startup::services::prepare_platform_screen_capture_source(
                &capture,
                Arc::clone(manager),
                expected_config,
                admission_coordinator,
                capacity_plan.total_capacity(),
            )
            .map_err(CaptureConfigTransactionError::Prepare)?;
        #[cfg(not(target_os = "windows"))]
        let (mut source, persistence) =
            crate::startup::services::prepare_platform_screen_capture_source(
                &capture,
                Arc::clone(manager),
                expected_config,
            )
            .map_err(CaptureConfigTransactionError::Prepare)?;
        source.set_source_graph_generation(plan.replacement_source_graph_generation());
        source
            .set_screen_capture_demand(plan.capture_demand())
            .map_err(CaptureConfigTransactionError::Prepare)?;
        let source = Some(
            tokio::task::spawn_blocking(move || {
                source.start()?;
                Ok::<_, anyhow::Error>(source)
            })
            .await
            .map_err(|error| {
                CaptureConfigTransactionError::Prepare(anyhow::anyhow!(
                    "capture preparation task failed: {error}"
                ))
            })?
            .map_err(CaptureConfigTransactionError::Prepare)?,
        );
        (source, Some(persistence))
    } else {
        (None, None)
    };
    if plan.capture_demand().is_active()
        && let Some(status) = replacement
            .as_ref()
            .and_then(|source| source.source_status_handle())
        && let Err(error) = validate_prepared_capture_status(status).await
    {
        if let Some(persistence) = &persistence {
            persistence.revoke();
        }
        stop_prepared_capture_source(replacement).await;
        return Err(CaptureConfigTransactionError::Prepare(error));
    }

    let mut input_manager = state.input_manager.lock().await;
    if let Err(error) = input_manager.validate_screen_runtime_config(&plan, &replacement) {
        if let Some(persistence) = &persistence {
            persistence.revoke();
        }
        drop(input_manager);
        stop_prepared_capture_source(replacement).await;
        return Err(CaptureConfigTransactionError::Commit(error));
    }
    #[cfg(target_os = "windows")]
    if let Err(error) = input_manager.validate_screen_capacity(&capacity_preparation) {
        if let Some(persistence) = &persistence {
            persistence.revoke();
        }
        drop(input_manager);
        stop_prepared_capture_source(replacement).await;
        return Err(CaptureConfigTransactionError::Commit(error));
    }
    let persistence_result = if let Some(persistence) = &persistence {
        manager.save_capture_and_activate_if_current(
            expected_config,
            persistence.epoch(),
            persistence.source_identity(),
            capture.clone(),
        )
    } else {
        manager
            .modify_and_save_if_current(expected_config, |config| {
                config.capture.clone_from(&capture);
            })
            .map(|saved| saved.then(|| Arc::clone(&manager.get())))
    };
    match persistence_result {
        Ok(Some(_)) => {}
        Ok(None) => {
            if let Some(persistence) = &persistence {
                persistence.revoke();
            }
            drop(input_manager);
            stop_prepared_capture_source(replacement).await;
            return Err(CaptureConfigTransactionError::Conflict);
        }
        Err(error) => {
            if let Some(persistence) = &persistence {
                persistence.revoke();
            }
            drop(input_manager);
            stop_prepared_capture_source(replacement).await;
            return Err(CaptureConfigTransactionError::Persist(error));
        }
    }

    #[cfg(target_os = "windows")]
    input_manager
        .commit_screen_capacity(capacity_preparation)
        .expect("screen capacity was validated under the same input-manager lock");
    let retirement = input_manager
        .commit_screen_runtime_config(&plan, &mut replacement)
        .expect("screen runtime plan was validated under the same input-manager lock");
    if !capture.enabled {
        input_manager.remove_screen_sources();
    }
    manager.mark_capture_runtime_applied(&capture);
    drop(input_manager);

    if let Err(error) = tokio::task::spawn_blocking(move || retirement.retire()).await {
        warn!(%error, "Detached capture source retirement task failed");
    }
    if let Some(persistence) = persistence
        && let Err(error) = tokio::task::spawn_blocking(move || persistence.commit()).await
    {
        warn!(%error, "Capture identity persistence task failed");
    }
    if let Err(error) = state
        .scene_transactions
        .push(SceneTransaction::SetScreenCaptureConfigured(
            capture.enabled,
        ))
    {
        warn!(%error, "Render pipeline stopped before capture state publication");
    }
    info!(
        enabled = capture.enabled,
        "Applied live screen capture config"
    );
    Ok(())
}

async fn validate_prepared_capture_status(
    status: hypercolor_core::input::SourceStatusHandle,
) -> anyhow::Result<()> {
    let mut subscription = status.subscribe();
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(500);
    loop {
        let snapshot = subscription.snapshot();
        match snapshot.state {
            SourceState::Live => return Ok(()),
            SourceState::Degraded if snapshot.resource_count > 0 => return Ok(()),
            SourceState::Starting => {}
            _ => {
                anyhow::bail!(
                    "{}",
                    snapshot.issue.as_ref().map_or_else(
                        || format!("capture source is not usable ({:?})", snapshot.state),
                        |issue| issue.message.to_string()
                    )
                );
            }
        }
        match tokio::time::timeout_at(deadline, subscription.changed()).await {
            Ok(Some(_)) => {}
            Ok(None) => anyhow::bail!("capture source status closed before becoming usable"),
            Err(_) => anyhow::bail!("capture source did not become usable within 500ms"),
        }
    }
}

async fn capture_runtime_matches(
    state: &Arc<AppState>,
    expected_config: &Arc<HypercolorConfig>,
) -> bool {
    let Some(manager) = state.config_manager.as_ref() else {
        return false;
    };
    let input_manager = state.input_manager.lock().await;
    if !manager.is_current(expected_config)
        || !manager.capture_runtime_matches(&expected_config.capture)
    {
        return false;
    }
    let registry = input_manager.source_status_registry();
    let statuses = registry.snapshot().statuses();
    capture_statuses_match(&expected_config.capture, &statuses)
}

fn capture_statuses_match(
    capture: &CaptureConfig,
    statuses: &[Arc<hypercolor_core::input::SourceStatus>],
) -> bool {
    let mut screen = statuses
        .iter()
        .filter(|status| status.kind == SourceKind::Screen && !status.retired);
    let first = screen.next();
    if !capture.enabled {
        return first.is_none();
    }
    let Some(status) = first else {
        return false;
    };
    if screen.next().is_some() || !status.configured || !status.consented {
        return false;
    }
    matches!(status.state, SourceState::Live)
        || matches!(status.state, SourceState::Degraded) && status.resource_count > 0
        || matches!(status.state, SourceState::Stopped) && !status.demanded
}

async fn stop_prepared_capture_source(source: Option<Box<dyn InputSource>>) {
    let Some(mut source) = source else {
        return;
    };
    let _ = tokio::task::spawn_blocking(move || source.stop()).await;
}

fn should_reconfigure_input(key: Option<&str>) -> bool {
    key.is_none_or(|value| value == "input" || value.starts_with("input."))
}

/// Apply host-input config changes live.
///
/// Enable/disable adds or removes the interaction source on the running
/// input manager. Activation converges on the next frame through the
/// uncached interaction demand reconcile, so a source added while an
/// interactive effect is already running starts capturing immediately.
async fn maybe_apply_input_config_change(state: &Arc<AppState>, key: Option<&str>) -> bool {
    if !should_reconfigure_input(key) {
        return false;
    }

    let Some(manager) = state.config_manager.as_ref() else {
        return false;
    };

    let input = manager.get().input.clone();
    let route_snapshot = state.interaction_routing.snapshot();
    let route_changed = route_snapshot.daemon_policy != input.daemon_route
        || route_snapshot.preview_policy != input.preview_route;
    if route_changed {
        state.interaction_routing.publish_policies(
            route_snapshot
                .config_generation
                .checked_add(1)
                .expect("interaction route config generation exhausted"),
            input.daemon_route,
            input.preview_route,
        );
    }
    if matches!(key, Some("input.daemon_route" | "input.preview_route")) {
        return route_changed;
    }

    let mut input_manager = state.input_manager.lock().await;
    // Only the host hardware source is consent-gated; the browser injection
    // source is always registered and must survive enable/disable toggles.
    let had_source = input_manager.has_host_capture_source();
    let replacement = crate::startup::services::build_interaction_source(&input);

    // Rebuild on any change so keyboard/mouse toggles apply, not just enable
    // and disable.
    input_manager.remove_host_capture_sources();
    let Some(mut source) = replacement else {
        if had_source {
            info!("Disabled host input capture live");
        }
        return had_source || route_changed;
    };

    if let Err(error) = source.start() {
        warn!(%error, "Failed to start live host input source");
        return had_source || route_changed;
    }
    input_manager.add_source(source);
    info!("Applied live host input capture config");
    true
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
/// are queued as an acknowledged layout transaction and take effect at the
/// next frame boundary without blocking the pipeline.
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
    let state = Arc::clone(state);
    match tokio::spawn(sync_active_layout_canvas_size_workflow(
        state, width, height,
    ))
    .await
    {
        Ok(applied) => applied,
        Err(error) => {
            warn!(%error, width, height, "Live canvas dimension workflow failed");
            false
        }
    }
}

async fn sync_active_layout_canvas_size_workflow(
    state: Arc<AppState>,
    width: u32,
    height: u32,
) -> bool {
    #[cfg(feature = "persistence-test-hooks")]
    let mutation_reference = format!("{width}x{height}");
    #[cfg(feature = "persistence-test-hooks")]
    state
        .layout_mutation_test_hooks
        .wait(
            crate::api::layouts::LayoutMutationTestPoint::BeforeGuard,
            crate::api::layouts::LayoutMutationTestOperation::ConfigResize,
            &mutation_reference,
        )
        .await;
    let guard = state.scene_transactions.acquire_layout_update_guard().await;
    let updated_layout = {
        let spatial = state.spatial_engine.read().await;
        let current = spatial.layout().as_ref().clone();
        if canvas_dimensions_differ(current.canvas_width, current.canvas_height, width, height) {
            let mut updated = current;
            updated.canvas_width = width;
            updated.canvas_height = height;
            Some(updated)
        } else {
            None
        }
    };

    let Some(updated_layout) = updated_layout else {
        return false;
    };

    let prepared = match PreparedLayoutUpdate::try_new(updated_layout.clone()) {
        Ok(prepared) => prepared,
        Err(error) => {
            warn!(%error, width, height, "Rejected live canvas dimension config");
            return false;
        }
    };
    if let Err(error) = apply_prepared_layout_update_under_guard(
        Arc::clone(&state.spatial_engine),
        Arc::clone(&state.scene_manager),
        state.scene_transactions.clone(),
        &guard,
        prepared,
    )
    .await
    {
        warn!(%error, width, height, "Rejected live canvas dimension config");
        return false;
    }

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

    #[cfg(feature = "persistence-test-hooks")]
    state
        .layout_mutation_test_hooks
        .wait(
            crate::api::layouts::LayoutMutationTestPoint::AfterMemoryMutation,
            crate::api::layouts::LayoutMutationTestOperation::ConfigResize,
            &mutation_reference,
        )
        .await;
    if persisted_layout_updated {
        crate::api::persist_layouts_best_effort(&state).await;
    }
    #[cfg(feature = "persistence-test-hooks")]
    state
        .layout_mutation_test_hooks
        .wait(
            crate::api::layouts::LayoutMutationTestPoint::AfterWorkflow,
            crate::api::layouts::LayoutMutationTestOperation::ConfigResize,
            &mutation_reference,
        )
        .await;
    drop(guard);

    true
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    use hypercolor_core::config::ConfigManager;
    #[cfg(target_os = "windows")]
    use hypercolor_core::input::screen::ScreenAdmissionCapacity;
    use hypercolor_core::input::screen::{PixelExtent, ScreenCaptureDemand};
    use hypercolor_core::input::{
        InputData, InputManager, InputSource, ScreenReconfigurationConflict, SourceIssue,
        SourceKind, SourceState, SourceStatus, SourceStatusHandle, SourceStatusReporter,
    };
    use hypercolor_types::config::InteractionRoutePolicy;

    use super::{
        CaptureConfigTransactionError, SetConfigRequest, apply_capture_config_transaction,
        canvas_dimensions_differ, capture_statuses_match, maybe_apply_input_config_change,
        set_config_value, validate_prepared_capture_status,
    };
    use crate::api::AppState;

    struct TestScreenSource {
        running: bool,
        demand: ScreenCaptureDemand,
        stopped: Arc<AtomicBool>,
    }

    impl TestScreenSource {
        fn new(stopped: Arc<AtomicBool>) -> Self {
            Self {
                running: false,
                demand: ScreenCaptureDemand::Inactive,
                stopped,
            }
        }
    }

    impl InputSource for TestScreenSource {
        fn name(&self) -> &'static str {
            "test_screen"
        }

        fn start(&mut self) -> anyhow::Result<()> {
            self.running = true;
            Ok(())
        }

        fn stop(&mut self) {
            self.running = false;
            self.stopped.store(true, Ordering::Release);
        }

        fn sample(&mut self) -> anyhow::Result<InputData> {
            Ok(InputData::None)
        }

        fn is_running(&self) -> bool {
            self.running
        }

        fn is_screen_source(&self) -> bool {
            true
        }

        fn screen_capture_demand(&self) -> ScreenCaptureDemand {
            self.demand
        }

        fn set_screen_capture_demand(&mut self, demand: ScreenCaptureDemand) -> anyhow::Result<()> {
            self.demand = demand;
            Ok(())
        }
    }

    fn test_screen_demand() -> ScreenCaptureDemand {
        ScreenCaptureDemand::active(
            PixelExtent::new(640, 480).expect("test screen extent should be non-empty"),
        )
    }

    fn screen_status(state: SourceState, resource_count: usize) -> Arc<SourceStatus> {
        screen_status_handle(state, resource_count).snapshot()
    }

    fn screen_status_handle(state: SourceState, resource_count: usize) -> SourceStatusHandle {
        let mut reporter =
            SourceStatusReporter::new("test-screen", SourceKind::Screen, "test", true, true, true);
        reporter.set_source_graph_generation(1);
        if state == SourceState::Stopped {
            return reporter.handle();
        }
        let session = reporter
            .begin_session()
            .expect("test source session begins")
            .expect("manager-bound source creates a session");
        match state {
            SourceState::Starting => {}
            SourceState::Live => {
                session.mark_event_driven_live_without_deadline(resource_count);
            }
            SourceState::Degraded => {
                session.degraded_with_resources(
                    SourceIssue::new("test_degraded", "reduced capture", true),
                    resource_count,
                );
            }
            SourceState::Unavailable => {
                session.unavailable(SourceIssue::new(
                    "test_unavailable",
                    "capture unavailable",
                    true,
                ));
            }
            SourceState::Failed => {
                session.failed(SourceIssue::new("test_failed", "capture failed", false));
            }
            SourceState::Stopped => unreachable!("stopped status returned before session start"),
        }
        reporter.handle()
    }

    fn starting_screen_status() -> SourceStatusHandle {
        let mut reporter =
            SourceStatusReporter::new("test-screen", SourceKind::Screen, "test", true, true, true);
        reporter.set_source_graph_generation(1);
        reporter
            .begin_session()
            .expect("test source session begins")
            .expect("manager-bound source creates a session");
        reporter.handle()
    }

    #[test]
    fn canvas_dimensions_differ_only_when_size_changes() {
        assert!(!canvas_dimensions_differ(800, 600, 800, 600));
        assert!(canvas_dimensions_differ(800, 600, 801, 600));
        assert!(canvas_dimensions_differ(800, 600, 800, 601));
    }

    #[tokio::test]
    async fn route_only_input_config_changes_publish_without_rebuilding_sources() {
        let mut state = AppState::new();
        let config_path = std::env::temp_dir().join(format!(
            "hypercolor-route-config-{}-{}.toml",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system clock should follow Unix epoch")
                .as_nanos()
        ));
        let manager = Arc::new(
            ConfigManager::new(config_path).expect("test config manager should initialize"),
        );
        state.config_manager = Some(Arc::clone(&manager));
        let state = Arc::new(state);
        let graph_generation = state.input_manager.lock().await.source_graph_generation();

        manager.modify(|config| config.input.daemon_route = InteractionRoutePolicy::Merge);
        assert!(maybe_apply_input_config_change(&state, Some("input.daemon_route")).await);
        let first = state.interaction_routing.snapshot();
        assert_eq!(first.daemon_policy, InteractionRoutePolicy::Merge);
        assert_eq!(first.config_generation, 2);

        manager.modify(|config| config.input.preview_route = InteractionRoutePolicy::Host);
        assert!(maybe_apply_input_config_change(&state, Some("input.preview_route")).await);
        let second = state.interaction_routing.snapshot();
        assert_eq!(second.preview_policy, InteractionRoutePolicy::Host);
        assert_eq!(second.config_generation, 3);
        assert_eq!(
            state.input_manager.lock().await.source_graph_generation(),
            graph_generation
        );
    }

    #[tokio::test]
    async fn demanded_starting_capture_times_out_instead_of_committing() {
        let error = validate_prepared_capture_status(starting_screen_status())
            .await
            .expect_err("starting capture must become usable before commit");

        assert!(error.to_string().contains("within 500ms"));
    }

    #[tokio::test]
    async fn demanded_degraded_capture_commits_only_with_usable_resources() {
        validate_prepared_capture_status(screen_status_handle(SourceState::Degraded, 1))
            .await
            .expect("degraded capture with resources is usable");

        let error =
            validate_prepared_capture_status(screen_status_handle(SourceState::Degraded, 0))
                .await
                .expect_err("degraded capture without resources is unusable");
        assert!(error.to_string().contains("reduced capture"));
    }

    #[test]
    fn capture_runtime_health_rejects_missing_stopped_failed_and_extra_sources() {
        let mut capture = hypercolor_types::config::CaptureConfig {
            enabled: true,
            ..hypercolor_types::config::CaptureConfig::default()
        };
        assert!(!capture_statuses_match(&capture, &[]));
        assert!(!capture_statuses_match(
            &capture,
            &[screen_status(SourceState::Stopped, 0)]
        ));
        assert!(!capture_statuses_match(
            &capture,
            &[screen_status(SourceState::Failed, 0)]
        ));
        assert!(capture_statuses_match(
            &capture,
            &[screen_status(SourceState::Degraded, 1)]
        ));

        capture.enabled = false;
        assert!(!capture_statuses_match(
            &capture,
            &[
                screen_status(SourceState::Stopped, 0),
                screen_status(SourceState::Stopped, 0),
            ]
        ));
    }

    #[test]
    fn capture_runtime_fingerprint_rejects_divergent_config() {
        let tempdir = tempfile::tempdir().expect("temporary config directory should build");
        let manager = ConfigManager::new(tempdir.path().join("hypercolor.toml"))
            .expect("test config manager should initialize");
        let applied = manager.get().capture.clone();
        manager.mark_capture_runtime_applied(&applied);
        let mut divergent = applied.clone();
        divergent.capture_fps += 1;

        assert!(manager.capture_runtime_matches(&applied));
        assert!(!manager.capture_runtime_matches(&divergent));
    }

    #[test]
    fn screen_runtime_commit_preserves_demand_and_retires_after_swap() {
        let mut manager = InputManager::new();
        manager
            .set_screen_capture_demand(test_screen_demand())
            .expect("screen demand should cache before a source exists");
        let first_plan = manager.plan_screen_runtime_config(true);
        assert_eq!(first_plan.capture_demand(), test_screen_demand());

        let first_stopped = Arc::new(AtomicBool::new(false));
        let mut first = Box::new(TestScreenSource::new(Arc::clone(&first_stopped)));
        first
            .set_screen_capture_demand(first_plan.capture_demand())
            .expect("prepared source should accept demand");
        first.start().expect("prepared source should start");
        let mut first = Some(first as Box<dyn InputSource>);
        manager
            .commit_screen_runtime_config(&first_plan, &mut first)
            .expect("initial prepared source should commit")
            .retire();
        assert!(first.is_none());

        let replacement_plan = manager.plan_screen_runtime_config(true);
        assert_eq!(replacement_plan.capture_demand(), test_screen_demand());
        let replacement_stopped = Arc::new(AtomicBool::new(false));
        let mut replacement = Box::new(TestScreenSource::new(replacement_stopped));
        replacement
            .set_screen_capture_demand(replacement_plan.capture_demand())
            .expect("replacement should accept demand");
        replacement.start().expect("replacement should start");
        let mut replacement = Some(replacement as Box<dyn InputSource>);
        let retirement = manager
            .commit_screen_runtime_config(&replacement_plan, &mut replacement)
            .expect("replacement should commit");

        assert!(!first_stopped.load(Ordering::Acquire));
        retirement.retire();
        assert!(first_stopped.load(Ordering::Acquire));
    }

    #[test]
    fn screen_runtime_commit_rejects_stale_graph_without_consuming_replacement() {
        let mut manager = InputManager::new();
        let plan = manager.plan_screen_runtime_config(true);
        let stopped = Arc::new(AtomicBool::new(false));
        let mut source = Box::new(TestScreenSource::new(Arc::clone(&stopped)));
        source.start().expect("prepared source should start");
        let mut replacement = Some(source as Box<dyn InputSource>);
        manager.add_source(Box::new(hypercolor_core::input::MediaSource::new()));

        assert!(matches!(
            manager.commit_screen_runtime_config(&plan, &mut replacement),
            Err(ScreenReconfigurationConflict::GraphChanged)
        ));
        assert!(replacement.is_some());
        assert!(!stopped.load(Ordering::Acquire));
        replacement
            .as_mut()
            .expect("failed commit preserves replacement ownership")
            .stop();
        assert!(stopped.load(Ordering::Acquire));
    }

    #[cfg(target_os = "windows")]
    #[tokio::test]
    async fn capture_transaction_applies_publication_capacity_with_config() {
        let tempdir = tempfile::tempdir().expect("temporary config directory should build");
        let manager = Arc::new(
            ConfigManager::new(tempdir.path().join("hypercolor.toml"))
                .expect("test config manager should initialize"),
        );
        let expected = Arc::clone(&manager.get());
        let mut capture = expected.capture.clone();
        capture.enabled = false;
        capture.publication_memory_bytes = Some(30_000);
        let mut state = AppState::new();
        state.config_manager = Some(Arc::clone(&manager));
        let state = Arc::new(state);
        state
            .input_manager
            .lock()
            .await
            .set_screen_capacity_plan(
                ScreenAdmissionCapacity::new(40_000, 40_000),
                ScreenAdmissionCapacity::new(30_000, 40_000),
                ScreenAdmissionCapacity::new(20_000, 40_000),
            )
            .expect("empty manager should accept test capacity");

        apply_capture_config_transaction(&state, &expected, capture.clone())
            .await
            .expect("valid publication capacity should apply");

        assert_eq!(manager.get().capture, capture);
        assert!(manager.capture_runtime_matches(&capture));
        let capacity = state
            .input_manager
            .lock()
            .await
            .screen_publication_capacity();
        assert_eq!(capacity.byte_budget(), 30_000);
        assert_eq!(capacity.backend_capacity(), 40_000);
        assert_eq!(
            state.input_manager.lock().await.screen_resource_capacity(),
            ScreenAdmissionCapacity::new(40_000, 40_000)
        );
    }

    #[cfg(target_os = "windows")]
    #[tokio::test]
    async fn capture_transaction_conflict_preserves_publication_capacity() {
        let tempdir = tempfile::tempdir().expect("temporary config directory should build");
        let manager = Arc::new(
            ConfigManager::new(tempdir.path().join("hypercolor.toml"))
                .expect("test config manager should initialize"),
        );
        let expected = Arc::clone(&manager.get());
        let mut capture = expected.capture.clone();
        capture.enabled = false;
        capture.publication_memory_bytes = Some(30_000);
        manager.modify(|config| config.capture.capture_fps += 1);
        let mut state = AppState::new();
        state.config_manager = Some(Arc::clone(&manager));
        let state = Arc::new(state);
        state
            .input_manager
            .lock()
            .await
            .set_screen_capacity_plan(
                ScreenAdmissionCapacity::new(40_000, 40_000),
                ScreenAdmissionCapacity::new(20_000, 40_000),
                ScreenAdmissionCapacity::new(20_000, 40_000),
            )
            .expect("empty manager should accept test capacity");

        let result = apply_capture_config_transaction(&state, &expected, capture).await;

        assert!(matches!(
            result,
            Err(CaptureConfigTransactionError::Conflict)
        ));
        let capacity = state
            .input_manager
            .lock()
            .await
            .screen_publication_capacity();
        assert_eq!(capacity, ScreenAdmissionCapacity::new(20_000, 40_000));
        assert_eq!(
            manager.get().capture.capture_fps,
            expected.capture.capture_fps + 1
        );
        assert_eq!(manager.get().capture.publication_memory_bytes, None);
    }

    #[cfg(target_os = "windows")]
    #[tokio::test]
    async fn failed_windows_capture_preparation_preserves_old_graph_and_config() {
        let config_path = std::env::temp_dir().join(format!(
            "hypercolor-capture-config-{}-{}.toml",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system clock should follow Unix epoch")
                .as_nanos()
        ));
        let manager = Arc::new(
            ConfigManager::new(config_path.clone()).expect("test config manager should initialize"),
        );
        let mut state = AppState::new();
        state.config_manager = Some(Arc::clone(&manager));
        let state = Arc::new(state);
        {
            let mut input_manager = state.input_manager.lock().await;
            let mut old = Box::new(TestScreenSource::new(Arc::new(AtomicBool::new(false))));
            old.start().expect("old test source should start");
            input_manager.add_source(old);
            input_manager
                .set_screen_capture_demand(test_screen_demand())
                .expect("old source should accept active demand");
        }
        let graph_generation = state.input_manager.lock().await.source_graph_generation();
        let admission_coordinator = state
            .input_manager
            .lock()
            .await
            .screen_admission_coordinator();
        let reserved_before = admission_coordinator.snapshot().reserved_bytes();
        let expected = Arc::clone(&manager.get());
        let mut capture = expected.capture.clone();
        capture.source = "monitor:hypercolor-test-source-that-does-not-exist".to_owned();

        let result = apply_capture_config_transaction(&state, &expected, capture).await;

        assert!(matches!(
            result,
            Err(CaptureConfigTransactionError::Prepare(_))
        ));
        assert_eq!(manager.get().capture.source, "auto");
        let input_manager = state.input_manager.lock().await;
        assert_eq!(input_manager.source_graph_generation(), graph_generation);
        assert!(input_manager.has_screen_source());
        assert!(
            input_manager
                .source_names()
                .iter()
                .any(|name| name == "test_screen")
        );
        assert_eq!(
            admission_coordinator.snapshot().reserved_bytes(),
            reserved_before
        );
        assert!(!config_path.exists());
    }

    #[tokio::test]
    async fn unchanged_disabled_capture_repairs_stale_runtime_source() {
        let tempdir = tempfile::tempdir().expect("temporary config directory should build");
        let manager = Arc::new(
            ConfigManager::new(tempdir.path().join("hypercolor.toml"))
                .expect("test config manager should initialize"),
        );
        manager.modify(|config| config.capture.enabled = false);
        let mut state = AppState::new();
        state.config_manager = Some(Arc::clone(&manager));
        let state = Arc::new(state);
        let stopped = Arc::new(AtomicBool::new(false));
        {
            let mut input_manager = state.input_manager.lock().await;
            let mut source = Box::new(TestScreenSource::new(Arc::clone(&stopped)));
            source.start().expect("stale source should start");
            input_manager.add_source(source);
            let mut extra = Box::new(TestScreenSource::new(Arc::new(AtomicBool::new(false))));
            extra.start().expect("extra stale source should start");
            input_manager.add_source(extra);
        }

        let response = set_config_value(
            axum::extract::State(Arc::clone(&state)),
            axum::Json(SetConfigRequest {
                key: "capture.enabled".to_owned(),
                value: "false".to_owned(),
                live: None,
            }),
        )
        .await;

        assert_eq!(response.status(), axum::http::StatusCode::OK);
        assert!(!state.input_manager.lock().await.has_screen_source());
        assert!(stopped.load(Ordering::Acquire));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn unchanged_capture_rejects_a_concurrent_config_generation() {
        let tempdir = tempfile::tempdir().expect("temporary config directory should build");
        let manager = Arc::new(
            ConfigManager::new(tempdir.path().join("hypercolor.toml"))
                .expect("test config manager should initialize"),
        );
        manager.modify(|config| config.capture.enabled = false);
        let initial = Arc::clone(&manager.get());
        manager.mark_capture_runtime_applied(&initial.capture);
        let mut state = AppState::new();
        state.config_manager = Some(Arc::clone(&manager));
        let state = Arc::new(state);
        let input_manager = state.input_manager.lock().await;
        let request_state = Arc::clone(&state);
        let unchanged_fps = initial.capture.capture_fps;
        let request = tokio::spawn(async move {
            set_config_value(
                axum::extract::State(request_state),
                axum::Json(SetConfigRequest {
                    key: "capture.capture_fps".to_owned(),
                    value: unchanged_fps.to_string(),
                    live: None,
                }),
            )
            .await
        });

        tokio::task::yield_now().await;
        let mut competing = (*initial).clone();
        competing.capture.capture_fps += 1;
        let competing_capture = competing.capture.clone();
        manager.update(competing);
        manager.mark_capture_runtime_applied(&competing_capture);
        drop(input_manager);

        let response = request.await.expect("unchanged request should complete");
        assert_eq!(response.status(), axum::http::StatusCode::CONFLICT);
        assert_eq!(manager.get().capture, competing_capture);
        assert!(manager.capture_runtime_matches(&competing_capture));
    }
}
