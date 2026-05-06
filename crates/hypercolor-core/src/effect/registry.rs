//! Effect registry — discovery, indexing, and search.
//!
//! The [`EffectRegistry`] scans effect directories, parses metadata, and
//! provides lookup/search/filter operations over the known effect catalog.

use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::fs;
use std::path::{Component, Path, PathBuf};
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

    /// Compatibility ids that resolve to canonical effect ids.
    aliases: HashMap<EffectId, EffectId>,

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
            aliases: HashMap::new(),
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
        let new_aliases = super::loader::html_effect_aliases(&entry)
            .into_iter()
            .collect::<HashSet<_>>();
        let aliases_changed = self.aliases_for(id) != new_aliases;
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
        self.aliases
            .retain(|alias, canonical| *alias != id && *canonical != id);
        for alias in new_aliases {
            self.aliases.insert(alias, id);
        }
        if invalidates || aliases_changed {
            self.bump_generation();
        }
        replaced
    }

    /// Remove an effect from the registry by id.
    ///
    /// Returns the removed entry, or `None` if not found.
    pub fn remove(&mut self, id: &EffectId) -> Option<EffectEntry> {
        debug!(id = %id, "Removing effect from registry");
        let canonical_id = if self.effects.contains_key(id) {
            *id
        } else {
            self.aliases.get(id).copied().unwrap_or(*id)
        };
        let removed = self.effects.remove(&canonical_id);
        let alias_count_before = self.aliases.len();
        self.aliases.retain(|alias, canonical| {
            *alias != *id && *alias != canonical_id && *canonical != canonical_id
        });
        if removed.is_some() || self.aliases.len() != alias_count_before {
            self.bump_generation();
        }
        removed
    }

    /// Look up an effect by its unique id.
    #[must_use]
    pub fn get(&self, id: &EffectId) -> Option<&EffectEntry> {
        self.resolve_id(id)
            .and_then(|resolved_id| self.effects.get(&resolved_id))
    }

    /// Resolve a canonical id from a current id or a compatibility alias.
    #[must_use]
    pub fn resolve_id(&self, id: &EffectId) -> Option<EffectId> {
        if self.effects.contains_key(id) {
            return Some(*id);
        }

        self.aliases
            .get(id)
            .copied()
            .filter(|resolved_id| self.effects.contains_key(resolved_id))
    }

    /// Apply a semantic mutation to an effect entry and advance generation.
    pub fn update(&mut self, id: &EffectId, update: impl FnOnce(&mut EffectEntry)) -> Option<bool> {
        let canonical_id = self.resolve_id(id)?;
        let invalidates = {
            let entry = self.effects.get_mut(&canonical_id)?;
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
        if !is_html_file(path) {
            return RescanReport::default();
        }

        // If the file was deleted, prune it.
        if !path.exists() {
            let removed_count = self.remove_by_source_path(path);
            return RescanReport {
                added: 0,
                removed: removed_count,
                updated: 0,
            };
        }

        let entry = match super::loader::load_html_effect_file(path) {
            Ok(Some(entry)) => entry,
            Ok(None) => {
                let removed_count = self.remove_by_source_path(path);
                return RescanReport {
                    added: 0,
                    removed: removed_count,
                    updated: 0,
                };
            }
            Err(error) => {
                warn!(
                    path = %error.path.display(),
                    error = %error.message,
                    "Failed to reload HTML effect"
                );
                return RescanReport::default();
            }
        };

        let stale_count =
            self.remove_by_source_path_except(&entry.source_path, Some(entry.metadata.id));
        if self.register(entry).is_some() {
            RescanReport {
                added: 0,
                removed: stale_count,
                updated: 1,
            }
        } else {
            RescanReport {
                added: 1,
                removed: stale_count,
                updated: 0,
            }
        }
    }

    /// Remove all effects whose source path matches the given path.
    /// Returns the count of removed effects.
    fn remove_by_source_path(&mut self, path: &std::path::Path) -> usize {
        self.remove_by_source_path_except(path, None)
    }

    /// Remove effects whose source path matches the given path, except one id.
    /// Returns the count of removed effects.
    fn remove_by_source_path_except(
        &mut self,
        path: &std::path::Path,
        keep_id: Option<EffectId>,
    ) -> usize {
        let normalized_path = normalize_registry_path(path);
        let stale: Vec<EffectId> = self
            .effects
            .iter()
            .filter(|(id, entry)| {
                normalize_registry_path(&entry.source_path) == normalized_path
                    && Some(**id) != keep_id
            })
            .map(|(id, _)| *id)
            .collect();

        let count = stale.len();
        for id in stale {
            info!(id = %id, path = %normalized_path.display(), "Removing deleted effect");
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

    fn aliases_for(&self, canonical_id: EffectId) -> HashSet<EffectId> {
        self.aliases
            .iter()
            .filter_map(|(alias, target)| (*target == canonical_id).then_some(*alias))
            .collect()
    }
}

fn is_html_file(path: &Path) -> bool {
    path.extension()
        .and_then(std::ffi::OsStr::to_str)
        .is_some_and(|extension| extension.eq_ignore_ascii_case("html"))
}

fn normalize_registry_path(path: &Path) -> PathBuf {
    let lexical = normalize_path_lexically(path);
    normalize_platform_path(
        fs::canonicalize(&lexical)
            .unwrap_or_else(|_| canonicalize_existing_prefix(&lexical).unwrap_or(lexical)),
    )
}

fn canonicalize_existing_prefix(path: &Path) -> Option<PathBuf> {
    let mut current = path;
    let mut suffix = Vec::<OsString>::new();

    loop {
        if let Ok(mut canonical) = fs::canonicalize(current) {
            for component in suffix.iter().rev() {
                canonical.push(component);
            }
            return Some(canonical);
        }

        suffix.push(current.file_name()?.to_os_string());
        current = current.parent()?;
    }
}

fn normalize_path_lexically(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() {
                    normalized.push(component.as_os_str());
                }
            }
            _ => normalized.push(component.as_os_str()),
        }
    }
    normalized
}

#[cfg(windows)]
fn normalize_platform_path(path: PathBuf) -> PathBuf {
    let normalized = {
        let text = path.to_string_lossy();
        if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
            Some(PathBuf::from(format!(r"\\{rest}")))
        } else {
            text.strip_prefix(r"\\?\").map(PathBuf::from)
        }
    };

    normalized.unwrap_or(path)
}

#[cfg(not(windows))]
fn normalize_platform_path(path: PathBuf) -> PathBuf {
    path
}

impl Default for EffectRegistry {
    fn default() -> Self {
        Self::new(Vec::new())
    }
}
