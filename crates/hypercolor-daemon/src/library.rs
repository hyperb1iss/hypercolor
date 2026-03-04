//! Library storage abstraction and in-memory implementation.
//!
//! The API uses [`LibraryStore`] so storage can move from in-memory to a
//! database backend (e.g. Turso/libsql) without rewriting handlers.

use std::collections::HashMap;

use async_trait::async_trait;
use tokio::sync::RwLock;

use hypercolor_types::effect::EffectId;
use hypercolor_types::library::{
    EffectPlaylist, EffectPreset, FavoriteEffect, PlaylistId, PresetId,
};

/// Storage-layer errors for library entities.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LibraryStoreError {
    #[error("preset not found: {0}")]
    PresetNotFound(PresetId),
    #[error("preset already exists: {0}")]
    PresetConflict(PresetId),
    #[error("playlist not found: {0}")]
    PlaylistNotFound(PlaylistId),
    #[error("playlist already exists: {0}")]
    PlaylistConflict(PlaylistId),
}

/// Persistence contract for saved effect library data.
#[async_trait]
pub trait LibraryStore: Send + Sync {
    async fn list_favorites(&self) -> Vec<FavoriteEffect>;
    async fn upsert_favorite(&self, effect_id: EffectId, added_at_ms: u64) -> FavoriteEffect;
    async fn remove_favorite(&self, effect_id: EffectId) -> bool;

    async fn list_presets(&self) -> Vec<EffectPreset>;
    async fn get_preset(&self, id: PresetId) -> Option<EffectPreset>;
    async fn insert_preset(&self, preset: EffectPreset) -> Result<(), LibraryStoreError>;
    async fn update_preset(&self, preset: EffectPreset) -> Result<(), LibraryStoreError>;
    async fn remove_preset(&self, id: PresetId) -> bool;

    async fn list_playlists(&self) -> Vec<EffectPlaylist>;
    async fn get_playlist(&self, id: PlaylistId) -> Option<EffectPlaylist>;
    async fn insert_playlist(&self, playlist: EffectPlaylist) -> Result<(), LibraryStoreError>;
    async fn update_playlist(&self, playlist: EffectPlaylist) -> Result<(), LibraryStoreError>;
    async fn remove_playlist(&self, id: PlaylistId) -> bool;
}

#[derive(Debug, Default)]
struct InMemoryLibraryData {
    favorites: HashMap<EffectId, FavoriteEffect>,
    presets: HashMap<PresetId, EffectPreset>,
    playlists: HashMap<PlaylistId, EffectPlaylist>,
}

/// In-memory storage backend for library entities.
#[derive(Debug, Default)]
pub struct InMemoryLibraryStore {
    data: RwLock<InMemoryLibraryData>,
}

impl InMemoryLibraryStore {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl LibraryStore for InMemoryLibraryStore {
    async fn list_favorites(&self) -> Vec<FavoriteEffect> {
        let data = self.data.read().await;
        let mut favorites: Vec<FavoriteEffect> = data.favorites.values().cloned().collect();
        favorites.sort_by(|left, right| right.added_at_ms.cmp(&left.added_at_ms));
        favorites
    }

    async fn upsert_favorite(&self, effect_id: EffectId, added_at_ms: u64) -> FavoriteEffect {
        let mut data = self.data.write().await;
        let favorite = FavoriteEffect {
            effect_id,
            added_at_ms,
        };
        data.favorites.insert(effect_id, favorite.clone());
        favorite
    }

    async fn remove_favorite(&self, effect_id: EffectId) -> bool {
        let mut data = self.data.write().await;
        data.favorites.remove(&effect_id).is_some()
    }

    async fn list_presets(&self) -> Vec<EffectPreset> {
        let data = self.data.read().await;
        let mut presets: Vec<EffectPreset> = data.presets.values().cloned().collect();
        presets.sort_by(|left, right| {
            right
                .updated_at_ms
                .cmp(&left.updated_at_ms)
                .then_with(|| left.name.cmp(&right.name))
        });
        presets
    }

    async fn get_preset(&self, id: PresetId) -> Option<EffectPreset> {
        let data = self.data.read().await;
        data.presets.get(&id).cloned()
    }

    async fn insert_preset(&self, preset: EffectPreset) -> Result<(), LibraryStoreError> {
        let mut data = self.data.write().await;
        if data.presets.contains_key(&preset.id) {
            return Err(LibraryStoreError::PresetConflict(preset.id));
        }
        data.presets.insert(preset.id, preset);
        Ok(())
    }

    async fn update_preset(&self, preset: EffectPreset) -> Result<(), LibraryStoreError> {
        let mut data = self.data.write().await;
        if !data.presets.contains_key(&preset.id) {
            return Err(LibraryStoreError::PresetNotFound(preset.id));
        }
        data.presets.insert(preset.id, preset);
        Ok(())
    }

    async fn remove_preset(&self, id: PresetId) -> bool {
        let mut data = self.data.write().await;
        data.presets.remove(&id).is_some()
    }

    async fn list_playlists(&self) -> Vec<EffectPlaylist> {
        let data = self.data.read().await;
        let mut playlists: Vec<EffectPlaylist> = data.playlists.values().cloned().collect();
        playlists.sort_by(|left, right| {
            right
                .updated_at_ms
                .cmp(&left.updated_at_ms)
                .then_with(|| left.name.cmp(&right.name))
        });
        playlists
    }

    async fn get_playlist(&self, id: PlaylistId) -> Option<EffectPlaylist> {
        let data = self.data.read().await;
        data.playlists.get(&id).cloned()
    }

    async fn insert_playlist(&self, playlist: EffectPlaylist) -> Result<(), LibraryStoreError> {
        let mut data = self.data.write().await;
        if data.playlists.contains_key(&playlist.id) {
            return Err(LibraryStoreError::PlaylistConflict(playlist.id));
        }
        data.playlists.insert(playlist.id, playlist);
        Ok(())
    }

    async fn update_playlist(&self, playlist: EffectPlaylist) -> Result<(), LibraryStoreError> {
        let mut data = self.data.write().await;
        if !data.playlists.contains_key(&playlist.id) {
            return Err(LibraryStoreError::PlaylistNotFound(playlist.id));
        }
        data.playlists.insert(playlist.id, playlist);
        Ok(())
    }

    async fn remove_playlist(&self, id: PlaylistId) -> bool {
        let mut data = self.data.write().await;
        data.playlists.remove(&id).is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::{InMemoryLibraryStore, LibraryStore};
    use hypercolor_types::effect::EffectId;
    use hypercolor_types::library::{
        EffectPlaylist, EffectPreset, PlaylistId, PlaylistItem, PlaylistItemId, PlaylistItemTarget,
        PresetId,
    };
    use uuid::Uuid;

    #[tokio::test]
    async fn favorites_upsert_and_remove() {
        let store = InMemoryLibraryStore::new();
        let effect_id = EffectId::new(Uuid::now_v7());

        let favorite = store.upsert_favorite(effect_id, 10).await;
        assert_eq!(favorite.effect_id, effect_id);

        let listed = store.list_favorites().await;
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].added_at_ms, 10);

        assert!(store.remove_favorite(effect_id).await);
        assert!(!store.remove_favorite(effect_id).await);
    }

    #[tokio::test]
    async fn presets_insert_update_and_get() {
        let store = InMemoryLibraryStore::new();
        let preset = EffectPreset {
            id: PresetId::new(),
            name: "Test Preset".to_owned(),
            description: None,
            effect_id: EffectId::new(Uuid::now_v7()),
            controls: std::collections::HashMap::new(),
            tags: Vec::new(),
            created_at_ms: 1,
            updated_at_ms: 1,
        };

        store
            .insert_preset(preset.clone())
            .await
            .expect("insert preset");
        let fetched = store.get_preset(preset.id).await.expect("preset should exist");
        assert_eq!(fetched.name, "Test Preset");

        let mut updated = fetched.clone();
        updated.name = "Updated Preset".to_owned();
        updated.updated_at_ms = 2;
        store
            .update_preset(updated.clone())
            .await
            .expect("update preset");
        let fetched_again = store
            .get_preset(updated.id)
            .await
            .expect("updated preset should exist");
        assert_eq!(fetched_again.name, "Updated Preset");
    }

    #[tokio::test]
    async fn playlists_insert_update_and_remove() {
        let store = InMemoryLibraryStore::new();
        let playlist = EffectPlaylist {
            id: PlaylistId::new(),
            name: "Playlist".to_owned(),
            description: None,
            items: vec![PlaylistItem {
                id: PlaylistItemId::new(),
                target: PlaylistItemTarget::Effect {
                    effect_id: EffectId::new(Uuid::now_v7()),
                },
                duration_ms: Some(1_000),
                transition_ms: Some(300),
            }],
            loop_enabled: true,
            created_at_ms: 1,
            updated_at_ms: 1,
        };

        store
            .insert_playlist(playlist.clone())
            .await
            .expect("insert playlist");
        assert!(store.get_playlist(playlist.id).await.is_some());

        let mut updated = playlist.clone();
        updated.loop_enabled = false;
        updated.updated_at_ms = 2;
        store
            .update_playlist(updated.clone())
            .await
            .expect("update playlist");
        let fetched = store
            .get_playlist(updated.id)
            .await
            .expect("updated playlist should exist");
        assert!(!fetched.loop_enabled);

        assert!(store.remove_playlist(updated.id).await);
        assert!(!store.remove_playlist(updated.id).await);
    }
}
