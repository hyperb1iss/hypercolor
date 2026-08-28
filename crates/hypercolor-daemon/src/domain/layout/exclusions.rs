use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context;
use hypercolor_types::scene::{SceneId, Zone, ZoneId};
use hypercolor_types::spatial::{Output, SpatialLayout};
use tokio::sync::{OwnedRwLockWriteGuard, RwLock};

use crate::domain::device_binding::{DeviceBindingRemaps, MigrationPersistence};
use crate::domain::scene::SceneService;
use crate::layout_auto_exclusions::{self, LayoutAutoExclusionKey, LayoutAutoExclusionStore};
use crate::persistence::{
    AdmittedAtomicWrite, AtomicFileWriter, AtomicWriteOutcome, AtomicWriteReservation,
};

#[derive(Clone)]
pub(super) struct LayoutExclusions {
    entries: Arc<RwLock<LayoutAutoExclusionStore>>,
    persistence: ExclusionPersistence,
    scenes: SceneService,
}

pub(super) struct LayoutExclusionsBindingMigration {
    source: LayoutAutoExclusionStore,
    candidate: LayoutAutoExclusionStore,
    write: AtomicWriteReservation,
    payload: Vec<u8>,
    migrated: usize,
}

pub(super) struct AdmittedLayoutExclusionsBindingMigration {
    source: LayoutAutoExclusionStore,
    candidate: LayoutAutoExclusionStore,
    write: AdmittedAtomicWrite,
    migrated: usize,
}

pub(super) struct PersistedLayoutExclusionsBindingMigration {
    source: LayoutAutoExclusionStore,
    candidate: LayoutAutoExclusionStore,
    migrated: usize,
}

pub(super) struct LayoutExclusionsBindingPublication {
    entries: OwnedRwLockWriteGuard<LayoutAutoExclusionStore>,
    candidate: Option<LayoutAutoExclusionStore>,
    migrated: usize,
}

impl LayoutExclusions {
    pub(super) fn new(
        entries: LayoutAutoExclusionStore,
        path: PathBuf,
        scenes: SceneService,
    ) -> Self {
        Self {
            entries: Arc::new(RwLock::new(entries)),
            persistence: ExclusionPersistence { path },
            scenes,
        }
    }

    #[cfg(feature = "persistence-test-hooks")]
    pub(super) fn entries(&self) -> &RwLock<LayoutAutoExclusionStore> {
        &self.entries
    }

    pub(super) async fn excluded_device_ids(&self, layout: &SpatialLayout) -> HashSet<String> {
        let keys = self.active_keys(layout).await;
        let entries = self.entries.read().await;
        keys.iter()
            .filter_map(|key| entries.get(key))
            .flat_map(|device_ids| device_ids.iter().cloned())
            .collect()
    }

    pub(super) async fn reconcile_layout(
        &self,
        layout_id: &str,
        previous_zones: &[Output],
        updated_zones: &[Output],
    ) {
        let changed = {
            let mut entries = self.entries.write().await;
            reconcile_entry(
                &mut entries,
                LayoutAutoExclusionKey::layout(layout_id),
                previous_zones,
                updated_zones,
            )
        };
        if changed {
            self.persist().await;
        }
    }

    pub(super) async fn remove_layout(&self, layout_id: &str) {
        let removed = self
            .entries
            .write()
            .await
            .remove(&LayoutAutoExclusionKey::layout(layout_id))
            .is_some();
        if removed {
            self.persist().await;
        }
    }

    pub(super) async fn reconcile_zones(
        &self,
        scene_id: SceneId,
        previous_zones: &[Zone],
        updated_zones: &[Zone],
    ) {
        let changed = {
            let mut entries = self.entries.write().await;
            let mut changed = false;
            for previous_zone in previous_zones {
                let Some(updated_zone) = updated_zones
                    .iter()
                    .find(|zone| zone.id == previous_zone.id)
                else {
                    continue;
                };
                if previous_zone.layout.zones == updated_zone.layout.zones {
                    continue;
                }
                changed |= reconcile_entry(
                    &mut entries,
                    LayoutAutoExclusionKey::zone(scene_id, previous_zone.id),
                    &previous_zone.layout.zones,
                    &updated_zone.layout.zones,
                );
            }
            changed
        };
        if changed {
            self.persist().await;
        }
    }

    pub(super) async fn remove_zone(&self, scene_id: SceneId, zone_id: ZoneId) {
        let removed = self
            .entries
            .write()
            .await
            .remove(&LayoutAutoExclusionKey::zone(scene_id, zone_id))
            .is_some();
        if removed {
            self.persist().await;
        }
    }

    async fn active_keys(&self, layout: &SpatialLayout) -> Vec<LayoutAutoExclusionKey> {
        let mut keys = vec![LayoutAutoExclusionKey::layout(layout.id.as_str())];
        let manager = self.scenes.snapshot().await;
        if let Some(scene) = manager.active_scene()
            && let Some(zone) = scene.primary_zone()
        {
            keys.push(LayoutAutoExclusionKey::zone(scene.id, zone.id));
        }
        keys
    }

    async fn persist(&self) {
        let pending = {
            let entries = self.entries.read().await;
            self.persistence.reserve_snapshot(&entries)
        };
        let result = match pending {
            Ok(pending) => tokio::task::spawn_blocking(move || pending.commit())
                .await
                .map_err(|error| anyhow::anyhow!("layout exclusion store task failed: {error}"))
                .and_then(|result| result),
            Err(error) => Err(error),
        };
        if let Err(error) = result {
            tracing::warn!(
                path = %self.persistence.path.display(),
                %error,
                "Failed to persist layout auto-exclusion store"
            );
        }
    }

    pub(super) async fn prepare_binding_migration(
        &self,
        remaps: &DeviceBindingRemaps,
    ) -> anyhow::Result<Option<LayoutExclusionsBindingMigration>> {
        let source = self.entries.read().await.clone();
        let mut candidate = source.clone();
        let migrated = candidate
            .values_mut()
            .map(|device_ids| remaps.remap_layout_device_id_set(device_ids))
            .sum();
        if migrated == 0 {
            return Ok(None);
        }
        let payload = layout_auto_exclusions::serialize(&candidate)?;
        let writer = AtomicFileWriter::new(&self.persistence.path)?;
        Ok(Some(LayoutExclusionsBindingMigration {
            source,
            candidate,
            write: writer.reserve(),
            payload,
            migrated,
        }))
    }

    pub(super) async fn prepare_binding_publication(
        &self,
        migration: PersistedLayoutExclusionsBindingMigration,
    ) -> anyhow::Result<LayoutExclusionsBindingPublication> {
        let entries = Arc::clone(&self.entries).write_owned().await;
        anyhow::ensure!(
            *entries == migration.source,
            "device binding migration was superseded by newer layout exclusions"
        );
        Ok(LayoutExclusionsBindingPublication {
            entries,
            candidate: Some(migration.candidate),
            migrated: migration.migrated,
        })
    }
}

impl LayoutExclusionsBindingMigration {
    pub(super) fn admit(self) -> AdmittedLayoutExclusionsBindingMigration {
        AdmittedLayoutExclusionsBindingMigration {
            source: self.source,
            candidate: self.candidate,
            write: self.write.admit(self.payload),
            migrated: self.migrated,
        }
    }
}

impl AdmittedLayoutExclusionsBindingMigration {
    pub(super) fn persist(
        self,
    ) -> (
        PersistedLayoutExclusionsBindingMigration,
        MigrationPersistence,
    ) {
        let persistence = MigrationPersistence::from_commit(self.write.commit_stage_aware());
        (
            PersistedLayoutExclusionsBindingMigration {
                source: self.source,
                candidate: self.candidate,
                migrated: self.migrated,
            },
            persistence,
        )
    }
}

impl LayoutExclusionsBindingPublication {
    pub(super) fn publish(&mut self) -> usize {
        *self.entries = self
            .candidate
            .take()
            .expect("exclusion binding migration must publish exactly once");
        self.migrated
    }
}

fn reconcile_entry(
    entries: &mut LayoutAutoExclusionStore,
    key: LayoutAutoExclusionKey,
    previous_zones: &[Output],
    updated_zones: &[Output],
) -> bool {
    let current = entries.get(&key).cloned().unwrap_or_default();
    let next = layout_auto_exclusions::reconcile_layout_device_exclusions(
        previous_zones,
        updated_zones,
        &current,
    );
    if next == current {
        return false;
    }
    if next.is_empty() {
        entries.remove(&key);
    } else {
        entries.insert(key, next);
    }
    true
}

#[derive(Clone)]
struct ExclusionPersistence {
    path: PathBuf,
}

impl ExclusionPersistence {
    fn reserve_snapshot(
        &self,
        entries: &LayoutAutoExclusionStore,
    ) -> anyhow::Result<PendingExclusionSave> {
        let writer = AtomicFileWriter::new(&self.path)
            .context("failed to reserve layout auto-exclusion destination")?;
        let payload = layout_auto_exclusions::serialize(entries)?;
        Ok(PendingExclusionSave {
            path: self.path.clone(),
            write: writer.reserve().admit(payload),
        })
    }
}

struct PendingExclusionSave {
    path: PathBuf,
    write: AdmittedAtomicWrite,
}

impl PendingExclusionSave {
    fn commit(self) -> anyhow::Result<AtomicWriteOutcome> {
        self.write.commit().with_context(|| {
            format!(
                "failed to persist layout auto-exclusions at {}",
                self.path.display()
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, HashSet};

    use super::{AtomicWriteOutcome, ExclusionPersistence, LayoutAutoExclusionKey};
    use crate::layout_auto_exclusions;

    #[test]
    fn older_worker_cannot_overwrite_newer_reserved_snapshot() {
        let temp = tempfile::tempdir().expect("tempdir should build");
        let path = temp.path().join("layout-auto-exclusions.json");
        let persistence = ExclusionPersistence { path: path.clone() };
        let key = LayoutAutoExclusionKey::layout("default");
        let older = HashMap::from([(key.clone(), HashSet::from(["usb:older".to_owned()]))]);
        let newer = HashMap::from([(key.clone(), HashSet::from(["usb:newer".to_owned()]))]);

        let older_write = persistence
            .reserve_snapshot(&older)
            .expect("older snapshot should reserve");
        let newer_write = persistence
            .reserve_snapshot(&newer)
            .expect("newer snapshot should reserve");

        assert_eq!(
            newer_write.commit().expect("newer snapshot should commit"),
            AtomicWriteOutcome::Written
        );
        assert_eq!(
            older_write
                .commit()
                .expect("older snapshot should be rejected"),
            AtomicWriteOutcome::Superseded
        );
        assert_eq!(
            layout_auto_exclusions::load(&path).expect("snapshot should load"),
            newer
        );
    }
}
