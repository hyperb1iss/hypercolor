//! Library storage abstraction and in-memory implementation.
//!
//! The API uses [`LibraryStore`] so storage can move from in-memory to a
//! database backend (e.g. Turso/libsql) without rewriting handlers.

use std::cmp::Reverse;
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

use crate::persistence::{
    AdmittedAtomicWrite, AtomicFileWriter, AtomicWriteCommitResult, PersistenceError,
    serialize_json_pretty,
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
    #[error("library persistence preparation failed: {0}")]
    Persistence(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConditionalFavoriteMutation {
    Upsert { added_at_ms: u64 },
    Remove,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConditionalFavoriteMutationOutcome {
    Applied { revision: u64 },
    AlreadyCurrent { revision: u64 },
    ConcurrentLocalEdit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProjectedFavoriteState {
    pub effect_id: EffectId,
    pub added_at_ms: Option<u64>,
    pub revision: u64,
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
    #[error("failed to prepare library persistence at {path}: {source}")]
    PreparePersistence {
        path: PathBuf,
        #[source]
        source: PersistenceError,
    },
}

/// Persistence contract for saved effect library data.
#[async_trait]
pub trait LibraryStore: Send + Sync {
    async fn list_favorites(&self) -> Vec<FavoriteEffect>;
    async fn project_favorites(&self) -> Vec<ProjectedFavoriteState>;
    async fn upsert_favorite(
        &self,
        effect_id: EffectId,
        added_at_ms: u64,
    ) -> Result<FavoriteEffect, LibraryStoreError>;
    async fn remove_favorite(&self, effect_id: EffectId) -> Result<bool, LibraryStoreError>;
    async fn mutate_favorite_if_current(
        &self,
        effect_id: EffectId,
        expected_added_at_ms: Option<u64>,
        expected_revision: u64,
        mutation: ConditionalFavoriteMutation,
    ) -> Result<ConditionalFavoriteMutationOutcome, LibraryStoreError>;

    async fn list_presets(&self) -> Vec<EffectPreset>;
    async fn get_preset(&self, id: PresetId) -> Option<EffectPreset>;
    async fn insert_preset(&self, preset: EffectPreset) -> Result<(), LibraryStoreError>;
    async fn update_preset(&self, preset: EffectPreset) -> Result<(), LibraryStoreError>;
    async fn remove_preset(&self, id: PresetId) -> Result<bool, LibraryStoreError>;

    async fn list_playlists(&self) -> Vec<EffectPlaylist>;
    async fn get_playlist(&self, id: PlaylistId) -> Option<EffectPlaylist>;
    async fn insert_playlist(&self, playlist: EffectPlaylist) -> Result<(), LibraryStoreError>;
    async fn update_playlist(&self, playlist: EffectPlaylist) -> Result<(), LibraryStoreError>;
    async fn remove_playlist(&self, id: PlaylistId) -> Result<bool, LibraryStoreError>;
}

#[derive(Debug, Clone, Default)]
struct InMemoryLibraryData {
    favorites: HashMap<EffectId, FavoriteEffect>,
    favorite_revisions: HashMap<EffectId, u64>,
    presets: HashMap<PresetId, EffectPreset>,
    playlists: HashMap<PlaylistId, EffectPlaylist>,
}

impl InMemoryLibraryData {
    fn favorite_state(&self, effect_id: EffectId) -> Option<u64> {
        self.favorites
            .get(&effect_id)
            .map(|favorite| favorite.added_at_ms)
    }

    fn favorite_revision(&self, effect_id: EffectId) -> u64 {
        self.favorite_revisions
            .get(&effect_id)
            .copied()
            .unwrap_or_default()
    }

    fn projected_favorites(&self) -> Vec<ProjectedFavoriteState> {
        let mut projected: Vec<ProjectedFavoriteState> = self
            .favorite_revisions
            .iter()
            .map(|(effect_id, revision)| ProjectedFavoriteState {
                effect_id: *effect_id,
                added_at_ms: self.favorite_state(*effect_id),
                revision: *revision,
            })
            .collect();
        projected.sort_by_key(|state| state.effect_id.to_string());
        projected
    }

    fn apply_favorite_change(
        &mut self,
        effect_id: EffectId,
        change: &FavoriteChange,
    ) -> Result<Option<u64>, LibraryStoreError> {
        let changed = match change {
            FavoriteChange::Upsert(favorite) => self.favorites.get(&effect_id) != Some(favorite),
            FavoriteChange::Remove => self.favorites.contains_key(&effect_id),
        };
        if !changed {
            return Ok(None);
        }
        let revision = self
            .favorite_revision(effect_id)
            .checked_add(1)
            .ok_or_else(|| {
                LibraryStoreError::Persistence(format!("favorite revision exhausted: {effect_id}"))
            })?;
        match change {
            FavoriteChange::Upsert(favorite) => {
                self.favorites.insert(effect_id, favorite.clone());
            }
            FavoriteChange::Remove => {
                self.favorites.remove(&effect_id);
            }
        }
        self.favorite_revisions.insert(effect_id, revision);
        Ok(Some(revision))
    }
}

/// Serialized snapshot format for [`JsonLibraryStore`].
#[derive(Debug, Serialize, Deserialize)]
#[serde(default)]
struct LibrarySnapshot {
    version: u32,
    favorites: Vec<FavoriteEffect>,
    favorite_revisions: Vec<FavoriteRevisionRecord>,
    presets: Vec<EffectPreset>,
    playlists: Vec<EffectPlaylist>,
}

#[derive(Debug, Serialize, Deserialize)]
struct FavoriteRevisionRecord {
    effect_id: EffectId,
    revision: u64,
}

impl Default for LibrarySnapshot {
    fn default() -> Self {
        Self {
            version: 1,
            favorites: Vec::new(),
            favorite_revisions: Vec::new(),
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
        let mut favorite_revisions: Vec<FavoriteRevisionRecord> = data
            .favorite_revisions
            .iter()
            .map(|(effect_id, revision)| FavoriteRevisionRecord {
                effect_id: *effect_id,
                revision: *revision,
            })
            .collect();
        favorite_revisions.sort_by_key(|record| record.effect_id.to_string());

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
            favorite_revisions,
            presets,
            playlists,
        }
    }

    fn into_data(self) -> InMemoryLibraryData {
        let favorites: HashMap<EffectId, FavoriteEffect> = self
            .favorites
            .into_iter()
            .map(|favorite| (favorite.effect_id, favorite))
            .collect();
        let mut favorite_revisions: HashMap<EffectId, u64> = self
            .favorite_revisions
            .into_iter()
            .map(|record| (record.effect_id, record.revision))
            .collect();
        for effect_id in favorites.keys() {
            favorite_revisions.entry(*effect_id).or_insert(1);
        }
        InMemoryLibraryData {
            favorites,
            favorite_revisions,
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
    #[error("failed to persist snapshot file {path}: {source}")]
    Persist {
        path: PathBuf,
        #[source]
        source: PersistenceError,
    },
}

#[derive(Debug)]
struct PendingLibrarySnapshot {
    write: AdmittedAtomicWrite,
}

/// JSON-backed persistence for library entities.
///
/// This store keeps an in-memory index for fast reads and writes a full
/// snapshot to disk after each mutation.
#[derive(Debug)]
pub struct JsonLibraryStore {
    path: PathBuf,
    writer: AtomicFileWriter,
    data: RwLock<InMemoryLibraryData>,
    uncertain_favorites: std::sync::Mutex<HashMap<EffectId, UncertainFavoriteChange>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum FavoriteChange {
    Upsert(FavoriteEffect),
    Remove,
}

#[derive(Debug, Clone)]
struct UncertainFavoriteChange {
    change: FavoriteChange,
    revision: u64,
    durable: bool,
}

impl JsonLibraryStore {
    /// Open a JSON-backed store at `path`, loading existing data when present.
    ///
    /// # Errors
    ///
    /// Returns an error if an existing snapshot cannot be read or parsed.
    pub fn open(path: PathBuf) -> Result<Self, JsonLibraryStoreOpenError> {
        let snapshot_exists =
            path.try_exists()
                .map_err(|source| JsonLibraryStoreOpenError::Read {
                    path: path.clone(),
                    source,
                })?;
        let data = if snapshot_exists {
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
        let writer = AtomicFileWriter::new(&path).map_err(|source| {
            JsonLibraryStoreOpenError::PreparePersistence {
                path: path.clone(),
                source,
            }
        })?;

        Ok(Self {
            path,
            writer,
            data: RwLock::new(data),
            uncertain_favorites: std::sync::Mutex::new(HashMap::new()),
        })
    }

    fn pending_snapshot(
        &self,
        data: &InMemoryLibraryData,
    ) -> Result<PendingLibrarySnapshot, JsonPersistError> {
        let bytes = serialize_json_pretty(&LibrarySnapshot::from_data(data))
            .map_err(JsonPersistError::Serialize)?;
        Ok(PendingLibrarySnapshot {
            write: self.writer.reserve().admit(bytes),
        })
    }

    fn persist_best_effort(&self, pending: PendingLibrarySnapshot) {
        if let Err(error) = self.persist_snapshot(pending) {
            warn!(
                path = %self.path.display(),
                %error,
                "Failed to persist library snapshot; keeping in-memory state"
            );
        }
    }

    fn persist_snapshot(&self, pending: PendingLibrarySnapshot) -> Result<(), JsonPersistError> {
        pending
            .write
            .commit()
            .map_err(|source| JsonPersistError::Persist {
                path: self.path.clone(),
                source,
            })?;
        Ok(())
    }

    fn retain_snapshot(&self, data: &InMemoryLibraryData) {
        let Ok(pending) = self.pending_snapshot(data) else {
            return;
        };
        match pending.write.commit_stage_aware() {
            AtomicWriteCommitResult::Superseded | AtomicWriteCommitResult::DurableWritten => {}
            AtomicWriteCommitResult::FailedBeforeReplacement(error)
            | AtomicWriteCommitResult::ReplacementVisibleButNotDurable(error) => {
                warn!(
                    path = %self.path.display(),
                    %error,
                    "Failed to restore retained library snapshot; retry remains armed"
                );
            }
        }
    }

    fn favorite_change_is_visible(
        data: &InMemoryLibraryData,
        effect_id: EffectId,
        pending: &UncertainFavoriteChange,
    ) -> bool {
        data.favorite_revision(effect_id) == pending.revision
            && match &pending.change {
                FavoriteChange::Upsert(favorite) => {
                    data.favorites.get(&effect_id) == Some(favorite)
                }
                FavoriteChange::Remove => !data.favorites.contains_key(&effect_id),
            }
    }

    fn reconcile_uncertain_favorites(&self, data: &InMemoryLibraryData) {
        let mut uncertain = self
            .uncertain_favorites
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        uncertain.retain(|effect_id, pending| {
            if Self::favorite_change_is_visible(data, *effect_id, pending) {
                pending.durable = true;
                true
            } else {
                false
            }
        });
    }

    fn record_uncertain_favorite(
        &self,
        effect_id: EffectId,
        change: FavoriteChange,
        revision: u64,
    ) {
        self.uncertain_favorites
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(
                effect_id,
                UncertainFavoriteChange {
                    change,
                    revision,
                    durable: false,
                },
            );
    }

    fn matching_uncertain_favorite(
        &self,
        data: &InMemoryLibraryData,
        effect_id: EffectId,
        change: &FavoriteChange,
    ) -> Option<(bool, u64)> {
        self.uncertain_favorites
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&effect_id)
            .filter(|pending| {
                pending.change == *change && data.favorite_revision(effect_id) == pending.revision
            })
            .map(|pending| (pending.durable, pending.revision))
    }

    fn clear_uncertain_favorite(&self, effect_id: EffectId) {
        self.uncertain_favorites
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&effect_id);
    }

    fn retry_uncertain_favorite(
        &self,
        data: &InMemoryLibraryData,
        effect_id: EffectId,
        change: &FavoriteChange,
    ) -> Result<Option<u64>, LibraryStoreError> {
        let Some((durable, revision)) = self.matching_uncertain_favorite(data, effect_id, change)
        else {
            return Ok(None);
        };
        if durable {
            self.clear_uncertain_favorite(effect_id);
            return Ok(Some(revision));
        }

        let pending = self
            .pending_snapshot(data)
            .map_err(|error| LibraryStoreError::Persistence(error.to_string()))?;
        match pending.write.commit_stage_aware() {
            AtomicWriteCommitResult::DurableWritten => {
                self.reconcile_uncertain_favorites(data);
                self.clear_uncertain_favorite(effect_id);
                Ok(Some(revision))
            }
            AtomicWriteCommitResult::Superseded => Err(LibraryStoreError::Persistence(
                "favorite retry snapshot was superseded before replacement".to_owned(),
            )),
            AtomicWriteCommitResult::FailedBeforeReplacement(error)
            | AtomicWriteCommitResult::ReplacementVisibleButNotDurable(error) => {
                Err(LibraryStoreError::Persistence(error.to_string()))
            }
        }
    }

    fn apply_favorite_change(
        &self,
        data: &mut InMemoryLibraryData,
        effect_id: EffectId,
        change: FavoriteChange,
    ) -> Result<(bool, u64), LibraryStoreError> {
        if let Some(revision) = self.retry_uncertain_favorite(data, effect_id, &change)? {
            return Ok((true, revision));
        }

        let mut candidate = data.clone();
        let Some(revision) = candidate.apply_favorite_change(effect_id, &change)? else {
            self.writer.kick();
            return Ok((false, data.favorite_revision(effect_id)));
        };

        let pending = self
            .pending_snapshot(&candidate)
            .map_err(|error| LibraryStoreError::Persistence(error.to_string()))?;
        match pending.write.commit_stage_aware() {
            AtomicWriteCommitResult::DurableWritten => {
                *data = candidate;
                self.clear_uncertain_favorite(effect_id);
                self.reconcile_uncertain_favorites(data);
                Ok((true, revision))
            }
            AtomicWriteCommitResult::Superseded => Err(LibraryStoreError::Persistence(
                "favorite snapshot was superseded before replacement".to_owned(),
            )),
            AtomicWriteCommitResult::FailedBeforeReplacement(error) => {
                self.retain_snapshot(data);
                Err(LibraryStoreError::Persistence(error.to_string()))
            }
            AtomicWriteCommitResult::ReplacementVisibleButNotDurable(error) => {
                *data = candidate;
                self.record_uncertain_favorite(effect_id, change, revision);
                Err(LibraryStoreError::Persistence(error.to_string()))
            }
        }
    }
}

#[async_trait]
impl LibraryStore for InMemoryLibraryStore {
    async fn list_favorites(&self) -> Vec<FavoriteEffect> {
        let data = self.data.read().await;
        let mut favorites: Vec<FavoriteEffect> = data.favorites.values().cloned().collect();
        favorites.sort_by_key(|favorite| Reverse(favorite.added_at_ms));
        favorites
    }

    async fn project_favorites(&self) -> Vec<ProjectedFavoriteState> {
        self.data.read().await.projected_favorites()
    }

    async fn upsert_favorite(
        &self,
        effect_id: EffectId,
        added_at_ms: u64,
    ) -> Result<FavoriteEffect, LibraryStoreError> {
        let mut data = self.data.write().await;
        let favorite = FavoriteEffect {
            effect_id,
            added_at_ms,
        };
        data.apply_favorite_change(effect_id, &FavoriteChange::Upsert(favorite.clone()))?;
        Ok(favorite)
    }

    async fn remove_favorite(&self, effect_id: EffectId) -> Result<bool, LibraryStoreError> {
        let mut data = self.data.write().await;
        Ok(data
            .apply_favorite_change(effect_id, &FavoriteChange::Remove)?
            .is_some())
    }

    async fn mutate_favorite_if_current(
        &self,
        effect_id: EffectId,
        expected_added_at_ms: Option<u64>,
        expected_revision: u64,
        mutation: ConditionalFavoriteMutation,
    ) -> Result<ConditionalFavoriteMutationOutcome, LibraryStoreError> {
        let mut data = self.data.write().await;
        let current = data.favorite_state(effect_id);
        if current != expected_added_at_ms || data.favorite_revision(effect_id) != expected_revision
        {
            return Ok(ConditionalFavoriteMutationOutcome::ConcurrentLocalEdit);
        }

        let change = match mutation {
            ConditionalFavoriteMutation::Upsert { added_at_ms } => {
                FavoriteChange::Upsert(FavoriteEffect {
                    effect_id,
                    added_at_ms,
                })
            }
            ConditionalFavoriteMutation::Remove => FavoriteChange::Remove,
        };
        Ok(
            if let Some(revision) = data.apply_favorite_change(effect_id, &change)? {
                ConditionalFavoriteMutationOutcome::Applied { revision }
            } else {
                ConditionalFavoriteMutationOutcome::AlreadyCurrent {
                    revision: data.favorite_revision(effect_id),
                }
            },
        )
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

    async fn remove_preset(&self, id: PresetId) -> Result<bool, LibraryStoreError> {
        let mut data = self.data.write().await;
        Ok(data.presets.remove(&id).is_some())
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

    async fn remove_playlist(&self, id: PlaylistId) -> Result<bool, LibraryStoreError> {
        let mut data = self.data.write().await;
        Ok(data.playlists.remove(&id).is_some())
    }
}

#[async_trait]
impl LibraryStore for JsonLibraryStore {
    async fn list_favorites(&self) -> Vec<FavoriteEffect> {
        let data = self.data.read().await;
        let mut favorites: Vec<FavoriteEffect> = data.favorites.values().cloned().collect();
        favorites.sort_by_key(|favorite| Reverse(favorite.added_at_ms));
        favorites
    }

    async fn project_favorites(&self) -> Vec<ProjectedFavoriteState> {
        self.data.read().await.projected_favorites()
    }

    async fn upsert_favorite(
        &self,
        effect_id: EffectId,
        added_at_ms: u64,
    ) -> Result<FavoriteEffect, LibraryStoreError> {
        let mut data = self.data.write().await;
        let favorite = FavoriteEffect {
            effect_id,
            added_at_ms,
        };
        let change = FavoriteChange::Upsert(favorite.clone());
        self.apply_favorite_change(&mut data, effect_id, change)?;
        Ok(favorite)
    }

    async fn remove_favorite(&self, effect_id: EffectId) -> Result<bool, LibraryStoreError> {
        let mut data = self.data.write().await;
        let change = FavoriteChange::Remove;
        self.apply_favorite_change(&mut data, effect_id, change)
            .map(|(changed, _revision)| changed)
    }

    async fn mutate_favorite_if_current(
        &self,
        effect_id: EffectId,
        expected_added_at_ms: Option<u64>,
        expected_revision: u64,
        mutation: ConditionalFavoriteMutation,
    ) -> Result<ConditionalFavoriteMutationOutcome, LibraryStoreError> {
        let mut data = self.data.write().await;
        let change = match mutation {
            ConditionalFavoriteMutation::Upsert { added_at_ms } => {
                FavoriteChange::Upsert(FavoriteEffect {
                    effect_id,
                    added_at_ms,
                })
            }
            ConditionalFavoriteMutation::Remove => FavoriteChange::Remove,
        };
        if let Some(revision) = self.retry_uncertain_favorite(&data, effect_id, &change)? {
            return Ok(ConditionalFavoriteMutationOutcome::Applied { revision });
        }
        let current = data.favorite_state(effect_id);
        if current != expected_added_at_ms || data.favorite_revision(effect_id) != expected_revision
        {
            return Ok(ConditionalFavoriteMutationOutcome::ConcurrentLocalEdit);
        }
        let (changed, revision) = self.apply_favorite_change(&mut data, effect_id, change)?;
        Ok(if changed {
            ConditionalFavoriteMutationOutcome::Applied { revision }
        } else {
            ConditionalFavoriteMutationOutcome::AlreadyCurrent { revision }
        })
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
        let pending = {
            let mut data = self.data.write().await;
            if data.presets.contains_key(&preset.id) {
                self.writer.kick();
                return Err(LibraryStoreError::PresetConflict(preset.id));
            }
            let mut candidate = data.clone();
            candidate.presets.insert(preset.id, preset);
            let pending = self
                .pending_snapshot(&candidate)
                .map_err(|error| LibraryStoreError::Persistence(error.to_string()))?;
            *data = candidate;
            pending
        };
        self.persist_best_effort(pending);
        Ok(())
    }

    async fn update_preset(&self, preset: EffectPreset) -> Result<(), LibraryStoreError> {
        let pending = {
            let mut data = self.data.write().await;
            if !data.presets.contains_key(&preset.id) {
                self.writer.kick();
                return Err(LibraryStoreError::PresetNotFound(preset.id));
            }
            let mut candidate = data.clone();
            candidate.presets.insert(preset.id, preset);
            let pending = self
                .pending_snapshot(&candidate)
                .map_err(|error| LibraryStoreError::Persistence(error.to_string()))?;
            *data = candidate;
            pending
        };
        self.persist_best_effort(pending);
        Ok(())
    }

    async fn remove_preset(&self, id: PresetId) -> Result<bool, LibraryStoreError> {
        let (removed, pending) = {
            let mut data = self.data.write().await;
            let mut candidate = data.clone();
            let removed = candidate.presets.remove(&id).is_some();
            let pending = if removed {
                let pending = self
                    .pending_snapshot(&candidate)
                    .map_err(|error| LibraryStoreError::Persistence(error.to_string()))?;
                *data = candidate;
                Some(pending)
            } else {
                None
            };
            (removed, pending)
        };
        if let Some(pending) = pending {
            self.persist_best_effort(pending);
        } else {
            self.writer.kick();
        }
        Ok(removed)
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
        let pending = {
            let mut data = self.data.write().await;
            if data.playlists.contains_key(&playlist.id) {
                self.writer.kick();
                return Err(LibraryStoreError::PlaylistConflict(playlist.id));
            }
            let mut candidate = data.clone();
            candidate.playlists.insert(playlist.id, playlist);
            let pending = self
                .pending_snapshot(&candidate)
                .map_err(|error| LibraryStoreError::Persistence(error.to_string()))?;
            *data = candidate;
            pending
        };
        self.persist_best_effort(pending);
        Ok(())
    }

    async fn update_playlist(&self, playlist: EffectPlaylist) -> Result<(), LibraryStoreError> {
        let pending = {
            let mut data = self.data.write().await;
            if !data.playlists.contains_key(&playlist.id) {
                self.writer.kick();
                return Err(LibraryStoreError::PlaylistNotFound(playlist.id));
            }
            let mut candidate = data.clone();
            candidate.playlists.insert(playlist.id, playlist);
            let pending = self
                .pending_snapshot(&candidate)
                .map_err(|error| LibraryStoreError::Persistence(error.to_string()))?;
            *data = candidate;
            pending
        };
        self.persist_best_effort(pending);
        Ok(())
    }

    async fn remove_playlist(&self, id: PlaylistId) -> Result<bool, LibraryStoreError> {
        let (removed, pending) = {
            let mut data = self.data.write().await;
            let mut candidate = data.clone();
            let removed = candidate.playlists.remove(&id).is_some();
            let pending = if removed {
                let pending = self
                    .pending_snapshot(&candidate)
                    .map_err(|error| LibraryStoreError::Persistence(error.to_string()))?;
                *data = candidate;
                Some(pending)
            } else {
                None
            };
            (removed, pending)
        };
        if let Some(pending) = pending {
            self.persist_best_effort(pending);
        } else {
            self.writer.kick();
        }
        Ok(removed)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::{
        ConditionalFavoriteMutation, ConditionalFavoriteMutationOutcome, InMemoryLibraryStore,
        JsonLibraryStore, JsonLibraryStoreOpenError, LibraryStore, ProjectedFavoriteState,
    };
    use hypercolor_types::effect::EffectId;
    use hypercolor_types::library::{
        EffectPlaylist, EffectPreset, FavoriteEffect, PlaylistId, PlaylistItem, PlaylistItemId,
        PlaylistItemTarget, PresetId,
    };
    use tempfile::TempDir;
    use uuid::Uuid;

    async fn projected_favorite(
        store: &impl LibraryStore,
        effect_id: EffectId,
    ) -> ProjectedFavoriteState {
        store
            .project_favorites()
            .await
            .into_iter()
            .find(|state| state.effect_id == effect_id)
            .unwrap_or(ProjectedFavoriteState {
                effect_id,
                added_at_ms: None,
                revision: 0,
            })
    }

    #[tokio::test]
    async fn favorites_upsert_and_remove() {
        let store = InMemoryLibraryStore::new();
        let effect_id = EffectId::new(Uuid::now_v7());

        let favorite = store
            .upsert_favorite(effect_id, 10)
            .await
            .expect("upsert favorite");
        assert_eq!(favorite.effect_id, effect_id);

        let listed = store.list_favorites().await;
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].added_at_ms, 10);

        assert!(
            store
                .remove_favorite(effect_id)
                .await
                .expect("remove favorite")
        );
        assert!(
            !store
                .remove_favorite(effect_id)
                .await
                .expect("remove missing favorite")
        );
    }

    #[tokio::test]
    async fn in_memory_favorite_tokens_detect_aba_and_ignore_unrelated_entities() {
        let store = InMemoryLibraryStore::new();
        let effect_id = EffectId::new(Uuid::now_v7());
        let unrelated_id = EffectId::new(Uuid::now_v7());
        let absent = projected_favorite(&store, effect_id).await;

        store
            .upsert_favorite(effect_id, 10)
            .await
            .expect("local add");
        assert!(
            store
                .remove_favorite(effect_id)
                .await
                .expect("local remove")
        );
        assert_eq!(
            store
                .mutate_favorite_if_current(
                    effect_id,
                    absent.added_at_ms,
                    absent.revision,
                    ConditionalFavoriteMutation::Upsert { added_at_ms: 20 },
                )
                .await
                .expect("conditional remote add"),
            ConditionalFavoriteMutationOutcome::ConcurrentLocalEdit
        );

        let tombstone = projected_favorite(&store, effect_id).await;
        assert_eq!(tombstone.added_at_ms, None);
        assert_eq!(tombstone.revision, 2);
        let ConditionalFavoriteMutationOutcome::Applied {
            revision: present_revision,
        } = store
            .mutate_favorite_if_current(
                effect_id,
                tombstone.added_at_ms,
                tombstone.revision,
                ConditionalFavoriteMutation::Upsert { added_at_ms: 10 },
            )
            .await
            .expect("remote add from current tombstone")
        else {
            panic!("current tombstone must apply")
        };

        assert!(
            store
                .remove_favorite(effect_id)
                .await
                .expect("local remove")
        );
        store
            .upsert_favorite(effect_id, 10)
            .await
            .expect("local re-add");
        assert_eq!(
            store
                .mutate_favorite_if_current(
                    effect_id,
                    Some(10),
                    present_revision,
                    ConditionalFavoriteMutation::Remove,
                )
                .await
                .expect("conditional remote remove"),
            ConditionalFavoriteMutationOutcome::ConcurrentLocalEdit
        );

        let current = projected_favorite(&store, effect_id).await;
        store
            .upsert_favorite(unrelated_id, 30)
            .await
            .expect("unrelated add");
        let first = store
            .mutate_favorite_if_current(
                effect_id,
                current.added_at_ms,
                current.revision,
                ConditionalFavoriteMutation::Upsert { added_at_ms: 11 },
            )
            .await
            .expect("unrelated mutation must not conflict");
        let ConditionalFavoriteMutationOutcome::Applied { revision } = first else {
            panic!("first sequential remote row must apply")
        };
        assert_eq!(
            store
                .mutate_favorite_if_current(
                    effect_id,
                    Some(11),
                    revision,
                    ConditionalFavoriteMutation::Remove,
                )
                .await
                .expect("second sequential remote row"),
            ConditionalFavoriteMutationOutcome::Applied {
                revision: revision + 1,
            }
        );
    }

    #[tokio::test]
    async fn json_favorite_tombstone_revision_survives_reopen_and_blocks_aba() {
        let tempdir = TempDir::new().expect("tempdir");
        let path = tempdir.path().join("library.json");
        let effect_id = EffectId::new(Uuid::now_v7());
        let store = JsonLibraryStore::open(path.clone()).expect("open json store");
        let absent = projected_favorite(&store, effect_id).await;

        store
            .upsert_favorite(effect_id, 10)
            .await
            .expect("local add");
        assert!(
            store
                .remove_favorite(effect_id)
                .await
                .expect("local remove")
        );
        drop(store);

        let reopened = JsonLibraryStore::open(path).expect("reopen json store");
        assert_eq!(projected_favorite(&reopened, effect_id).await.revision, 2);
        assert_eq!(
            reopened
                .mutate_favorite_if_current(
                    effect_id,
                    absent.added_at_ms,
                    absent.revision,
                    ConditionalFavoriteMutation::Upsert { added_at_ms: 20 },
                )
                .await
                .expect("conditional remote add"),
            ConditionalFavoriteMutationOutcome::ConcurrentLocalEdit
        );
    }

    #[tokio::test]
    async fn json_favorite_tokens_detect_present_aba_and_support_sequential_rows() {
        let tempdir = TempDir::new().expect("tempdir");
        let path = tempdir.path().join("library.json");
        let store = JsonLibraryStore::open(path).expect("open json store");
        let effect_id = EffectId::new(Uuid::now_v7());
        let unrelated_id = EffectId::new(Uuid::now_v7());
        store
            .upsert_favorite(effect_id, 10)
            .await
            .expect("seed favorite");
        let projected = projected_favorite(&store, effect_id).await;

        assert!(
            store
                .remove_favorite(effect_id)
                .await
                .expect("local remove")
        );
        store
            .upsert_favorite(effect_id, 10)
            .await
            .expect("local re-add");
        assert_eq!(
            store
                .mutate_favorite_if_current(
                    effect_id,
                    projected.added_at_ms,
                    projected.revision,
                    ConditionalFavoriteMutation::Remove,
                )
                .await
                .expect("conditional remote remove"),
            ConditionalFavoriteMutationOutcome::ConcurrentLocalEdit
        );

        let current = projected_favorite(&store, effect_id).await;
        store
            .upsert_favorite(unrelated_id, 30)
            .await
            .expect("unrelated add");
        let ConditionalFavoriteMutationOutcome::Applied { revision } = store
            .mutate_favorite_if_current(
                effect_id,
                current.added_at_ms,
                current.revision,
                ConditionalFavoriteMutation::Upsert { added_at_ms: 11 },
            )
            .await
            .expect("first sequential remote row")
        else {
            panic!("first sequential remote row must apply")
        };
        assert_eq!(
            store
                .mutate_favorite_if_current(
                    effect_id,
                    Some(11),
                    revision,
                    ConditionalFavoriteMutation::Remove,
                )
                .await
                .expect("second sequential remote row"),
            ConditionalFavoriteMutationOutcome::Applied {
                revision: revision + 1,
            }
        );
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

        assert!(
            store
                .remove_playlist(updated.id)
                .await
                .expect("remove playlist")
        );
        assert!(
            !store
                .remove_playlist(updated.id)
                .await
                .expect("remove missing playlist")
        );
    }

    #[tokio::test]
    async fn json_store_persists_and_reloads() {
        let tempdir = TempDir::new().expect("tempdir");
        let path = tempdir.path().join("library.json");

        let first = JsonLibraryStore::open(path.clone()).expect("open first json store");

        let effect_id = EffectId::new(Uuid::now_v7());
        first
            .upsert_favorite(effect_id, 111)
            .await
            .expect("upsert favorite");

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

    #[tokio::test]
    async fn json_store_does_not_resurrect_a_superseded_snapshot() {
        let tempdir = TempDir::new().expect("tempdir");
        let path = tempdir.path().join("library.json");
        let store = JsonLibraryStore::open(path.clone()).expect("open json store");
        let effect_id = EffectId::new(Uuid::now_v7());

        let older = {
            let mut data = store.data.write().await;
            data.favorites.insert(
                effect_id,
                FavoriteEffect {
                    effect_id,
                    added_at_ms: 1,
                },
            );
            store.pending_snapshot(&data)
        };
        let newer = {
            let mut data = store.data.write().await;
            data.favorites.remove(&effect_id);
            store.pending_snapshot(&data)
        };

        store
            .persist_snapshot(newer.expect("prepare newer snapshot"))
            .expect("persist newer snapshot");
        store
            .persist_snapshot(older.expect("prepare older snapshot"))
            .expect("discard older snapshot");

        let reopened = JsonLibraryStore::open(path).expect("reopen json store");
        assert!(reopened.list_favorites().await.is_empty());
    }

    #[cfg(feature = "persistence-test-hooks")]
    #[tokio::test]
    async fn favorite_upsert_replace_failure_keeps_memory_and_disk_unchanged() {
        let tempdir = TempDir::new().expect("tempdir");
        let path = tempdir.path().join("library.json");
        let store = JsonLibraryStore::open(path.clone()).expect("open json store");
        let effect_id = EffectId::new(Uuid::now_v7());
        store.writer.set_injected_replace_failures(2);

        let error = store
            .upsert_favorite(effect_id, 10)
            .await
            .expect_err("replace failure must reject favorite");

        assert!(matches!(error, super::LibraryStoreError::Persistence(_)));
        assert!(store.list_favorites().await.is_empty());
        store
            .writer
            .flush(std::time::Duration::from_secs(5))
            .expect("retained snapshot should converge");
        let reopened = JsonLibraryStore::open(path).expect("reopen json store");
        assert!(reopened.list_favorites().await.is_empty());
    }

    #[cfg(all(unix, feature = "persistence-test-hooks"))]
    #[tokio::test]
    async fn favorite_directory_sync_failure_reconciles_memory_to_visible_disk() {
        let tempdir = TempDir::new().expect("tempdir");
        let path = tempdir.path().join("library.json");
        let store = JsonLibraryStore::open(path.clone()).expect("open json store");
        let effect_id = EffectId::new(Uuid::now_v7());
        store.writer.set_injected_directory_sync_failures(1);

        let error = store
            .upsert_favorite(effect_id, 10)
            .await
            .expect_err("directory sync failure must remain observable");

        assert!(matches!(error, super::LibraryStoreError::Persistence(_)));
        let live = store.list_favorites().await;
        assert_eq!(live.len(), 1);
        assert_eq!(live[0].effect_id, effect_id);
        let visible = JsonLibraryStore::open(path.clone()).expect("visible snapshot should parse");
        assert_eq!(visible.list_favorites().await, live);

        let converged = store
            .upsert_favorite(effect_id, 10)
            .await
            .expect("retry should report success after durable convergence");
        assert_eq!(converged.effect_id, effect_id);
        store
            .writer
            .flush(std::time::Duration::from_secs(5))
            .expect("visible candidate should reach the durability barrier");
        let durable = JsonLibraryStore::open(path).expect("durable snapshot should parse");
        assert_eq!(durable.list_favorites().await, live);
    }

    #[cfg(all(unix, feature = "persistence-test-hooks"))]
    #[tokio::test]
    async fn favorite_remove_directory_sync_retry_returns_true_exactly_once() {
        let tempdir = TempDir::new().expect("tempdir");
        let path = tempdir.path().join("library.json");
        let store = JsonLibraryStore::open(path.clone()).expect("open json store");
        let effect_id = EffectId::new(Uuid::now_v7());
        store
            .upsert_favorite(effect_id, 10)
            .await
            .expect("seed favorite");
        store.writer.set_injected_directory_sync_failures(1);

        let error = store
            .remove_favorite(effect_id)
            .await
            .expect_err("directory sync failure must remain observable");

        assert!(matches!(error, super::LibraryStoreError::Persistence(_)));
        assert!(store.list_favorites().await.is_empty());
        let visible = JsonLibraryStore::open(path.clone()).expect("visible snapshot should parse");
        assert!(visible.list_favorites().await.is_empty());
        assert!(
            store
                .remove_favorite(effect_id)
                .await
                .expect("retry should converge")
        );
        assert!(
            !store
                .remove_favorite(effect_id)
                .await
                .expect("acknowledged removal should become a no-op")
        );
        let durable = JsonLibraryStore::open(path).expect("durable snapshot should parse");
        assert!(durable.list_favorites().await.is_empty());
    }

    #[cfg(all(unix, feature = "persistence-test-hooks"))]
    #[tokio::test]
    async fn conditional_favorite_upsert_retries_visible_uncertain_change_before_cas() {
        let tempdir = TempDir::new().expect("tempdir");
        let path = tempdir.path().join("library.json");
        let store = JsonLibraryStore::open(path).expect("open json store");
        let effect_id = EffectId::new(Uuid::now_v7());
        store.writer.set_injected_directory_sync_failures(1);

        let error = store
            .mutate_favorite_if_current(
                effect_id,
                None,
                0,
                ConditionalFavoriteMutation::Upsert { added_at_ms: 10 },
            )
            .await
            .expect_err("directory sync failure must remain observable");
        assert!(matches!(error, super::LibraryStoreError::Persistence(_)));
        assert_eq!(store.list_favorites().await.len(), 1);

        assert_eq!(
            store
                .mutate_favorite_if_current(
                    effect_id,
                    None,
                    0,
                    ConditionalFavoriteMutation::Upsert { added_at_ms: 10 },
                )
                .await
                .expect("uncertain upsert should converge before CAS"),
            ConditionalFavoriteMutationOutcome::Applied { revision: 1 }
        );
        assert_eq!(
            store
                .mutate_favorite_if_current(
                    effect_id,
                    Some(10),
                    1,
                    ConditionalFavoriteMutation::Upsert { added_at_ms: 10 },
                )
                .await
                .expect("durable upsert should be current"),
            ConditionalFavoriteMutationOutcome::AlreadyCurrent { revision: 1 }
        );
    }

    #[cfg(all(unix, feature = "persistence-test-hooks"))]
    #[tokio::test]
    async fn conditional_favorite_remove_retries_visible_uncertain_change_before_cas() {
        let tempdir = TempDir::new().expect("tempdir");
        let path = tempdir.path().join("library.json");
        let store = JsonLibraryStore::open(path).expect("open json store");
        let effect_id = EffectId::new(Uuid::now_v7());
        store
            .upsert_favorite(effect_id, 10)
            .await
            .expect("seed favorite");
        store.writer.set_injected_directory_sync_failures(1);

        let error = store
            .mutate_favorite_if_current(effect_id, Some(10), 1, ConditionalFavoriteMutation::Remove)
            .await
            .expect_err("directory sync failure must remain observable");
        assert!(matches!(error, super::LibraryStoreError::Persistence(_)));
        assert!(store.list_favorites().await.is_empty());

        assert_eq!(
            store
                .mutate_favorite_if_current(
                    effect_id,
                    Some(10),
                    1,
                    ConditionalFavoriteMutation::Remove,
                )
                .await
                .expect("uncertain remove should converge before CAS"),
            ConditionalFavoriteMutationOutcome::Applied { revision: 2 }
        );
        assert_eq!(
            store
                .mutate_favorite_if_current(
                    effect_id,
                    None,
                    2,
                    ConditionalFavoriteMutation::Remove,
                )
                .await
                .expect("durable remove should be current"),
            ConditionalFavoriteMutationOutcome::AlreadyCurrent { revision: 2 }
        );
    }

    #[cfg(feature = "persistence-test-hooks")]
    #[tokio::test]
    async fn favorite_remove_replace_failure_keeps_memory_and_disk_unchanged() {
        let tempdir = TempDir::new().expect("tempdir");
        let path = tempdir.path().join("library.json");
        let store = JsonLibraryStore::open(path.clone()).expect("open json store");
        let effect_id = EffectId::new(Uuid::now_v7());
        store
            .upsert_favorite(effect_id, 10)
            .await
            .expect("seed favorite");
        store.writer.set_injected_replace_failures(1);

        let error = store
            .remove_favorite(effect_id)
            .await
            .expect_err("replace failure must reject removal");

        assert!(matches!(error, super::LibraryStoreError::Persistence(_)));
        assert_eq!(store.list_favorites().await.len(), 1);
        store
            .writer
            .flush(std::time::Duration::from_secs(5))
            .expect("retained snapshot should converge");
        let reopened = JsonLibraryStore::open(path).expect("reopen json store");
        assert_eq!(reopened.list_favorites().await.len(), 1);
    }

    #[test]
    fn json_store_open_fails_for_invalid_json() {
        let tempdir = TempDir::new().expect("tempdir");
        let path = tempdir.path().join("library.json");
        std::fs::write(&path, "{ not json").expect("write invalid json");

        let error = JsonLibraryStore::open(path).expect_err("expected parse error");
        assert!(matches!(error, JsonLibraryStoreOpenError::Parse { .. }));
    }

    #[test]
    fn json_store_open_accepts_a_truly_missing_snapshot() {
        let tempdir = TempDir::new().expect("tempdir");
        let path = tempdir.path().join("library.json");

        JsonLibraryStore::open(path).expect("missing snapshot should start empty");
    }

    #[cfg(unix)]
    #[test]
    fn json_store_open_rejects_snapshot_metadata_errors() {
        use std::os::unix::fs::symlink;

        let tempdir = TempDir::new().expect("tempdir");
        let path = tempdir.path().join("library.json");
        symlink("library.json", &path).expect("self-referential symlink");

        let error = JsonLibraryStore::open(path).expect_err("metadata error must fail closed");
        assert!(matches!(error, JsonLibraryStoreOpenError::Read { .. }));
    }
}
