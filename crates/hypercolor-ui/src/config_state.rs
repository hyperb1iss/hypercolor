use std::collections::HashMap;

use hypercolor_types::config::HypercolorConfig;
use hypercolor_types::config_registry::{ApplyPolicy, ConfigKeySchemaEntry};
use leptos::prelude::*;

#[derive(Clone, Copy)]
pub struct ConfigContext {
    pub config: ReadSignal<Option<HypercolorConfig>>,
    pub set_config: WriteSignal<Option<HypercolorConfig>>,
    pub refresh: Callback<()>,
    pub audio_enabled: Memo<bool>,
}

/// The daemon's config key registry, read once per connection from
/// `GET /api/v1/config/schema`. Every live/restart affordance in the
/// settings surface derives from it rather than from a hand mirror.
#[derive(Clone, Copy)]
pub struct ConfigSchemaContext {
    pub entries: Signal<Vec<ConfigKeySchemaEntry>>,
}

impl ConfigSchemaContext {
    /// Whether a change to `key` only takes effect at the next daemon
    /// start. False while the schema is still in flight, so a badge
    /// appears once the answer is known rather than guessed.
    #[must_use]
    pub fn requires_restart(&self, key: &str) -> bool {
        self.entries
            .with(|entries| schema_requires_restart(entries, key))
    }
}

/// Whether the schema classifies `key` as boot-frozen.
#[must_use]
pub fn schema_requires_restart(entries: &[ConfigKeySchemaEntry], key: &str) -> bool {
    schema_entry_for(entries, key).is_some_and(|entry| matches!(entry.apply, ApplyPolicy::Restart))
}

/// The most specific schema entry covering `key`.
///
/// Mirrors the daemon's lookup over the wire projection: the deepest
/// matching pattern wins, so an exact key beats the section it sits in,
/// and the `*` catch-all is the last resort. Depth alone settles it
/// because each root — section or `root.*` namespace — appears once.
#[must_use]
pub fn schema_entry_for<'a>(
    entries: &'a [ConfigKeySchemaEntry],
    key: &str,
) -> Option<&'a ConfigKeySchemaEntry> {
    entries
        .iter()
        .filter_map(|entry| pattern_specificity(&entry.pattern, key).map(|rank| (rank, entry)))
        .max_by_key(|(rank, _)| *rank)
        .map(|(_, entry)| entry)
}

/// How specifically `pattern` covers `key`, or `None` when it does not.
/// Deeper patterns win; the catch-all sits below every named pattern.
fn pattern_specificity(pattern: &str, key: &str) -> Option<usize> {
    if pattern == "*" {
        return Some(0);
    }

    let root = pattern.strip_suffix(".*").unwrap_or(pattern);
    if key == root || key.strip_prefix(root).is_some_and(|rest| rest.starts_with('.')) {
        return Some(root.split('.').count());
    }
    None
}

#[derive(Debug, Default)]
pub struct ConfigApplyTracker {
    next_generation: u64,
    current: HashMap<String, u64>,
}

impl ConfigApplyTracker {
    pub fn begin(&mut self, key: &str) -> u64 {
        self.next_generation = self
            .next_generation
            .checked_add(1)
            .expect("config apply generation exhausted");
        self.current.insert(key.to_owned(), self.next_generation);
        self.next_generation
    }

    pub fn finish_if_current(&mut self, key: &str, generation: u64) -> bool {
        if self.current.get(key).copied() != Some(generation) {
            return false;
        }
        self.current.remove(key);
        true
    }
}

#[must_use]
pub fn config_key_value(config: &HypercolorConfig, key: &str) -> Option<serde_json::Value> {
    let root = serde_json::to_value(config).ok()?;
    key.split('.')
        .try_fold(&root, |cursor, part| cursor.get(part))
        .cloned()
}

pub fn apply_config_key(config: &mut HypercolorConfig, key: &str, value: &serde_json::Value) {
    let Ok(mut root) = serde_json::to_value(&*config) else {
        return;
    };

    let parts: Vec<&str> = key.split('.').collect();
    if parts.is_empty() {
        return;
    }

    let (parents, leaf) = parts.split_at(parts.len() - 1);
    let mut cursor = &mut root;
    for &part in parents {
        let Some(obj) = cursor.as_object_mut() else {
            return;
        };
        cursor = obj
            .entry(part.to_owned())
            .or_insert_with(|| serde_json::json!({}));
    }

    if let Some(obj) = cursor.as_object_mut() {
        obj.insert(leaf[0].to_owned(), value.clone());
    }

    if let Ok(updated) = serde_json::from_value(root) {
        *config = updated;
    }
}
