//! Library storage abstraction and in-memory implementation.
//!
//! The API uses [`LibraryStore`] so storage can move from in-memory to a
//! database backend (e.g. Turso/libsql) without rewriting handlers.

pub mod migration;

use std::collections::HashMap;
use std::path::PathBuf;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::warn;

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

/// Errors that can occur when opening a JSON-backed library store.
#[derive(Debug, thiserror::Error)]
pub enum JsonLibraryStoreOpenError {
    #[error("failed to read library snapshot at {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse library snapshot at {path}: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
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

/// Serialized snapshot format for [`JsonLibraryStore`].
#[derive(Debug, Serialize, Deserialize)]
#[serde(default)]
struct LibrarySnapshot {
    version: u32,
    favorites: Vec<FavoriteEffect>,
    presets: Vec<EffectPreset>,
    playlists: Vec<EffectPlaylist>,
}

impl Default for LibrarySnapshot {
    fn default() -> Self {
        Self {
            version: 1,
            favorites: Vec::new(),
            presets: Vec::new(),
            playlists: Vec::new(),
        }
    }
}

impl LibrarySnapshot {
    fn from_data(data: &InMemoryLibraryData) -> Self {
        let mut favorites: Vec<FavoriteEffect> = data.favorites.values().cloned().collect();
        favorites.sort_by(|left, right| {
            right
                .added_at_ms
                .cmp(&left.added_at_ms)
                .then_with(|| left.effect_id.to_string().cmp(&right.effect_id.to_string()))
        });

        let mut presets: Vec<EffectPreset> = data.presets.values().cloned().collect();
        presets.sort_by(|left, right| {
            right
                .updated_at_ms
                .cmp(&left.updated_at_ms)
                .then_with(|| left.name.cmp(&right.name))
        });

        let mut playlists: Vec<EffectPlaylist> = data.playlists.values().cloned().collect();
        playlists.sort_by(|left, right| {
            right
                .updated_at_ms
                .cmp(&left.updated_at_ms)
                .then_with(|| left.name.cmp(&right.name))
        });

        Self {
            version: 1,
            favorites,
            presets,
            playlists,
        }
    }

    fn into_data(self) -> InMemoryLibraryData {
        InMemoryLibraryData {
            favorites: self
                .favorites
                .into_iter()
                .map(|favorite| (favorite.effect_id, favorite))
                .collect(),
            presets: self
                .presets
                .into_iter()
                .map(|preset| (preset.id, preset))
                .collect(),
            playlists: self
                .playlists
                .into_iter()
                .map(|playlist| (playlist.id, playlist))
                .collect(),
        }
    }
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

#[derive(Debug, thiserror::Error)]
enum JsonPersistError {
    #[error("failed to serialize snapshot: {0}")]
    Serialize(#[source] serde_json::Error),
    #[error("failed to create snapshot directory {path}: {source}")]
    CreateDir {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to write temporary snapshot {path}: {source}")]
    WriteTemp {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to replace snapshot file {path}: {source}")]
    Replace {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// JSON-backed persistence for library entities.
///
/// This store keeps an in-memory index for fast reads and writes a full
/// snapshot to disk after each mutation.
#[derive(Debug)]
pub struct JsonLibraryStore {
    path: PathBuf,
    data: RwLock<InMemoryLibraryData>,
}

impl JsonLibraryStore {
    /// Open a JSON-backed store at `path`, loading existing data when present.
    ///
    /// # Errors
    ///
    /// Returns an error if an existing snapshot cannot be read or parsed.
    pub fn open(path: PathBuf) -> Result<Self, JsonLibraryStoreOpenError> {
        let data = if path.exists() {
            let raw = std::fs::read_to_string(&path).map_err(|source| {
                JsonLibraryStoreOpenError::Read {
                    path: path.clone(),
                    source,
                }
            })?;
            let snapshot: LibrarySnapshot =
                serde_json::from_str(&raw).map_err(|source| JsonLibraryStoreOpenError::Parse {
                    path: path.clone(),
                    source,
                })?;
            snapshot.into_data()
        } else {
            InMemoryLibraryData::default()
        };

        Ok(Self {
            path,
            data: RwLock::new(data),
        })
    }

    fn persist_best_effort(&self, snapshot: &LibrarySnapshot) {
        if let Err(error) = self.persist_snapshot(snapshot) {
            warn!(
                path = %self.path.display(),
                %error,
                "Failed to persist library snapshot; keeping in-memory state"
            );
        }
    }

    fn persist_snapshot(&self, snapshot: &LibrarySnapshot) -> Result<(), JsonPersistError> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|source| JsonPersistError::CreateDir {
                path: parent.to_path_buf(),
                source,
            })?;
        }

        let bytes = serde_json::to_vec_pretty(snapshot).map_err(JsonPersistError::Serialize)?;
        let tmp_path = self.path.with_extension("json.tmp");

        std::fs::write(&tmp_path, bytes).map_err(|source| JsonPersistError::WriteTemp {
            path: tmp_path.clone(),
            source,
        })?;
        std::fs::rename(&tmp_path, &self.path).map_err(|source| JsonPersistError::Replace {
            path: self.path.clone(),
            source,
        })?;
        Ok(())
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

#[async_trait]
impl LibraryStore for JsonLibraryStore {
    async fn list_favorites(&self) -> Vec<FavoriteEffect> {
        let data = self.data.read().await;
        let mut favorites: Vec<FavoriteEffect> = data.favorites.values().cloned().collect();
        favorites.sort_by(|left, right| right.added_at_ms.cmp(&left.added_at_ms));
        favorites
    }

    async fn upsert_favorite(&self, effect_id: EffectId, added_at_ms: u64) -> FavoriteEffect {
        let (favorite, snapshot) = {
            let mut data = self.data.write().await;
            let favorite = FavoriteEffect {
                effect_id,
                added_at_ms,
            };
            data.favorites.insert(effect_id, favorite.clone());
            (favorite, LibrarySnapshot::from_data(&data))
        };
        self.persist_best_effort(&snapshot);
        favorite
    }

    async fn remove_favorite(&self, effect_id: EffectId) -> bool {
        let (removed, snapshot) = {
            let mut data = self.data.write().await;
            let removed = data.favorites.remove(&effect_id).is_some();
            let snapshot = removed.then(|| LibrarySnapshot::from_data(&data));
            (removed, snapshot)
        };
        if let Some(snapshot) = snapshot {
            self.persist_best_effort(&snapshot);
        }
        removed
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
        let snapshot = {
            let mut data = self.data.write().await;
            if data.presets.contains_key(&preset.id) {
                return Err(LibraryStoreError::PresetConflict(preset.id));
            }
            data.presets.insert(preset.id, preset);
            LibrarySnapshot::from_data(&data)
        };
        self.persist_best_effort(&snapshot);
        Ok(())
    }

    async fn update_preset(&self, preset: EffectPreset) -> Result<(), LibraryStoreError> {
        let snapshot = {
            let mut data = self.data.write().await;
            if !data.presets.contains_key(&preset.id) {
                return Err(LibraryStoreError::PresetNotFound(preset.id));
            }
            data.presets.insert(preset.id, preset);
            LibrarySnapshot::from_data(&data)
        };
        self.persist_best_effort(&snapshot);
        Ok(())
    }

    async fn remove_preset(&self, id: PresetId) -> bool {
        let (removed, snapshot) = {
            let mut data = self.data.write().await;
            let removed = data.presets.remove(&id).is_some();
            let snapshot = removed.then(|| LibrarySnapshot::from_data(&data));
            (removed, snapshot)
        };
        if let Some(snapshot) = snapshot {
            self.persist_best_effort(&snapshot);
        }
        removed
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
        let snapshot = {
            let mut data = self.data.write().await;
            if data.playlists.contains_key(&playlist.id) {
                return Err(LibraryStoreError::PlaylistConflict(playlist.id));
            }
            data.playlists.insert(playlist.id, playlist);
            LibrarySnapshot::from_data(&data)
        };
        self.persist_best_effort(&snapshot);
        Ok(())
    }

    async fn update_playlist(&self, playlist: EffectPlaylist) -> Result<(), LibraryStoreError> {
        let snapshot = {
            let mut data = self.data.write().await;
            if !data.playlists.contains_key(&playlist.id) {
                return Err(LibraryStoreError::PlaylistNotFound(playlist.id));
            }
            data.playlists.insert(playlist.id, playlist);
            LibrarySnapshot::from_data(&data)
        };
        self.persist_best_effort(&snapshot);
        Ok(())
    }

    async fn remove_playlist(&self, id: PlaylistId) -> bool {
        let (removed, snapshot) = {
            let mut data = self.data.write().await;
            let removed = data.playlists.remove(&id).is_some();
            let snapshot = removed.then(|| LibrarySnapshot::from_data(&data));
            (removed, snapshot)
        };
        if let Some(snapshot) = snapshot {
            self.persist_best_effort(&snapshot);
        }
        removed
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::{InMemoryLibraryStore, JsonLibraryStore, JsonLibraryStoreOpenError, LibraryStore};
    use hypercolor_types::effect::EffectId;
    use hypercolor_types::library::{
        EffectPlaylist, EffectPreset, PlaylistId, PlaylistItem, PlaylistItemId, PlaylistItemTarget,
        PresetId,
    };
    use tempfile::TempDir;
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
        let fetched = store
            .get_preset(preset.id)
            .await
            .expect("preset should exist");
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

    #[tokio::test]
    async fn json_store_persists_and_reloads() {
        let tempdir = TempDir::new().expect("tempdir");
        let path = tempdir.path().join("library.json");

        let first = JsonLibraryStore::open(path.clone()).expect("open first json store");

        let effect_id = EffectId::new(Uuid::now_v7());
        first.upsert_favorite(effect_id, 111).await;

        let preset = EffectPreset {
            id: PresetId::new(),
            name: "Persisted Preset".to_owned(),
            description: Some("desc".to_owned()),
            effect_id,
            controls: HashMap::new(),
            tags: vec!["tag".to_owned()],
            created_at_ms: 1,
            updated_at_ms: 2,
        };
        first
            .insert_preset(preset.clone())
            .await
            .expect("insert preset");

        let playlist = EffectPlaylist {
            id: PlaylistId::new(),
            name: "Persisted Playlist".to_owned(),
            description: None,
            items: vec![PlaylistItem {
                id: PlaylistItemId::new(),
                target: PlaylistItemTarget::Preset {
                    preset_id: preset.id,
                },
                duration_ms: Some(2_000),
                transition_ms: Some(100),
            }],
            loop_enabled: true,
            created_at_ms: 3,
            updated_at_ms: 4,
        };
        first
            .insert_playlist(playlist.clone())
            .await
            .expect("insert playlist");

        let second = JsonLibraryStore::open(path).expect("re-open json store");
        let favorites = second.list_favorites().await;
        let presets = second.list_presets().await;
        let playlists = second.list_playlists().await;

        assert_eq!(favorites.len(), 1);
        assert_eq!(favorites[0].effect_id, effect_id);
        assert_eq!(presets.len(), 1);
        assert_eq!(presets[0].id, preset.id);
        assert_eq!(playlists.len(), 1);
        assert_eq!(playlists[0].id, playlist.id);
    }

    #[test]
    fn json_store_open_fails_for_invalid_json() {
        let tempdir = TempDir::new().expect("tempdir");
        let path = tempdir.path().join("library.json");
        std::fs::write(&path, "{ not json").expect("write invalid json");

        let error = JsonLibraryStore::open(path).expect_err("expected parse error");
        assert!(matches!(error, JsonLibraryStoreOpenError::Parse { .. }));
    }
}
