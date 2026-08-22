//! Persisted named scene store.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use hypercolor_core::scene::SceneManager;
use hypercolor_types::scene::{Scene, SceneId, SceneKind};
use serde::{Deserialize, Serialize};

use crate::domain::effect::{EffectIdMigrations, remap_zones};
use crate::persistence::{
    AdmittedAtomicWrite, AtomicFileWriter, AtomicWriteCommitResult, AtomicWriteOutcome,
    AtomicWriteReservation, PersistenceError, serialize_json_pretty,
};

const SCENE_STORE_SCHEMA_VERSION: u32 = 2;
const SCENE_STORE_V2_SHAPE: &str = r#"{"schema_version":2,"scenes":{...}}"#;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SceneStoreDocument {
    schema_version: u32,
    scenes: HashMap<SceneId, Scene>,
}

impl SceneStoreDocument {
    fn current(scenes: HashMap<SceneId, Scene>) -> Self {
        Self {
            schema_version: SCENE_STORE_SCHEMA_VERSION,
            scenes,
        }
    }
}

/// Named-scene snapshot reserved at its owning scene-manager boundary.
#[derive(Debug)]
pub(crate) struct SceneStoreSave {
    scenes: HashMap<SceneId, Scene>,
    write: AtomicWriteReservation,
    payload: Vec<u8>,
}

#[derive(Debug)]
pub(crate) struct AdmittedSceneStoreSave {
    scenes: HashMap<SceneId, Scene>,
    write: AdmittedAtomicWrite,
}

/// JSON-backed named-scene store.
#[derive(Debug, Clone)]
pub struct SceneStore {
    writer: AtomicFileWriter,
    scenes: HashMap<SceneId, Scene>,
}

impl SceneStore {
    /// Create an empty store rooted at `path`.
    pub fn new(path: PathBuf) -> Result<Self, PersistenceError> {
        let writer = AtomicFileWriter::new(&path)?;
        Ok(Self {
            writer,
            scenes: HashMap::new(),
        })
    }

    /// Load an existing store or create an empty one when absent.
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let writer = AtomicFileWriter::new(path)?;
        if !path.exists() {
            return Ok(Self {
                writer,
                scenes: HashMap::new(),
            });
        }

        let raw = fs::read_to_string(path)
            .with_context(|| format!("failed to read scenes at {}", path.display()))?;
        let original = serde_json::from_str::<serde_json::Value>(&raw)
            .with_context(|| format!("failed to parse scenes at {}", path.display()))?;
        let Some(schema_version) = original
            .as_object()
            .and_then(|document| document.get("schema_version"))
            .and_then(serde_json::Value::as_u64)
        else {
            bail!(
                "scene store uses the retired unversioned schema; this release accepts the v2 \
                 envelope {SCENE_STORE_V2_SHAPE}. Restore the file with a pre-v2 Hypercolor \
                 release, export or rewrite its scenes into the zones/layers schema, then \
                 restart. The original file was not modified"
            );
        };
        if schema_version != u64::from(SCENE_STORE_SCHEMA_VERSION) {
            bail!(
                "unsupported scene store schema version {schema_version}; this release accepts \
                 the v2 envelope {SCENE_STORE_V2_SHAPE}. Export or rewrite the store into the \
                 zones/layers schema before restarting. The original file was not modified"
            );
        }
        let document = serde_json::from_value::<SceneStoreDocument>(original.clone())
            .with_context(|| format!("failed to parse scenes at {}", path.display()))?;

        let mut store = Self {
            writer,
            scenes: document.scenes,
        };
        store
            .normalize()
            .with_context(|| format!("failed to validate scenes at {}", path.display()))?;
        let normalized = serde_json::to_value(SceneStoreDocument::current(store.scenes.clone()))
            .context("failed to serialize normalized scene store")?;
        if normalized != original {
            store
                .save()
                .context("failed to persist normalized scene store")?;
        }
        Ok(store)
    }

    /// Save the current snapshot to disk.
    pub fn save(&self) -> anyhow::Result<()> {
        let pending = self.reserve_save(self.scenes.values().cloned())?;
        persist_reserved(pending).map(|_| ())
    }

    /// Reserve a named-scene snapshot before releasing its source lock.
    pub(crate) fn reserve_save<I>(&self, scenes: I) -> Result<SceneStoreSave, PersistenceError>
    where
        I: IntoIterator<Item = Scene>,
    {
        let scenes = named_scenes(scenes);
        let payload = serialize_json_pretty(&SceneStoreDocument::current(scenes.clone())).map_err(
            |source| PersistenceError::SerializeSnapshot {
                subject: "named scenes",
                source,
            },
        )?;
        Ok(SceneStoreSave {
            scenes,
            write: self.writer.reserve(),
            payload,
        })
    }

    /// Commit a previously reserved snapshot and retain it when it wins.
    pub(crate) fn save_reserved(
        &mut self,
        pending: SceneStoreSave,
    ) -> anyhow::Result<AtomicWriteOutcome> {
        match self.save_reserved_stage_aware(pending) {
            AtomicWriteCommitResult::Superseded => Ok(AtomicWriteOutcome::Superseded),
            AtomicWriteCommitResult::DurableWritten => Ok(AtomicWriteOutcome::Written),
            AtomicWriteCommitResult::FailedBeforeReplacement(error)
            | AtomicWriteCommitResult::ReplacementVisibleButNotDurable(error) => {
                Err(error).context("failed to persist scenes")
            }
        }
    }

    /// Commit a reserved snapshot without collapsing its durability stage.
    pub(crate) fn save_reserved_stage_aware(
        &mut self,
        pending: SceneStoreSave,
    ) -> AtomicWriteCommitResult {
        let AdmittedSceneStoreSave { scenes, write } = pending.admit();
        let previous = std::mem::replace(&mut self.scenes, scenes);
        let outcome = write.commit_stage_aware();
        if matches!(outcome, AtomicWriteCommitResult::Superseded) {
            self.scenes = previous;
        }
        outcome
    }

    /// Wake a pending retry after a semantic no-op.
    pub fn kick_persistence(&self) -> Result<(), PersistenceError> {
        self.writer.kick();
        Ok(())
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.scenes.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.scenes.is_empty()
    }

    pub fn list(&self) -> impl Iterator<Item = &Scene> {
        self.scenes.values()
    }

    pub fn replace_named_scenes<I>(&mut self, scenes: I)
    where
        I: IntoIterator<Item = Scene>,
    {
        self.scenes = named_scenes(scenes);
    }

    /// Rewrite path-derived effect IDs during the startup import.
    pub fn migrate_effect_ids(&mut self, migrations: &EffectIdMigrations) -> anyhow::Result<usize> {
        let mut scenes = self.scenes.clone();
        let migrated = scenes
            .values_mut()
            .map(|scene| remap_zones(&mut scene.zones, migrations))
            .sum();
        if migrated == 0 {
            return Ok(0);
        }

        let pending = self.reserve_save(scenes.into_values())?;
        match self.save_reserved(pending)? {
            AtomicWriteOutcome::Written => Ok(migrated),
            AtomicWriteOutcome::Superseded => {
                bail!("effect ID migration was superseded by a newer scene snapshot")
            }
        }
    }

    pub fn sync_from_manager(&mut self, manager: &SceneManager) {
        self.replace_named_scenes(manager.list().into_iter().cloned());
    }

    fn normalize(&mut self) -> anyhow::Result<()> {
        for (id, scene) in &mut self.scenes {
            scene.name = scene.name.trim().to_owned();
            scene.description = scene
                .description
                .take()
                .map(|description| description.trim().to_owned())
                .filter(|description| !description.is_empty());

            if id.is_default() {
                bail!("persisted scene store contains the reserved default scene");
            }
            if scene.id != *id {
                bail!(
                    "persisted scene key {id} does not match scene id {}",
                    scene.id
                );
            }
            if scene.kind != SceneKind::Named {
                bail!("persisted scene {id} is not a named scene");
            }
            if let Err(errors) = scene.validate() {
                bail!("persisted scene {id} is invalid: {}", errors.join("; "));
            }
        }
        Ok(())
    }
}

impl SceneStoreSave {
    pub(crate) fn admit(self) -> AdmittedSceneStoreSave {
        AdmittedSceneStoreSave {
            scenes: self.scenes,
            write: self.write.admit(self.payload),
        }
    }
}

impl AdmittedSceneStoreSave {
    pub(crate) fn commit_stage_aware(self) -> (HashMap<SceneId, Scene>, AtomicWriteCommitResult) {
        (self.scenes, self.write.commit_stage_aware())
    }
}

fn named_scenes<I>(scenes: I) -> HashMap<SceneId, Scene>
where
    I: IntoIterator<Item = Scene>,
{
    scenes
        .into_iter()
        .filter(|scene| scene.kind == SceneKind::Named && !scene.id.is_default())
        .map(|scene| (scene.id, scene))
        .collect()
}

fn persist_reserved(
    pending: impl Into<AdmittedSceneStoreSave>,
) -> anyhow::Result<(AtomicWriteOutcome, HashMap<SceneId, Scene>)> {
    let pending = pending.into();
    let outcome = pending.write.commit().context("failed to persist scenes")?;
    Ok((outcome, pending.scenes))
}

impl From<SceneStoreSave> for AdmittedSceneStoreSave {
    fn from(pending: SceneStoreSave) -> Self {
        pending.admit()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    #[cfg(feature = "persistence-test-hooks")]
    use std::time::Duration;

    use hypercolor_core::scene::{SceneManager, make_scene};
    use hypercolor_types::effect::EffectId;
    use hypercolor_types::layer::{SceneLayer, SceneLayerId};
    use hypercolor_types::scene::{SceneId, SceneKind, SceneMutationMode};
    use tempfile::TempDir;

    #[cfg(feature = "persistence-test-hooks")]
    use crate::persistence::AtomicFileWriter;
    use crate::persistence::{AtomicWriteCommitResult, AtomicWriteOutcome};

    use super::SceneStore;

    #[test]
    fn rejects_an_overtaken_snapshot() {
        let tempdir = TempDir::new().expect("tempdir");
        let path = tempdir.path().join("scenes.json");
        let mut store = SceneStore::new(path.clone()).expect("scene store");
        let older_save = store
            .reserve_save([make_scene("Older")])
            .expect("reserve older scene snapshot");
        let newer_save = store
            .reserve_save([make_scene("Newer")])
            .expect("reserve newer scene snapshot");

        assert_eq!(
            store
                .save_reserved(newer_save)
                .expect("save newer scene snapshot"),
            AtomicWriteOutcome::Written
        );
        assert_eq!(
            store
                .save_reserved(older_save)
                .expect("reject older scene snapshot"),
            AtomicWriteOutcome::Superseded
        );

        let loaded = SceneStore::load(&path).expect("scene store should reload");
        let names = loaded
            .list()
            .map(|scene| scene.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(names, vec!["Newer"]);
    }

    #[test]
    fn stage_aware_save_restores_state_when_superseded() {
        let tempdir = TempDir::new().expect("tempdir");
        let path = tempdir.path().join("scenes.json");
        let mut store = SceneStore::new(path).expect("scene store");
        let older_save = store
            .reserve_save([make_scene("Older")])
            .expect("reserve older scene snapshot");
        let newer_save = store
            .reserve_save([make_scene("Newer")])
            .expect("reserve newer scene snapshot");

        assert!(matches!(
            store.save_reserved_stage_aware(newer_save),
            AtomicWriteCommitResult::DurableWritten
        ));
        assert!(matches!(
            store.save_reserved_stage_aware(older_save),
            AtomicWriteCommitResult::Superseded
        ));
        assert_eq!(
            store
                .list()
                .map(|scene| scene.name.as_str())
                .collect::<Vec<_>>(),
            vec!["Newer"]
        );
    }

    #[cfg(feature = "persistence-test-hooks")]
    #[test]
    fn failed_delete_keeps_live_state_and_does_not_resurrect() {
        let tempdir = TempDir::new().expect("tempdir");
        let path = tempdir.path().join("scenes.json");
        let mut store = SceneStore::new(path.clone()).expect("scene store");
        store.replace_named_scenes([make_scene("Ephemeral")]);
        store.save().expect("seed scene store");
        let writer = AtomicFileWriter::new(&path).expect("atomic writer");
        writer.set_injected_replace_failures(usize::MAX);
        let pending = store
            .reserve_save(std::iter::empty())
            .expect("reserve scene deletion");

        assert!(store.save_reserved(pending).is_err());
        assert!(store.is_empty(), "live scene state remains authoritative");

        writer.set_injected_replace_failures(0);
        store
            .kick_persistence()
            .expect("kick scene persistence retry");
        writer
            .flush(Duration::from_secs(5))
            .expect("scene deletion should converge");
        assert!(
            SceneStore::load(&path)
                .expect("reload scene store")
                .is_empty()
        );
    }

    #[test]
    fn scene_effect_id_migration_rewrites_the_durable_store() {
        let temp = TempDir::new().expect("tempdir");
        let path = temp.path().join("scenes.json");
        let legacy_id = EffectId::new(uuid::Uuid::now_v7());
        let canonical_id = EffectId::new(uuid::Uuid::now_v7());
        let mut scene = SceneManager::with_default()
            .get(&SceneId::DEFAULT)
            .cloned()
            .expect("default scene should exist");
        scene.id = SceneId::new();
        scene.name = "Migrated scene".to_owned();
        scene.kind = SceneKind::Named;
        scene.mutation_mode = SceneMutationMode::Live;
        scene.zones[0].layers = vec![SceneLayer::from_effect(
            SceneLayerId::new(),
            legacy_id,
            HashMap::new(),
            HashMap::new(),
            None,
        )];

        let mut store = SceneStore::new(path.clone()).expect("store should open");
        store.replace_named_scenes([scene]);
        store.save().expect("legacy scene should persist");
        assert_eq!(
            store
                .migrate_effect_ids(&HashMap::from([(legacy_id, canonical_id)]))
                .expect("migration should persist"),
            1
        );
        drop(store);

        let reopened = SceneStore::load(&path).expect("scene store should reopen");
        let effect_ids = reopened
            .list()
            .flat_map(|scene| &scene.zones)
            .flat_map(hypercolor_types::scene::Zone::effect_ids)
            .collect::<Vec<_>>();
        assert_eq!(effect_ids, vec![canonical_id]);
        assert!(
            !std::fs::read_to_string(path)
                .expect("scene file should read")
                .contains(&legacy_id.to_string())
        );
    }
}
