//! Saved effect library endpoints — `/api/v1/library/*`.

mod favorites;
mod playlists;
mod presets;

pub use favorites::*;
pub use playlists::*;
pub use presets::*;

use std::sync::Arc;

use hypercolor_types::effect::EffectId;
use hypercolor_types::library::PresetId;

use crate::app_state::AppState;
use crate::domain::effect::ResolvedEffect;
use crate::domain::{DomainError, ResourceKind};
use crate::library::LibraryStoreError;

// ── Shared Helpers ──────────────────────────────────────────────────────

pub(crate) async fn resolve_preset_id(state: &Arc<AppState>, id_or_name: &str) -> Option<PresetId> {
    if let Ok(id) = id_or_name.parse::<PresetId>() {
        return Some(id);
    }

    state
        .library_store()
        .list_presets()
        .await
        .iter()
        .find(|preset| preset.name.eq_ignore_ascii_case(id_or_name))
        .map(|preset| preset.id)
}

pub(crate) async fn metadata_for_effect_id(
    state: &Arc<AppState>,
    effect_id: EffectId,
) -> Result<ResolvedEffect, String> {
    let Some(metadata) = state.domains.effects.metadata_for_mutation(effect_id).await else {
        return Err(format!("effect not found: {effect_id}"));
    };
    Ok(metadata)
}

pub(crate) fn store_error(error: &LibraryStoreError) -> DomainError {
    match error {
        LibraryStoreError::PresetNotFound(id) => DomainError::not_found(ResourceKind::Preset, id),
        LibraryStoreError::PresetConflict(id) => {
            DomainError::conflict(format!("Preset already exists: {id}"))
        }
        LibraryStoreError::PlaylistNotFound(id) => {
            DomainError::not_found(ResourceKind::Playlist, id)
        }
        LibraryStoreError::PlaylistConflict(id) => {
            DomainError::conflict(format!("Playlist already exists: {id}"))
        }
        LibraryStoreError::Persistence(message) => {
            DomainError::Internal(anyhow::anyhow!(message.clone()))
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
