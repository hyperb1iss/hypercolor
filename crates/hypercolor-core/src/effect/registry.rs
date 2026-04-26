//! Effect registry — discovery, indexing, and search.
//!
//! The [`EffectRegistry`] scans effect directories, parses metadata, and
//! provides lookup/search/filter operations over the known effect catalog.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use tracing::{debug, info, warn};

use hypercolor_types::effect::{
    EffectCategory, EffectId, EffectMetadata, EffectSource, EffectState,
};

// ── RescanReport ─────────────────────────────────────────────────────────────

/// Summary of what changed during an effect registry rescan.
#[derive(Debug, Clone, Default)]
pub struct RescanReport {
    /// Number of newly discovered effects.
    pub added: usize,
    /// Number of effects removed (source file deleted).
    pub removed: usize,
    /// Number of effects re-loaded (source file modified).
    pub updated: usize,
}

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

impl EffectEntry {
    fn matches_active_scene_semantics(&self, other: &Self) -> bool {
        self.metadata == other.metadata
            && self.source_path == other.source_path
            && self.modified == other.modified
    }
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

    /// Monotonic revision for invalidation-sensitive registry changes.
    generation: u64,
}

impl EffectRegistry {
    /// Create an empty registry with the given search paths.
    #[must_use]
    pub fn new(search_paths: Vec<PathBuf>) -> Self {
        info!(paths = ?search_paths, "Creating effect registry");
        Self {
            effects: HashMap::new(),
            search_paths,
            generation: 0,
        }
    }

    /// Returns the configured search paths.
    #[must_use]
    pub fn search_paths(&self) -> &[PathBuf] {
        &self.search_paths
    }

    /// Returns the current registry generation.
    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.generation
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
        let existing = self.effects.get(&id);
        if let Some(existing) = existing {
            debug!(
                id = %id,
                name = %entry.metadata.name,
                previous_name = %existing.metadata.name,
                "Replacing effect"
            );
        } else {
            debug!(id = %id, name = %entry.metadata.name, "Registering effect");
        }
        let invalidates =
            existing.is_none_or(|existing| !existing.matches_active_scene_semantics(&entry));
        let replaced = self.effects.insert(id, entry);
        if invalidates {
            self.bump_generation();
        }
        replaced
    }

    /// Remove an effect from the registry by id.
    ///
    /// Returns the removed entry, or `None` if not found.
    pub fn remove(&mut self, id: &EffectId) -> Option<EffectEntry> {
        debug!(id = %id, "Removing effect from registry");
        let removed = self.effects.remove(id);
        if removed.is_some() {
            self.bump_generation();
        }
        removed
    }

    /// Look up an effect by its unique id.
    #[must_use]
    pub fn get(&self, id: &EffectId) -> Option<&EffectEntry> {
        self.effects.get(id)
    }

    /// Apply a semantic mutation to an effect entry and advance generation.
    pub fn update(&mut self, id: &EffectId, update: impl FnOnce(&mut EffectEntry)) -> Option<bool> {
        let invalidates = {
            let entry = self.effects.get_mut(id)?;
            let before = entry.clone();
            update(entry);
            !before.matches_active_scene_semantics(entry)
        };
        if invalidates {
            self.bump_generation();
        }
        Some(invalidates)
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

    /// Full rescan: re-registers all HTML effects and prunes stale entries.
    ///
    /// Returns a diff summary of what changed.
    pub fn rescan(&mut self) -> RescanReport {
        let before: std::collections::HashSet<EffectId> = self.effects.keys().copied().collect();

        let paths = self.search_paths.clone();
        let html_report = super::loader::register_html_effects(self, &paths);
        let pruned = self.prune_missing();

        let after: std::collections::HashSet<EffectId> = self.effects.keys().copied().collect();

        let added = after.difference(&before).count();
        let removed = pruned.len();
        let updated = html_report.replaced_effects;

        let report = RescanReport {
            added,
            removed,
            updated,
        };

        info!(
            added = report.added,
            removed = report.removed,
            updated = report.updated,
            "Effect registry rescan complete"
        );

        report
    }

    /// Fast path for a single file change — re-parse and re-register one effect.
    ///
    /// If `path` no longer exists, the effect is removed from the registry.
    /// Returns a diff summary.
    pub fn reload_single(&mut self, path: &std::path::Path) -> RescanReport {
        // If the file was deleted, prune it.
        if !path.exists() {
            let removed_count = self.remove_by_source_path(path);
            return RescanReport {
                added: 0,
                removed: removed_count,
                updated: 0,
            };
        }

        // Re-register just this one file by running the full loader
        // against a single-file search scope. We construct a temporary
        // search path containing just the parent directory and filter
        // to only this file by leveraging register_html_effects' idempotency.
        let paths = self.search_paths.clone();
        let had_before = self.has_source_path(path);
        let _report = super::loader::register_html_effects(self, &paths);

        if had_before {
            RescanReport {
                added: 0,
                removed: 0,
                updated: 1,
            }
        } else {
            RescanReport {
                added: 1,
                removed: 0,
                updated: 0,
            }
        }
    }

    /// Check if any registered effect has the given source path.
    fn has_source_path(&self, path: &std::path::Path) -> bool {
        self.effects.values().any(|entry| entry.source_path == path)
    }

    /// Remove all effects whose source path matches the given path.
    /// Returns the count of removed effects.
    fn remove_by_source_path(&mut self, path: &std::path::Path) -> usize {
        let stale: Vec<EffectId> = self
            .effects
            .iter()
            .filter(|(_, entry)| entry.source_path == path)
            .map(|(id, _)| *id)
            .collect();

        let count = stale.len();
        for id in stale {
            info!(id = %id, path = %path.display(), "Removing deleted effect");
            let _ = self.remove(&id);
        }
        count
    }

    /// Remove effects whose source file no longer exists on disk.
    ///
    /// Returns the ids of removed effects.
    pub fn prune_missing(&mut self) -> Vec<EffectId> {
        let stale: Vec<EffectId> = self
            .effects
            .iter()
            .filter(|(_, entry)| {
                !matches!(entry.metadata.source, EffectSource::Native { .. })
                    && !entry.source_path.exists()
            })
            .map(|(id, _)| *id)
            .collect();

        for id in &stale {
            warn!(id = %id, "Pruning missing effect from registry");
            let _ = self.remove(id);
        }

        stale
    }

    fn bump_generation(&mut self) {
        self.generation = self.generation.saturating_add(1);
    }
}

impl Default for EffectRegistry {
    fn default() -> Self {
        Self::new(Vec::new())
    }
}
