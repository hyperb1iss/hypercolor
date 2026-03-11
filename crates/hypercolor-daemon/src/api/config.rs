//! Config endpoints — `/api/v1/config*`.

use std::sync::Arc;

use anyhow::anyhow;
use axum::Json;
use axum::extract::{Query, State};
use axum::response::Response;
use serde::Deserialize;
use tracing::warn;

use hypercolor_types::config::HypercolorConfig;

use crate::api::AppState;
use crate::api::envelope::{ApiError, ApiResponse};
use crate::startup::build_input_manager;

#[derive(Debug, Deserialize)]
pub struct GetConfigQuery {
    pub key: String,
}

#[derive(Debug, Deserialize)]
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
    let config = config_snapshot(&state);
    let value = match serde_json::to_value(config) {
        Ok(v) => v,
        Err(e) => return ApiError::internal(format!("Failed to serialize config: {e}")),
    };

    let Some(found) = get_json_path(&value, &query.key) else {
        return ApiError::not_found(format!("Unknown config key: {}", query.key));
    };

    ApiResponse::ok(serde_json::json!({
        "key": query.key,
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

    let parsed_value = serde_json::from_str::<serde_json::Value>(&body.value)
        .unwrap_or_else(|_| serde_json::Value::String(body.value.clone()));

    if !set_json_path(&mut root, &body.key, parsed_value.clone()) {
        return ApiError::validation(format!("Invalid config key path: {}", body.key));
    }

    let updated: HypercolorConfig = match serde_json::from_value(root) {
        Ok(cfg) => cfg,
        Err(e) => {
            return ApiError::validation(format!(
                "Config update failed validation for '{}': {e}",
                body.key
            ));
        }
    };

    manager.update(updated);
    if let Err(e) = manager.save() {
        return ApiError::internal(format!("Failed to persist config: {e}"));
    }

    let live_applied =
        maybe_apply_audio_config_change(&state, Some(&body.key), body.live.unwrap_or(false)).await;

    ApiResponse::ok(serde_json::json!({
        "key": body.key,
        "value": parsed_value,
        "live": live_applied,
        "path": manager.path().display().to_string(),
    }))
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

    if let Some(key) = &body.key {
        let Some(default_value) = get_json_path(&defaults, key) else {
            return ApiError::not_found(format!("Unknown config key: {key}"));
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

    manager.update(updated);
    if let Err(e) = manager.save() {
        return ApiError::internal(format!("Failed to persist config: {e}"));
    }

    let live_applied =
        maybe_apply_audio_config_change(&state, body.key.as_deref(), body.live.unwrap_or(false))
            .await;

    ApiResponse::ok(serde_json::json!({
        "key": body.key,
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
    if !live_requested || !should_reconfigure_audio_inputs(key) {
        return false;
    }

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
    let mut replacement = build_input_manager(latest_config.as_ref());
    let mut input_manager = state.input_manager.lock().await;
    let mut previous = std::mem::take(&mut *input_manager);

    previous.stop_all();
    match replacement.start_all() {
        Ok(()) => {
            *input_manager = replacement;
            Ok(())
        }
        Err(error) => {
            if let Err(restart_error) = previous.start_all() {
                *input_manager = previous;
                return Err(anyhow!(
                    "failed to start rebuilt input sources: {error}; previous input sources could not be restarted cleanly: {restart_error}"
                ));
            }

            *input_manager = previous;
            Err(anyhow!("failed to start rebuilt input sources: {error}"))
        }
    }
}
