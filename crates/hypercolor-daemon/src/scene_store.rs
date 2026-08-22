//! Persisted named scene store.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use hypercolor_core::scene::SceneManager;
use hypercolor_types::scene::{Scene, SceneId, SceneKind};
use serde::{Deserialize, Serialize};

use crate::persistence::{
    AdmittedAtomicWrite, AtomicFileWriter, AtomicWriteCommitResult, AtomicWriteOutcome,
    PersistenceError, serialize_json_pretty,
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
pub struct SceneStoreSave {
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
    pub fn reserve_save<I>(&self, scenes: I) -> Result<SceneStoreSave, PersistenceError>
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
            write: self.writer.reserve().admit(payload),
        })
    }

    /// Commit a previously reserved snapshot and retain it when it wins.
    pub fn save_reserved(&mut self, pending: SceneStoreSave) -> anyhow::Result<AtomicWriteOutcome> {
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
    pub fn save_reserved_stage_aware(
        &mut self,
        pending: SceneStoreSave,
    ) -> AtomicWriteCommitResult {
        let SceneStoreSave { scenes, write } = pending;
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
    pending: SceneStoreSave,
) -> anyhow::Result<(AtomicWriteOutcome, HashMap<SceneId, Scene>)> {
    let outcome = pending.write.commit().context("failed to persist scenes")?;
    Ok((outcome, pending.scenes))
}
