//! Persisted named scene store.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Context;
use hypercolor_core::scene::SceneManager;
use hypercolor_types::scene::{RenderGroupRole, Scene, SceneId, SceneKind, SceneScope};

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
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }

        let payload =
            serde_json::to_string_pretty(&self.scenes).context("failed to serialize scenes")?;
        let tmp_path = self.path.with_extension("tmp");
        fs::write(&tmp_path, payload)
            .with_context(|| format!("failed to write {}", tmp_path.display()))?;
        fs::rename(&tmp_path, &self.path).with_context(|| {
            format!(
                "failed to move {} into {}",
                tmp_path.display(),
                self.path.display()
            )
        })?;
        Ok(())
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.scenes.len()
    }

    pub fn list(&self) -> impl Iterator<Item = &Scene> {
        self.scenes.values()
    }

    pub fn replace_named_scenes<I>(&mut self, scenes: I)
    where
        I: IntoIterator<Item = Scene>,
    {
        self.scenes = scenes
            .into_iter()
            .filter(|scene| scene.kind == SceneKind::Named && !scene.id.is_default())
            .map(|scene| (scene.id, scene))
            .collect();
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
            migrate_legacy_group_roles(scene);

            !id.is_default()
                && scene.id == *id
                && scene.kind == SceneKind::Named
                && !scene.name.is_empty()
        });
    }
}

fn migrate_legacy_group_roles(scene: &mut Scene) {
    for group in &mut scene.groups {
        if group.display_target.is_some() {
            group.role = RenderGroupRole::Display;
        }
    }

    let has_primary_group = scene
        .groups
        .iter()
        .any(|group| group.role == RenderGroupRole::Primary);
    if has_primary_group || !matches!(scene.scope, SceneScope::Full) {
        return;
    }

    let mut primary_candidate = None;
    for (index, group) in scene.groups.iter().enumerate() {
        if group.display_target.is_some() || group.role == RenderGroupRole::Display {
            continue;
        }
        if primary_candidate.replace(index).is_some() {
            return;
        }
    }

    if let Some(index) = primary_candidate {
        scene.groups[index].role = RenderGroupRole::Primary;
    }
}
