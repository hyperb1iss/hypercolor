//! Presets CRUD endpoints — `/api/v1/library/presets/*`.

use std::collections::HashMap;
use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use axum::response::{IntoResponse, Response};

use hypercolor_types::control::ControlValue;
use hypercolor_types::event::{HypercolorEvent, LibraryChangeKind, LibraryCollection};
use hypercolor_types::library::{EffectPreset, PresetId};

use crate::api::envelope;
use crate::app_state::AppState;
use crate::domain::{DomainError, ResourceKind};

use super::{normalize_tags, resolve_preset_id, store_error, unix_epoch_ms};

// Wire contracts live in hypercolor-types::api::library — shared with
// the web UI and the TUI.
pub use hypercolor_types::api::library::{
    DeletePresetResponse, PresetListResponse, SavePresetRequest,
};

// ── Handlers ────────────────────────────────────────────────────────────

/// `GET /api/v1/library/presets` — list all saved presets.
pub async fn list_presets(State(state): State<Arc<AppState>>) -> Response {
    let items = state.library_store().list_presets().await;
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

    let Some(preset) = state.library_store().get_preset(preset_id).await else {
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

    let _admission = state.domains.effects.admit_current().await;
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

    if let Err(error) = state.library_store().insert_preset(preset.clone()).await {
        return store_error(&error).into_response();
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

    let Some(existing) = state.library_store().get_preset(preset_id).await else {
        return DomainError::not_found(ResourceKind::Preset, &id).into_response();
    };

    let _admission = state.domains.effects.admit_current().await;
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

    if let Err(error) = state.library_store().update_preset(preset.clone()).await {
        return store_error(&error).into_response();
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

    let removed = match state.library_store().remove_preset(preset_id).await {
        Ok(removed) => removed,
        Err(error) => return store_error(&error).into_response(),
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
        let Some(definition) = effect.control_by_id(name) else {
            rejected.push(format!("{name} (unknown control)"));
            continue;
        };
        match definition.admit_effect_json(raw_value) {
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

#[cfg(test)]
mod tests {
    use hypercolor_types::control::ControlValue;
    use hypercolor_types::effect::{
        ControlDefinition, ControlKind, ControlType, EffectCategory, EffectId, EffectMetadata,
        EffectSource,
    };

    use super::parse_preset_controls;

    fn metadata() -> EffectMetadata {
        EffectMetadata {
            id: EffectId::new(uuid::Uuid::now_v7()),
            name: "preset fixture".to_owned(),
            author: "test".to_owned(),
            version: "1".to_owned(),
            description: String::new(),
            category: EffectCategory::Ambient,
            tags: Vec::new(),
            controls: vec![ControlDefinition {
                id: "accent".to_owned(),
                name: "Accent".to_owned(),
                kind: ControlKind::Color,
                control_type: ControlType::ColorPicker,
                default_value: ControlValue::linear_color([1.0, 1.0, 1.0, 1.0]),
                min: None,
                max: None,
                step: None,
                labels: Vec::new(),
                group: None,
                tooltip: None,
                aspect_lock: None,
                preview_source: None,
                binding: None,
            }],
            presets: Vec::new(),
            audio_reactive: false,
            screen_reactive: false,
            input_reactive: false,
            source: EffectSource::Native {
                path: "fixture".into(),
            },
            license: None,
        }
    }

    #[test]
    fn preset_controls_admit_schema_confirmed_rgba_arrays() {
        let payload = serde_json::json!({
            "accent": [0.125, 0.25, 0.5, 1.0],
        });

        let controls = parse_preset_controls(&metadata(), Some(&payload))
            .expect("schema-confirmed color should be admitted");

        assert_eq!(
            controls.get("accent"),
            Some(&ControlValue::linear_color([0.125, 0.25, 0.5, 1.0]))
        );
    }

    #[test]
    fn preset_controls_reject_unknown_control_ids() {
        let payload = serde_json::json!({
            "missing": 0.5,
        });

        let rejected = parse_preset_controls(&metadata(), Some(&payload))
            .expect_err("unknown controls must be rejected");

        assert_eq!(rejected, vec!["missing (unknown control)"]);
    }
}
