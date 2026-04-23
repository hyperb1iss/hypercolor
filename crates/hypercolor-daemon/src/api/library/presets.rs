//! Presets CRUD endpoints — `/api/v1/library/presets/*`.

use std::collections::HashMap;
use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use axum::response::Response;
use serde::{Deserialize, Serialize};

use hypercolor_types::effect::ControlValue;
use hypercolor_types::library::{EffectPreset, PresetId};

use crate::api::AppState;
use crate::api::control_values::json_to_control_value;
use crate::api::effects::resolve_effect_metadata;
use crate::api::envelope::{ApiError, ApiResponse};

use super::{
    ActivateEffectError, ActivationResult, activate_effect_with_controls, normalize_tags,
    resolve_preset_id, store_error_to_response, unix_epoch_ms,
};

// ── Request / Response Types ────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct PresetListResponse {
    pub items: Vec<EffectPreset>,
    pub pagination: crate::api::devices::Pagination,
}

#[derive(Debug, Deserialize)]
pub struct SavePresetRequest {
    pub name: String,
    pub description: Option<String>,
    pub effect: String,
    pub controls: Option<serde_json::Value>,
    pub tags: Option<Vec<String>>,
}

// ── Handlers ────────────────────────────────────────────────────────────

/// `GET /api/v1/library/presets` — list all saved presets.
pub async fn list_presets(State(state): State<Arc<AppState>>) -> Response {
    let items = state.library_store.list_presets().await;
    let total = items.len();

    ApiResponse::ok(PresetListResponse {
        items,
        pagination: crate::api::devices::Pagination {
            offset: 0,
            limit: 50,
            total,
            has_more: false,
        },
    })
}

/// `GET /api/v1/library/presets/:id` — fetch one preset.
pub async fn get_preset(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    let Some(preset_id) = resolve_preset_id(&state, &id).await else {
        return ApiError::not_found(format!("Preset not found: {id}"));
    };

    let Some(preset) = state.library_store.get_preset(preset_id).await else {
        return ApiError::not_found(format!("Preset not found: {id}"));
    };

    ApiResponse::ok(preset)
}

/// `POST /api/v1/library/presets` — create a new saved preset.
pub async fn create_preset(
    State(state): State<Arc<AppState>>,
    Json(body): Json<SavePresetRequest>,
) -> Response {
    if body.name.trim().is_empty() {
        return ApiError::validation("Preset name must not be empty");
    }

    let effect = {
        let registry = state.effect_registry.read().await;
        let Some(effect) = resolve_effect_metadata(&registry, &body.effect) else {
            return ApiError::not_found(format!("Effect not found: {}", body.effect));
        };
        effect
    };

    let controls = match parse_preset_controls(&effect, body.controls.as_ref()) {
        Ok(controls) => controls,
        Err(rejected) => {
            return ApiError::validation(format!(
                "Invalid preset controls: {}",
                rejected.join(", ")
            ));
        }
    };

    let now = unix_epoch_ms();
    let preset = EffectPreset {
        id: PresetId::new(),
        name: body.name.trim().to_owned(),
        description: body.description,
        effect_id: effect.id,
        controls,
        tags: normalize_tags(body.tags),
        created_at_ms: now,
        updated_at_ms: now,
    };

    if let Err(error) = state.library_store.insert_preset(preset.clone()).await {
        return store_error_to_response(&error);
    }

    ApiResponse::created(preset)
}

/// `PUT /api/v1/library/presets/:id` — update an existing preset.
pub async fn update_preset(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<SavePresetRequest>,
) -> Response {
    let Some(preset_id) = resolve_preset_id(&state, &id).await else {
        return ApiError::not_found(format!("Preset not found: {id}"));
    };
    if body.name.trim().is_empty() {
        return ApiError::validation("Preset name must not be empty");
    }

    let Some(existing) = state.library_store.get_preset(preset_id).await else {
        return ApiError::not_found(format!("Preset not found: {id}"));
    };

    let effect = {
        let registry = state.effect_registry.read().await;
        let Some(effect) = resolve_effect_metadata(&registry, &body.effect) else {
            return ApiError::not_found(format!("Effect not found: {}", body.effect));
        };
        effect
    };

    let controls = match parse_preset_controls(&effect, body.controls.as_ref()) {
        Ok(controls) => controls,
        Err(rejected) => {
            return ApiError::validation(format!(
                "Invalid preset controls: {}",
                rejected.join(", ")
            ));
        }
    };

    let preset = EffectPreset {
        id: preset_id,
        name: body.name.trim().to_owned(),
        description: body.description,
        effect_id: effect.id,
        controls,
        tags: normalize_tags(body.tags),
        created_at_ms: existing.created_at_ms,
        updated_at_ms: unix_epoch_ms(),
    };

    if let Err(error) = state.library_store.update_preset(preset.clone()).await {
        return store_error_to_response(&error);
    }

    ApiResponse::ok(preset)
}

/// `DELETE /api/v1/library/presets/:id` — remove a preset.
pub async fn delete_preset(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    let Some(preset_id) = resolve_preset_id(&state, &id).await else {
        return ApiError::not_found(format!("Preset not found: {id}"));
    };

    if !state.library_store.remove_preset(preset_id).await {
        return ApiError::not_found(format!("Preset not found: {id}"));
    }

    ApiResponse::ok(serde_json::json!({
        "id": preset_id.to_string(),
        "deleted": true,
    }))
}

/// `POST /api/v1/library/presets/:id/apply` — activate a preset immediately.
///
/// When the preset targets the same effect that is already running, controls
/// are updated in-place (reset to defaults, then apply preset values) without
/// tearing down and re-creating the renderer. This avoids animation restarts
/// and visual glitches.
pub async fn apply_preset(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    let Some(preset_id) = resolve_preset_id(&state, &id).await else {
        return ApiError::not_found(format!("Preset not found: {id}"));
    };
    let Some(preset) = state.library_store.get_preset(preset_id).await else {
        return ApiError::not_found(format!("Preset not found: {id}"));
    };

    let metadata = {
        let registry = state.effect_registry.read().await;
        let Some(entry) = registry.get(&preset.effect_id) else {
            return ApiError::not_found(format!(
                "Preset references missing effect: {}",
                preset.effect_id
            ));
        };
        entry.metadata.clone()
    };

    // Check if the same effect is already running — if so, skip full re-activation
    let same_effect = crate::api::effects::active_primary_effect(state.as_ref())
        .await
        .is_some_and(|(_, active)| active.id == metadata.id);

    let activation = if same_effect {
        // Hot-swap: reset to defaults, apply preset controls, set preset ID
        let (applied, rejected) =
            crate::api::effects::normalize_control_values(&metadata, &preset.controls);
        let Some((group, _)) = crate::api::effects::active_primary_effect(state.as_ref()).await
        else {
            return ApiError::not_found("No effect is currently active");
        };
        {
            let mut scene_manager = state.scene_manager.write().await;
            if let Err(error) = crate::api::active_scene_id_for_runtime_mutation(&scene_manager) {
                return ApiError::conflict(error.message("applying a preset"));
            }
            if scene_manager
                .reset_group_controls(
                    group.id,
                    crate::api::effects::default_control_values(&metadata),
                )
                .is_none()
            {
                return ApiError::not_found("No effect is currently active");
            }
            if scene_manager
                .patch_group_controls(group.id, applied.clone())
                .is_none()
            {
                return ApiError::not_found("No effect is currently active");
            }
            let _ = scene_manager.set_group_preset_id(group.id, Some(preset.id));
        }

        ActivationResult {
            applied,
            rejected,
            warnings: Vec::new(),
        }
    } else {
        // Different effect — full activation path
        match activate_effect_with_controls(&state, &metadata, &preset.controls).await {
            Err(ActivateEffectError::Conflict(error)) => {
                return ApiError::conflict(error);
            }
            Ok(activation) => {
                if let Some((group, _)) =
                    crate::api::effects::active_primary_effect(state.as_ref()).await
                {
                    let mut scene_manager = state.scene_manager.write().await;
                    let _ = scene_manager.set_group_preset_id(group.id, Some(preset.id));
                }
                activation
            }
            Err(ActivateEffectError::Activation(error)) => {
                return ApiError::internal(format!(
                    "Failed to activate effect '{}' from preset '{}': {error}",
                    metadata.name, preset.name
                ));
            }
        }
    };
    crate::api::persist_runtime_session(&state).await;

    ApiResponse::ok(serde_json::json!({
        "preset": {
            "id": preset.id.to_string(),
            "name": preset.name,
        },
        "effect": {
            "id": metadata.id.to_string(),
            "name": metadata.name,
        },
        "applied_controls": activation.applied,
        "rejected_controls": activation.rejected,
        "warnings": activation.warnings,
    }))
}

// ── Helpers ─────────────────────────────────────────────────────────────

fn parse_preset_controls(
    effect: &hypercolor_types::effect::EffectMetadata,
    controls_payload: Option<&serde_json::Value>,
) -> Result<HashMap<String, ControlValue>, Vec<String>> {
    let Some(controls_json) = controls_payload else {
        return Ok(HashMap::new());
    };
    let Some(control_map) = controls_json.as_object() else {
        return Err(vec!["controls must be a JSON object".to_owned()]);
    };

    let mut normalized = HashMap::new();
    let mut rejected = Vec::new();
    for (name, raw_value) in control_map {
        let Some(parsed) = json_to_control_value(raw_value) else {
            rejected.push(format!("{name} (unsupported JSON shape)"));
            continue;
        };
        let Some(definition) = effect.control_by_id(name) else {
            rejected.push(format!("{name} (unknown control)"));
            continue;
        };
        match definition.validate_value(&parsed) {
            Ok(validated) => {
                normalized.insert(name.clone(), validated);
            }
            Err(error) => rejected.push(format!("{name} ({error})")),
        }
    }

    if rejected.is_empty() {
        Ok(normalized)
    } else {
        Err(rejected)
    }
}
