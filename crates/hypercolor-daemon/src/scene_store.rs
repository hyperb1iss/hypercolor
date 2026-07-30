//! Persisted named scene store.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Context;
use hypercolor_core::scene::SceneManager;
use hypercolor_types::scene::{Scene, SceneId, SceneKind};

use crate::persistence::{
    AtomicFileWriter, AtomicWriteOutcome, AtomicWriteReservation, PersistenceError,
};

/// Named-scene snapshot reserved at its owning scene-manager boundary.
#[derive(Debug)]
pub struct SceneStoreSave {
    scenes: HashMap<SceneId, Scene>,
    write: AtomicWriteReservation,
}

/// JSON-backed named-scene store.
#[derive(Debug, Clone, Default)]
pub struct SceneStore {
    path: PathBuf,
    scenes: HashMap<SceneId, Scene>,
}

impl SceneStore {
    /// Create an empty store rooted at `path`.
    #[must_use]
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            scenes: HashMap::new(),
        }
    }

    /// Load an existing store or create an empty one when absent.
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        if !path.exists() {
            return Ok(Self::new(path.to_path_buf()));
        }

        let raw = fs::read_to_string(path)
            .with_context(|| format!("failed to read scenes at {}", path.display()))?;
        let scenes = serde_json::from_str::<HashMap<SceneId, Scene>>(&raw)
            .with_context(|| format!("failed to parse scenes at {}", path.display()))?;

        let mut store = Self {
            path: path.to_path_buf(),
            scenes,
        };
        store.normalize();
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
        let writer = AtomicFileWriter::new(&self.path)?;
        Ok(SceneStoreSave {
            scenes: named_scenes(scenes),
            write: writer.reserve(),
        })
    }

    /// Commit a previously reserved snapshot and retain it when it wins.
    pub fn save_reserved(&mut self, pending: SceneStoreSave) -> anyhow::Result<AtomicWriteOutcome> {
        let SceneStoreSave { scenes, write } = pending;
        let payload =
            serde_json::to_string_pretty(&scenes).context("failed to serialize scenes")?;
        let previous = std::mem::replace(&mut self.scenes, scenes);
        match write
            .write(payload.as_bytes())
            .context("failed to persist scenes")
        {
            Ok(AtomicWriteOutcome::Superseded) => {
                self.scenes = previous;
                Ok(AtomicWriteOutcome::Superseded)
            }
            Ok(AtomicWriteOutcome::Written) => Ok(AtomicWriteOutcome::Written),
            Err(error) => Err(error),
        }
    }

    /// Wake a pending retry after a semantic no-op.
    pub fn kick_persistence(&self) -> Result<(), PersistenceError> {
        AtomicFileWriter::new(&self.path)?.kick();
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

    fn normalize(&mut self) {
        self.scenes.retain(|id, scene| {
            scene.name = scene.name.trim().to_owned();
            scene.description = scene
                .description
                .take()
                .map(|description| description.trim().to_owned())
                .filter(|description| !description.is_empty());

            !id.is_default()
                && scene.id == *id
                && scene.kind == SceneKind::Named
                && !scene.name.is_empty()
        });
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
    let payload =
        serde_json::to_string_pretty(&pending.scenes).context("failed to serialize scenes")?;
    let outcome = pending
        .write
        .write(payload.as_bytes())
        .context("failed to persist scenes")?;
    Ok((outcome, pending.scenes))
}
