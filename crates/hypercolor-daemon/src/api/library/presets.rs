//! Presets CRUD endpoints — `/api/v1/library/presets/*`.

use std::collections::HashMap;
use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use axum::response::{IntoResponse, Response};

use hypercolor_types::effect::ControlValue;
use hypercolor_types::event::{HypercolorEvent, LibraryChangeKind, LibraryCollection};
use hypercolor_types::library::{EffectPreset, PresetId};

use crate::api::control_values::json_to_control_value;
use crate::api::envelope;
use crate::app_state::AppState;
use crate::domain::{DomainError, ResourceKind};

use super::{normalize_tags, resolve_preset_id, store_error_to_response, unix_epoch_ms};

// Wire contracts live in hypercolor-types::api::library — shared with
// the web UI and the TUI.
pub use hypercolor_types::api::library::{
    DeletePresetResponse, PresetListResponse, SavePresetRequest,
};

// ── Handlers ────────────────────────────────────────────────────────────

/// `GET /api/v1/library/presets` — list all saved presets.
pub async fn list_presets(State(state): State<Arc<AppState>>) -> Response {
    let items = state.library_store.list_presets().await;
    let total = items.len();

    envelope::ok(PresetListResponse {
        items,
        total: u64::try_from(total).expect("preset count fits in u64"),
        page: None,
    })
}

/// `GET /api/v1/library/presets/{id}` — fetch one preset.
pub async fn get_preset(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    let Some(preset_id) = resolve_preset_id(&state, &id).await else {
        return DomainError::not_found(ResourceKind::Preset, &id).into_response();
    };

    let Some(preset) = state.library_store.get_preset(preset_id).await else {
        return DomainError::not_found(ResourceKind::Preset, &id).into_response();
    };

    envelope::ok(preset)
}

/// `POST /api/v1/library/presets` — create a new saved preset.
pub async fn create_preset(
    State(state): State<Arc<AppState>>,
    Json(body): Json<SavePresetRequest>,
) -> Response {
    if body.name.trim().is_empty() {
        return DomainError::validation("Preset name must not be empty").into_response();
    }

    let Some(effect) = state.domains.effects.resolve_metadata(&body.effect).await else {
        return DomainError::not_found(ResourceKind::Effect, &body.effect).into_response();
    };

    let controls = match parse_preset_controls(&effect, body.controls.as_ref()) {
        Ok(controls) => controls,
        Err(rejected) => {
            return DomainError::validation(format!(
                "Invalid preset controls: {}",
                rejected.join(", ")
            ))
            .into_response();
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
    state
        .event_bus
        .publish(HypercolorEvent::LibraryStoreChanged {
            collection: LibraryCollection::Presets,
            entry_id: preset.id.to_string(),
            kind: LibraryChangeKind::Upserted,
        });

    envelope::created(preset)
}

/// `PUT /api/v1/library/presets/{id}` — update an existing preset.
pub async fn update_preset(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<SavePresetRequest>,
) -> Response {
    let Some(preset_id) = resolve_preset_id(&state, &id).await else {
        return DomainError::not_found(ResourceKind::Preset, &id).into_response();
    };
    if body.name.trim().is_empty() {
        return DomainError::validation("Preset name must not be empty").into_response();
    }

    let Some(existing) = state.library_store.get_preset(preset_id).await else {
        return DomainError::not_found(ResourceKind::Preset, &id).into_response();
    };

    let Some(effect) = state.domains.effects.resolve_metadata(&body.effect).await else {
        return DomainError::not_found(ResourceKind::Effect, &body.effect).into_response();
    };

    let controls = match parse_preset_controls(&effect, body.controls.as_ref()) {
        Ok(controls) => controls,
        Err(rejected) => {
            return DomainError::validation(format!(
                "Invalid preset controls: {}",
                rejected.join(", ")
            ))
            .into_response();
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
    state
        .event_bus
        .publish(HypercolorEvent::LibraryStoreChanged {
            collection: LibraryCollection::Presets,
            entry_id: preset.id.to_string(),
            kind: LibraryChangeKind::Upserted,
        });

    envelope::ok(preset)
}

/// `DELETE /api/v1/library/presets/{id}` — remove a preset.
pub async fn delete_preset(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    let Some(preset_id) = resolve_preset_id(&state, &id).await else {
        return DomainError::not_found(ResourceKind::Preset, &id).into_response();
    };

    let removed = match state.library_store.remove_preset(preset_id).await {
        Ok(removed) => removed,
        Err(error) => return store_error_to_response(&error),
    };
    if !removed {
        return DomainError::not_found(ResourceKind::Preset, &id).into_response();
    }
    state
        .event_bus
        .publish(HypercolorEvent::LibraryStoreChanged {
            collection: LibraryCollection::Presets,
            entry_id: preset_id.to_string(),
            kind: LibraryChangeKind::Removed,
        });

    envelope::ok(DeletePresetResponse {
        id: preset_id.to_string(),
        deleted: true,
    })
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
