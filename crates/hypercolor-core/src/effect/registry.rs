//! Effect registry — discovery, indexing, and search.
//!
//! The [`EffectRegistry`] scans effect directories, parses metadata, and
//! provides lookup/search/filter operations over the known effect catalog.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use tracing::{debug, info, warn};

use hypercolor_types::effect::{EffectCategory, EffectId, EffectMetadata, EffectState};

// ── EffectEntry ──────────────────────────────────────────────────────────────

/// A single entry in the effect registry.
///
/// Contains the parsed metadata, filesystem location, and current state.
#[derive(Debug, Clone)]
pub struct EffectEntry {
    /// Parsed effect metadata.
    pub metadata: EffectMetadata,

    /// Absolute path to the primary source file on disk.
    pub source_path: PathBuf,

    /// Last-modified timestamp of the source file at discovery time.
    /// Used for cache invalidation on subsequent scans.
    pub modified: SystemTime,

    /// Current lifecycle state in the registry.
    pub state: EffectState,
}

// ── EffectRegistry ───────────────────────────────────────────────────────────

/// Central index of all discovered effects.
///
/// Provides synchronous lookup, category filtering, and text search over
/// the effect catalog. Discovery is driven externally — callers register
/// effects via [`register`](Self::register) after parsing metadata from
/// the filesystem.
///
/// For async filesystem scanning and hot-reload watching, a higher-level
/// coordinator uses this registry as the backing store.
pub struct EffectRegistry {
    /// All known effects, indexed by their unique id.
    effects: HashMap<EffectId, EffectEntry>,

    /// Root directories to scan for effects.
    search_paths: Vec<PathBuf>,
}

impl EffectRegistry {
    /// Create an empty registry with the given search paths.
    #[must_use]
    pub fn new(search_paths: Vec<PathBuf>) -> Self {
        info!(paths = ?search_paths, "Creating effect registry");
        Self {
            effects: HashMap::new(),
            search_paths,
        }
    }

    /// Returns the configured search paths.
    #[must_use]
    pub fn search_paths(&self) -> &[PathBuf] {
        &self.search_paths
    }

    /// Returns the total number of registered effects.
    #[must_use]
    pub fn len(&self) -> usize {
        self.effects.len()
    }

    /// Returns `true` if no effects are registered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.effects.is_empty()
    }

    /// Register an effect entry in the registry.
    ///
    /// If an effect with the same id already exists, it is replaced and
    /// the old entry is returned.
    pub fn register(&mut self, entry: EffectEntry) -> Option<EffectEntry> {
        let id = entry.metadata.id;
        debug!(id = %id, name = %entry.metadata.name, "Registering effect");
        self.effects.insert(id, entry)
    }

    /// Remove an effect from the registry by id.
    ///
    /// Returns the removed entry, or `None` if not found.
    pub fn remove(&mut self, id: &EffectId) -> Option<EffectEntry> {
        debug!(id = %id, "Removing effect from registry");
        self.effects.remove(id)
    }

    /// Look up an effect by its unique id.
    #[must_use]
    pub fn get(&self, id: &EffectId) -> Option<&EffectEntry> {
        self.effects.get(id)
    }

    /// Look up an effect mutably by its unique id.
    pub fn get_mut(&mut self, id: &EffectId) -> Option<&mut EffectEntry> {
        self.effects.get_mut(id)
    }

    /// Returns an iterator over all registered effects.
    pub fn iter(&self) -> impl Iterator<Item = (&EffectId, &EffectEntry)> {
        self.effects.iter()
    }

    /// List all effects in a given category.
    #[must_use]
    pub fn by_category(&self, category: EffectCategory) -> Vec<&EffectEntry> {
        self.effects
            .values()
            .filter(|entry| entry.metadata.category == category)
            .collect()
    }

    /// Search effects by name or tag substring (case-insensitive).
    ///
    /// Returns all effects where the query matches any of:
    /// - The effect name
    /// - The effect description
    /// - Any tag
    #[must_use]
    pub fn search(&self, query: &str) -> Vec<&EffectEntry> {
        let q = query.to_lowercase();
        self.effects
            .values()
            .filter(|entry| {
                let meta = &entry.metadata;
                meta.name.to_lowercase().contains(&q)
                    || meta.description.to_lowercase().contains(&q)
                    || meta.tags.iter().any(|tag| tag.to_lowercase().contains(&q))
            })
            .collect()
    }

    /// List all effects whose source file lives under the given directory.
    #[must_use]
    pub fn by_directory(&self, dir: &Path) -> Vec<&EffectEntry> {
        self.effects
            .values()
            .filter(|entry| entry.source_path.starts_with(dir))
            .collect()
    }

    /// List all unique categories present in the registry.
    #[must_use]
    pub fn categories(&self) -> Vec<EffectCategory> {
        let mut cats: Vec<EffectCategory> = self
            .effects
            .values()
            .map(|entry| entry.metadata.category)
            .collect();
        cats.sort_by_key(|c| format!("{c:?}"));
        cats.dedup();
        cats
    }

    /// List all unique tags across all registered effects.
    #[must_use]
    pub fn all_tags(&self) -> Vec<String> {
        let mut tags: Vec<String> = self
            .effects
            .values()
            .flat_map(|entry| entry.metadata.tags.clone())
            .collect();
        tags.sort();
        tags.dedup();
        tags
    }

    /// Remove effects whose source file no longer exists on disk.
    ///
    /// Returns the ids of removed effects.
    pub fn prune_missing(&mut self) -> Vec<EffectId> {
        let stale: Vec<EffectId> = self
            .effects
            .iter()
            .filter(|(_, entry)| !entry.source_path.exists())
            .map(|(id, _)| *id)
            .collect();

        for id in &stale {
            warn!(id = %id, "Pruning missing effect from registry");
            self.effects.remove(id);
        }

        stale
    }
}

impl Default for EffectRegistry {
    fn default() -> Self {
        Self::new(Vec::new())
    }
}
