//! Saved effect library endpoints — `/api/v1/library/*`.

mod favorites;
mod playlists;
mod presets;

pub use favorites::*;
pub use playlists::*;
pub use presets::*;

use std::collections::HashMap;
use std::sync::Arc;

use axum::response::{IntoResponse, Response};
use hypercolor_types::effect::{ControlValue, EffectId, EffectMetadata};
use hypercolor_types::library::PresetId;

use crate::api::AppState;
use crate::domain::{DomainError, ResourceKind};
use crate::library::LibraryStoreError;

// ── Shared Types ────────────────────────────────────────────────────────

pub(crate) struct ActivationResult {
    pub applied: HashMap<String, ControlValue>,
    pub rejected: Vec<String>,
    pub warnings: Vec<String>,
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
    let (controls, rejected) = crate::api::effects::normalize_control_values(metadata, controls);
    let layout = {
        let spatial = state.spatial_engine.read().await;
        spatial.layout().as_ref().clone()
    };

    // Library activation loads the effect without announcing an effect
    // switch — the caller publishes its own preset/playlist events — so
    // it commits its own mutation rather than routing through
    // `domain::effect::apply_effect`.
    let mut mutation = state.begin_scene_mutation().await;
    mutation
        .active_scene_for_runtime_mutation("applying an effect")
        .map_err(|error| ActivateEffectError::Conflict(error.to_string()))?;
    mutation
        .upsert_primary_zone(metadata, controls.clone(), None, layout)
        .map_err(|error| ActivateEffectError::Activation(error.to_string()))?;
    let commit = crate::domain::scene::commit_scene(state.as_ref(), mutation)
        .await
        .map_err(|error| match error {
            // A competing scene commit is a state conflict, not an
            // activation failure, and this path already has a shape for
            // one.
            DomainError::PreconditionFailed { .. } => {
                ActivateEffectError::Conflict(error.to_string())
            }
            other => ActivateEffectError::Activation(other.to_string()),
        })?;
    if let Some(error) = commit.retry_error() {
        // Admitted and converging, not failed.
        tracing::warn!(%error, "Scene write has not proven durable yet; retry remains active");
    }
    crate::api::persist_runtime_session(state).await;

    Ok(ActivationResult {
        applied: controls,
        rejected,
        warnings: Vec::new(),
    })
}

pub(crate) fn store_error_to_response(error: &LibraryStoreError) -> Response {
    match error {
        LibraryStoreError::PresetNotFound(id) => {
            DomainError::not_found(ResourceKind::Preset, id).into_response()
        }
        LibraryStoreError::PresetConflict(id) => {
            DomainError::conflict(format!("Preset already exists: {id}")).into_response()
        }
        LibraryStoreError::PlaylistNotFound(id) => {
            DomainError::not_found(ResourceKind::Playlist, id).into_response()
        }
        LibraryStoreError::PlaylistConflict(id) => {
            DomainError::conflict(format!("Playlist already exists: {id}")).into_response()
        }
        LibraryStoreError::Persistence(message) => {
            DomainError::Internal(anyhow::anyhow!(message.clone())).into_response()
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
