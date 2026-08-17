//! Presets CRUD endpoints — `/api/v1/library/presets/*`.

use std::collections::HashMap;
use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use axum::response::{IntoResponse, Response};
use serde::Serialize;

use hypercolor_types::effect::{ControlValue, EffectMetadata};
use hypercolor_types::event::{
    ChangeTrigger, HypercolorEvent, LibraryChangeKind, LibraryCollection, ZoneChangeKind,
};
use hypercolor_types::library::{EffectPreset, PresetId};
use hypercolor_types::scene::ZoneId;

use crate::api::AppState;
use crate::api::control_values::json_to_control_value;
use crate::api::effects::resolve_effect_metadata;
use crate::api::envelope::ApiResponse;
use crate::domain::{DomainError, ResourceKind};

use super::{
    ActivateEffectError, ActivationResult, activate_effect_with_controls, normalize_tags,
    resolve_preset_id, store_error_to_response, unix_epoch_ms,
};

pub use hypercolor_types::api::library::{ApplyPresetRequest, SavePresetRequest};

// ── Request / Response Types ────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct PresetListResponse {
    pub items: Vec<EffectPreset>,
    pub pagination: crate::api::devices::Pagination,
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
        return DomainError::not_found(ResourceKind::Preset, &id).into_response();
    };

    let Some(preset) = state.library_store.get_preset(preset_id).await else {
        return DomainError::not_found(ResourceKind::Preset, &id).into_response();
    };

    ApiResponse::ok(preset)
}

/// `POST /api/v1/library/presets` — create a new saved preset.
pub async fn create_preset(
    State(state): State<Arc<AppState>>,
    Json(body): Json<SavePresetRequest>,
) -> Response {
    if body.name.trim().is_empty() {
        return DomainError::validation("Preset name must not be empty").into_response();
    }

    let effect = {
        let registry = state.effect_registry.read().await;
        let Some(effect) = resolve_effect_metadata(&registry, &body.effect) else {
            return DomainError::not_found(ResourceKind::Effect, &body.effect).into_response();
        };
        effect
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

    ApiResponse::created(preset)
}

/// `PUT /api/v1/library/presets/:id` — update an existing preset.
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

    let effect = {
        let registry = state.effect_registry.read().await;
        let Some(effect) = resolve_effect_metadata(&registry, &body.effect) else {
            return DomainError::not_found(ResourceKind::Effect, &body.effect).into_response();
        };
        effect
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

    ApiResponse::ok(preset)
}

/// `DELETE /api/v1/library/presets/:id` — remove a preset.
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
pub async fn apply_preset(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    body: Option<Json<ApplyPresetRequest>>,
) -> Response {
    let Some(preset_id) = resolve_preset_id(&state, &id).await else {
        return DomainError::not_found(ResourceKind::Preset, &id).into_response();
    };
    let Some(preset) = state.library_store.get_preset(preset_id).await else {
        return DomainError::not_found(ResourceKind::Preset, &id).into_response();
    };

    let metadata = {
        let registry = state.effect_registry.read().await;
        let Some(entry) = registry.get(&preset.effect_id) else {
            return DomainError::not_found(ResourceKind::Effect, preset.effect_id).into_response();
        };
        entry.metadata.clone()
    };

    // A zone_id naming a non-Primary zone takes the zone-scoped
    // path; naming the Primary (or omitting it) keeps legacy semantics.
    let target_group = match crate::api::effects::parse_zone_id_field(
        body.as_ref().and_then(|body| body.zone_id.as_deref()),
    ) {
        Ok(target) => target,
        Err(error) => return error.into_response(),
    };
    let named_target = match target_group {
        None => None,
        Some(group_id) => {
            let scene_manager = state.scene_manager.read().await;
            let primary_id = scene_manager
                .active_scene()
                .and_then(|scene| scene.primary_group())
                .map(|group| group.id);
            (Some(group_id) != primary_id).then_some(group_id)
        }
    };
    if let Some(group_id) = named_target {
        return apply_preset_to_zone(&state, group_id, &preset, &metadata).await;
    }

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
            return DomainError::not_found(ResourceKind::Effect, "active").into_response();
        };
        let mut mutation = state.begin_scene_mutation().await;
        if let Err(error) = mutation.active_scene_for_runtime_mutation("applying a preset") {
            return DomainError::conflict(error.to_string()).into_response();
        }
        if mutation
            .reset_zone_controls(
                group.id,
                crate::api::effects::default_control_values(&metadata),
            )
            .is_none()
            || mutation
                .patch_zone_controls(group.id, applied.clone())
                .is_none()
        {
            return DomainError::not_found(ResourceKind::Effect, "active").into_response();
        }
        mutation.set_zone_preset_id(group.id, Some(preset.id));
        if let Err(error) = crate::domain::scene::commit_scene(state.as_ref(), mutation).await {
            return error.into_response();
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
                return DomainError::conflict(error).into_response();
            }
            Ok(activation) => {
                if let Some((group, _)) =
                    crate::api::effects::active_primary_effect(state.as_ref()).await
                {
                    let mut mutation = state.begin_scene_mutation().await;
                    mutation.set_zone_preset_id(group.id, Some(preset.id));
                    if let Err(error) =
                        crate::domain::scene::commit_scene(state.as_ref(), mutation).await
                    {
                        return error.into_response();
                    }
                }
                activation
            }
            Err(ActivateEffectError::Activation(error)) => {
                return DomainError::Internal(anyhow::anyhow!(
                    "Failed to activate effect '{}' from preset '{}': {error}",
                    metadata.name,
                    preset.name
                ))
                .into_response();
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

/// Apply a preset to a named non-Primary zone. When the zone already runs
/// the preset's effect, controls hot-swap in place (defaults, then preset
/// values) exactly like the legacy primary path; otherwise the preset's
/// effect is set on the zone with the preset controls baked in.
async fn apply_preset_to_zone(
    state: &Arc<AppState>,
    group_id: ZoneId,
    preset: &EffectPreset,
    metadata: &EffectMetadata,
) -> Response {
    let (applied, rejected) =
        crate::api::effects::normalize_control_values(metadata, &preset.controls);

    // Naming the outgoing effect needs the registry, and the outgoing
    // effect is not known until the candidate is in hand. Taking the
    // whole index up front keeps every await out of the window between
    // the snapshot and its compare-and-swap, the same way apply_effect
    // does.
    let effect_refs = {
        let registry = state.effect_registry.read().await;
        registry
            .iter()
            .map(|(id, entry)| (*id, crate::api::effects::effect_ref(&entry.metadata)))
            .collect::<HashMap<_, _>>()
    };

    let mut mutation = state.begin_scene_mutation().await;
    let scene_id = match mutation.active_scene_for_runtime_mutation("applying a preset") {
        Ok(scene_id) => scene_id,
        Err(error) => return DomainError::conflict(error.to_string()).into_response(),
    };
    let previous_effect_id = mutation.zone_effect(group_id);

    if previous_effect_id == Some(metadata.id) {
        if mutation
            .reset_zone_controls(
                group_id,
                crate::api::effects::default_control_values(metadata),
            )
            .is_none()
            || mutation
                .patch_zone_controls(group_id, applied.clone())
                .is_none()
        {
            return DomainError::not_found(ResourceKind::Zone, group_id).into_response();
        }
        mutation.set_zone_preset_id(group_id, Some(preset.id));
    } else if let Err(error) =
        mutation.apply_effect_to_zone(group_id, metadata, applied.clone(), Some(preset.id))
    {
        return DomainError::validation(error.to_string()).into_response();
    }

    let Some(group) = mutation
        .scenes()
        .active_scene()
        .and_then(|scene| scene.groups.iter().find(|group| group.id == group_id))
        .cloned()
    else {
        return DomainError::not_found(ResourceKind::Zone, group_id).into_response();
    };

    if previous_effect_id != Some(metadata.id) {
        let previous =
            previous_effect_id.and_then(|effect_id| effect_refs.get(&effect_id).cloned());
        mutation.record(HypercolorEvent::EffectStarted {
            effect: crate::api::effects::effect_ref(metadata),
            trigger: ChangeTrigger::Api,
            previous,
            transition: None,
            zone_id: Some(group.id),
            zone_name: Some(group.name.clone()),
        });
    }
    mutation.record(crate::domain::scene::zone_changed_event(
        scene_id,
        &group,
        ZoneChangeKind::Updated,
    ));

    if let Err(error) = crate::domain::scene::commit_scene(state.as_ref(), mutation).await {
        return error.into_response();
    }
    crate::api::persist_runtime_session(state).await;

    ApiResponse::ok(serde_json::json!({
        "preset": {
            "id": preset.id.to_string(),
            "name": preset.name,
        },
        "effect": {
            "id": metadata.id.to_string(),
            "name": metadata.name,
        },
        "applied_controls": applied,
        "rejected_controls": rejected,
        "warnings": Vec::<String>::new(),
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
