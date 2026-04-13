//! Saved effect library endpoints — `/api/v1/library/*`.

mod favorites;
mod playlists;
mod presets;

pub use favorites::*;
pub use playlists::*;
pub use presets::*;

use std::collections::HashMap;
use std::sync::Arc;

use hypercolor_types::effect::{ControlValue, EffectId, EffectMetadata};
use hypercolor_types::library::PresetId;
use tracing::info;

use crate::api::AppState;
use crate::library::LibraryStoreError;

// ── Shared Types ────────────────────────────────────────────────────────

pub(crate) struct ActivationResult {
    pub applied: HashMap<String, ControlValue>,
    pub rejected: Vec<String>,
    pub warnings: Vec<crate::api::displays::OverlayCompatibilityWarning>,
}

pub(crate) enum ActivateEffectError {
    Conflict(String),
    Activation(String),
}

impl std::fmt::Display for ActivateEffectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Conflict(error) | Self::Activation(error) => f.write_str(error),
        }
    }
}

// ── Shared Helpers ──────────────────────────────────────────────────────

pub(crate) async fn resolve_preset_id(state: &Arc<AppState>, id_or_name: &str) -> Option<PresetId> {
    if let Ok(id) = id_or_name.parse::<PresetId>() {
        return Some(id);
    }

    state
        .library_store
        .list_presets()
        .await
        .iter()
        .find(|preset| preset.name.eq_ignore_ascii_case(id_or_name))
        .map(|preset| preset.id)
}

pub(crate) async fn metadata_for_effect_id(
    state: &Arc<AppState>,
    effect_id: EffectId,
) -> Result<EffectMetadata, String> {
    let registry = state.effect_registry.read().await;
    let Some(entry) = registry.get(&effect_id) else {
        return Err(format!("effect not found: {effect_id}"));
    };
    Ok(entry.metadata.clone())
}

pub(crate) async fn activate_effect_with_controls(
    state: &Arc<AppState>,
    metadata: &EffectMetadata,
    controls: &HashMap<String, ControlValue>,
) -> Result<ActivationResult, ActivateEffectError> {
    let (controls, migrated_controls) =
        crate::library::migration::migrate_effect_controls_for_load(metadata, controls);
    let (controls, rejected) = crate::api::effects::normalize_control_values(metadata, &controls);
    let layout = {
        let spatial = state.spatial_engine.read().await;
        spatial.layout().as_ref().clone()
    };

    {
        let mut scene_manager = state.scene_manager.write().await;
        crate::api::active_scene_id_for_runtime_mutation(&scene_manager)
            .map_err(|error| ActivateEffectError::Conflict(error.message("applying an effect")))?;
        scene_manager
            .upsert_primary_group(metadata, controls.clone(), None, layout)
            .map_err(|error| ActivateEffectError::Activation(error.to_string()))?;
    }
    if migrated_controls {
        info!(
            effect_id = %metadata.id,
            effect = %metadata.name,
            "Migrated legacy screencast controls to the viewport rect"
        );
    }
    let warnings =
        crate::api::displays::auto_disable_html_overlays_for_effect(state, metadata).await;
    crate::api::persist_runtime_session(state).await;

    Ok(ActivationResult {
        applied: controls,
        rejected,
        warnings,
    })
}

pub(crate) fn store_error_to_response(error: &LibraryStoreError) -> axum::response::Response {
    use crate::api::envelope::ApiError;

    match error {
        LibraryStoreError::PresetNotFound(id) => {
            ApiError::not_found(format!("Preset not found: {id}"))
        }
        LibraryStoreError::PresetConflict(id) => {
            ApiError::conflict(format!("Preset already exists: {id}"))
        }
        LibraryStoreError::PlaylistNotFound(id) => {
            ApiError::not_found(format!("Playlist not found: {id}"))
        }
        LibraryStoreError::PlaylistConflict(id) => {
            ApiError::conflict(format!("Playlist already exists: {id}"))
        }
    }
}

pub(crate) fn unix_epoch_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};

    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

pub(crate) fn normalize_tags(tags: Option<Vec<String>>) -> Vec<String> {
    tags.unwrap_or_default()
        .into_iter()
        .map(|tag| tag.trim().to_owned())
        .filter(|tag| !tag.is_empty())
        .collect()
}
