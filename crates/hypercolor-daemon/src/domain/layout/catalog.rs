use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use hypercolor_types::spatial::SpatialLayout;
use tokio::sync::{OwnedRwLockWriteGuard, RwLock};

use crate::domain::device_binding::{DeviceBindingRemaps, MigrationPersistence};
use crate::persistence::{AdmittedAtomicWrite, AtomicFileWriter, AtomicWriteReservation};

#[derive(Clone)]
pub(super) struct LayoutCatalog {
    entries: Arc<RwLock<HashMap<String, SpatialLayout>>>,
    path: PathBuf,
}

pub(super) struct LayoutCatalogBindingMigration {
    source: HashMap<String, SpatialLayout>,
    candidate: HashMap<String, SpatialLayout>,
    write: AtomicWriteReservation,
    payload: Vec<u8>,
    migrated: usize,
}

pub(super) struct AdmittedLayoutCatalogBindingMigration {
    source: HashMap<String, SpatialLayout>,
    candidate: HashMap<String, SpatialLayout>,
    write: AdmittedAtomicWrite,
    migrated: usize,
}

pub(super) struct PersistedLayoutCatalogBindingMigration {
    source: HashMap<String, SpatialLayout>,
    candidate: HashMap<String, SpatialLayout>,
    migrated: usize,
}

pub(super) struct LayoutCatalogBindingPublication {
    entries: OwnedRwLockWriteGuard<HashMap<String, SpatialLayout>>,
    candidate: Option<HashMap<String, SpatialLayout>>,
    migrated: usize,
}

impl LayoutCatalog {
    pub(super) fn new(entries: HashMap<String, SpatialLayout>, path: PathBuf) -> Self {
        Self {
            entries: Arc::new(RwLock::new(entries)),
            path,
        }
    }

    pub(super) fn entries(&self) -> &RwLock<HashMap<String, SpatialLayout>> {
        &self.entries
    }

    pub(super) fn path(&self) -> &Path {
        &self.path
    }

    pub(super) async fn persist(&self) -> anyhow::Result<()> {
        let snapshot = self.entries.read().await.clone();
        self.save_snapshot(snapshot).await
    }

    pub(super) async fn save_snapshot(
        &self,
        snapshot: HashMap<String, SpatialLayout>,
    ) -> anyhow::Result<()> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || crate::layout_store::save(&path, &snapshot))
            .await
            .map_err(|error| anyhow::anyhow!("layout store task failed: {error}"))?
    }

    pub(super) async fn persist_best_effort(&self) {
        if let Err(error) = self.persist().await {
            tracing::warn!(
                path = %self.path.display(),
                %error,
                "Failed to persist layout store"
            );
        }
    }

    pub(super) async fn prepare_binding_migration(
        &self,
        remaps: &DeviceBindingRemaps,
    ) -> anyhow::Result<Option<LayoutCatalogBindingMigration>> {
        let source = self.entries.read().await.clone();
        let mut candidate = source.clone();
        let migrated = candidate
            .values_mut()
            .map(|layout| remaps.remap_layout(layout))
            .sum();
        if migrated == 0 {
            return Ok(None);
        }
        let payload = crate::layout_store::serialize(&candidate)?;
        let writer = AtomicFileWriter::new(&self.path)?;
        Ok(Some(LayoutCatalogBindingMigration {
            source,
            candidate,
            write: writer.reserve(),
            payload,
            migrated,
        }))
    }

    pub(super) async fn prepare_binding_publication(
        &self,
        migration: PersistedLayoutCatalogBindingMigration,
    ) -> anyhow::Result<LayoutCatalogBindingPublication> {
        let entries = Arc::clone(&self.entries).write_owned().await;
        anyhow::ensure!(
            *entries == migration.source,
            "device binding migration was superseded by newer layouts"
        );
        Ok(LayoutCatalogBindingPublication {
            entries,
            candidate: Some(migration.candidate),
            migrated: migration.migrated,
        })
    }
}

impl LayoutCatalogBindingMigration {
    pub(super) fn admit(self) -> AdmittedLayoutCatalogBindingMigration {
        AdmittedLayoutCatalogBindingMigration {
            source: self.source,
            candidate: self.candidate,
            write: self.write.admit(self.payload),
            migrated: self.migrated,
        }
    }
}

impl AdmittedLayoutCatalogBindingMigration {
    pub(super) fn persist(self) -> (PersistedLayoutCatalogBindingMigration, MigrationPersistence) {
        let persistence = MigrationPersistence::from_commit(self.write.commit_stage_aware());
        (
            PersistedLayoutCatalogBindingMigration {
                source: self.source,
                candidate: self.candidate,
                migrated: self.migrated,
            },
            persistence,
        )
    }
}

impl LayoutCatalogBindingPublication {
    pub(super) fn publish(&mut self) -> usize {
        *self.entries = self
            .candidate
            .take()
            .expect("layout binding migration must publish exactly once");
        self.migrated
    }
}
