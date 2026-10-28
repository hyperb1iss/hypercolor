//! Bulk authored-library replacement under the ordinary mutation fence.

use std::collections::HashMap;
use std::sync::Arc;

use hypercolor_types::library::{EffectPlaylist, EffectPreset, PlaylistId, PresetId};
use serde::{Deserialize, Serialize};
use tokio::sync::{OwnedRwLockWriteGuard, RwLock};

use super::{InMemoryLibraryData, LibrarySnapshot, LibraryStoreError, PreparedLibrarySnapshot};
use crate::persistence::{AtomicFileWriter, serialize_json_pretty};
use crate::persistence::{AtomicWriteWaitOutcome, PersistenceShutdown};

/// Setup-owned library records. Favorites retain their separate account scope.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct LibraryPortableSnapshot {
    pub presets: HashMap<PresetId, EffectPreset>,
    pub playlists: HashMap<PlaylistId, EffectPlaylist>,
}

impl LibraryPortableSnapshot {
    pub(super) fn from_data(data: &InMemoryLibraryData) -> Self {
        Self {
            presets: data.presets.clone(),
            playlists: data.playlists.clone(),
        }
    }

    fn validate_keys(&self) -> Result<(), LibraryStoreError> {
        if self.presets.iter().any(|(id, preset)| *id != preset.id)
            || self
                .playlists
                .iter()
                .any(|(id, playlist)| *id != playlist.id)
        {
            return Err(LibraryStoreError::Persistence(
                "portable library keys must match their record IDs".to_owned(),
            ));
        }
        Ok(())
    }
}

/// A replacement held exclusively through durable persistence and publication.
pub struct PreparedPortableLibrary {
    data: OwnedRwLockWriteGuard<InMemoryLibraryData>,
    _mutation_guard: OwnedRwLockWriteGuard<()>,
    candidate: Option<LibraryPortableSnapshot>,
    write: Option<PreparedLibrarySnapshot>,
    durable: bool,
    attempted: bool,
}

impl PreparedPortableLibrary {
    pub(super) async fn prepare(
        data: &Arc<RwLock<InMemoryLibraryData>>,
        mutation_gate: &Arc<RwLock<()>>,
        writer: Option<&AtomicFileWriter>,
        expected: &LibraryPortableSnapshot,
        candidate: LibraryPortableSnapshot,
    ) -> Result<Self, LibraryStoreError> {
        candidate.validate_keys()?;
        // The gate waits for prior admitted writes, including persistence after
        // a mutation has released the data lock. Both guards stay held.
        let mutation_guard = Arc::clone(mutation_gate).write_owned().await;
        let data = Arc::clone(data).write_owned().await;
        if LibraryPortableSnapshot::from_data(&data) != *expected {
            return Err(LibraryStoreError::Persistence(
                "portable library replacement was superseded by a local edit".to_owned(),
            ));
        }
        let write = if let Some(writer) = writer {
            let mut merged = data.clone();
            merged.presets.clone_from(&candidate.presets);
            merged.playlists.clone_from(&candidate.playlists);
            let payload = serialize_json_pretty(&LibrarySnapshot::from_data(&merged))
                .map_err(|error| LibraryStoreError::Persistence(error.to_string()))?;
            Some(PreparedLibrarySnapshot {
                write: writer.reserve(),
                payload,
            })
        } else {
            None
        };
        Ok(Self {
            data,
            _mutation_guard: mutation_guard,
            candidate: Some(candidate),
            write,
            durable: false,
            attempted: false,
        })
    }

    pub(crate) fn candidate(&self) -> &LibraryPortableSnapshot {
        self.candidate
            .as_ref()
            .expect("replacement has not published")
    }

    pub(crate) fn persist(&mut self, shutdown: &PersistenceShutdown) -> AtomicWriteWaitOutcome {
        if shutdown.is_shutdown() {
            return AtomicWriteWaitOutcome::ShutdownPending;
        }
        assert!(!self.attempted, "replacement persistence is attempted once");
        self.attempted = true;
        let outcome = self
            .write
            .take()
            .map_or(AtomicWriteWaitOutcome::Written, |write| {
                write
                    .write
                    .admit(write.payload)
                    .commit_and_wait_durable_or_shutdown(shutdown)
            });
        self.durable = matches!(outcome, AtomicWriteWaitOutcome::Written);
        outcome
    }

    pub(crate) fn publish(&mut self) {
        assert!(
            self.durable,
            "replacement must be durable before publication"
        );
        let candidate = self.candidate.take().expect("replacement publishes once");
        self.data.presets = candidate.presets;
        self.data.playlists = candidate.playlists;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::{InMemoryLibraryStore, JsonLibraryStore, LibraryStore};
    use hypercolor_types::effect::EffectId;

    fn candidate() -> LibraryPortableSnapshot {
        let preset = EffectPreset {
            id: PresetId::new(),
            name: "Saved light".to_owned(),
            description: None,
            effect_id: EffectId::new(uuid::Uuid::now_v7()),
            controls: HashMap::new(),
            tags: Vec::new(),
            created_at_ms: 1,
            updated_at_ms: 1,
        };
        let playlist = EffectPlaylist {
            id: PlaylistId::new(),
            name: "Evening".to_owned(),
            description: None,
            items: Vec::new(),
            loop_enabled: true,
            created_at_ms: 1,
            updated_at_ms: 1,
        };
        LibraryPortableSnapshot {
            presets: HashMap::from([(preset.id, preset)]),
            playlists: HashMap::from([(playlist.id, playlist)]),
        }
    }

    #[tokio::test]
    async fn portable_library_preserves_favorites_and_persists_before_publish() {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = directory.path().join("library.json");
        let store = JsonLibraryStore::open(path.clone()).expect("open");
        let expected = store.portable_snapshot().await;
        let effect = EffectId::new(uuid::Uuid::now_v7());
        store.upsert_favorite(effect, 123).await.expect("favorite");
        let favorite_before = store.project_favorites().await;
        let candidate = candidate();
        let mut prepared = store
            .prepare_portable_replace(&expected, candidate.clone())
            .await
            .expect("prepare");
        assert_eq!(prepared.candidate(), &candidate);
        assert!(store.data.try_read().is_err());
        assert!(store.mutation_gate.try_read().is_err());
        assert!(matches!(
            prepared.persist(&PersistenceShutdown::default()),
            AtomicWriteWaitOutcome::Written
        ));
        let reopened = JsonLibraryStore::open(path).expect("reopen durable file");
        assert_eq!(reopened.portable_snapshot().await, candidate);
        assert_eq!(reopened.project_favorites().await, favorite_before);
        // Durable bytes do not publish the live maps prematurely.
        assert_eq!(LibraryPortableSnapshot::from_data(&prepared.data), expected);
        prepared.publish();
        drop(prepared);
        assert_eq!(store.portable_snapshot().await, candidate);
        assert_eq!(store.project_favorites().await, favorite_before);
    }

    #[tokio::test]
    async fn portable_library_rejects_stale_authored_snapshot_and_invalid_keys() {
        let store = InMemoryLibraryStore::new();
        let old = store.portable_snapshot().await;
        let candidate = candidate();
        let preset = candidate.presets.values().next().expect("preset").clone();
        store.insert_preset(preset).await.expect("local edit");
        assert!(
            store
                .prepare_portable_replace(&old, candidate.clone())
                .await
                .is_err()
        );
        let current = store.portable_snapshot().await;
        let mut invalid = candidate;
        let preset = invalid.presets.values().next().expect("preset").clone();
        invalid.presets.clear();
        invalid.presets.insert(PresetId::new(), preset);
        assert!(
            store
                .prepare_portable_replace(&current, invalid)
                .await
                .is_err()
        );
        assert_eq!(store.portable_snapshot().await, current);
    }

    #[tokio::test]
    async fn portable_library_bulk_replacement_removes_missing_authored_records() {
        let store = InMemoryLibraryStore::new();
        let expected = store.portable_snapshot().await;
        let mut prepared = store
            .prepare_portable_replace(&expected, candidate())
            .await
            .expect("prepare");
        assert!(matches!(
            prepared.persist(&PersistenceShutdown::default()),
            AtomicWriteWaitOutcome::Written
        ));
        prepared.publish();
        drop(prepared);
        let expected = store.portable_snapshot().await;
        assert_eq!(expected.presets.len(), 1);
        assert_eq!(expected.playlists.len(), 1);
        let mut prepared = store
            .prepare_portable_replace(&expected, LibraryPortableSnapshot::default())
            .await
            .expect("prepare delete");
        assert!(matches!(
            prepared.persist(&PersistenceShutdown::default()),
            AtomicWriteWaitOutcome::Written
        ));
        prepared.publish();
        drop(prepared);
        assert_eq!(
            store.portable_snapshot().await,
            LibraryPortableSnapshot::default()
        );
    }

    #[tokio::test]
    async fn portable_library_drop_before_persist_releases_guard_without_changes() {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = directory.path().join("library.json");
        let store = JsonLibraryStore::open(path.clone()).expect("open");
        let before = store.portable_snapshot().await;
        let prepared = store
            .prepare_portable_replace(&before, candidate())
            .await
            .expect("prepare");
        assert!(store.mutation_gate.try_read().is_err());
        drop(prepared);
        store
            .writer
            .flush(std::time::Duration::from_secs(1))
            .expect("no admitted intent");
        assert!(!path.exists());
        assert_eq!(store.portable_snapshot().await, before);
        assert!(store.mutation_gate.try_read().is_ok());
    }

    #[cfg(feature = "persistence-test-hooks")]
    #[tokio::test]
    async fn portable_library_waits_for_durability_before_publication() {
        let directory = tempfile::tempdir().expect("tempdir");
        let store = JsonLibraryStore::open(directory.path().join("library.json")).expect("open");
        let before = store.portable_snapshot().await;
        let mut prepared = store
            .prepare_portable_replace(&before, candidate())
            .await
            .expect("prepare");
        store.writer.set_injected_directory_sync_failures(1);
        assert!(matches!(
            prepared.persist(&PersistenceShutdown::default()),
            AtomicWriteWaitOutcome::Written
        ));
        assert!(prepared.durable);
        assert_eq!(LibraryPortableSnapshot::from_data(&prepared.data), before);
        assert!(store.data.try_read().is_err());
        assert!(store.writer.holds_durable_bytes());
        prepared.publish();
        drop(prepared);
        assert_ne!(store.portable_snapshot().await, before);
    }
}
