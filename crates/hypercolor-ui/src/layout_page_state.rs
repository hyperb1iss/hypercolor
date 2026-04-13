//! Persistence for the layout page's UI state — which layout is open,
//! which zones are selected, which are hidden, compound-selection depth,
//! plus a couple of UI-only preferences. Lets the layout page come back
//! exactly the way the user left it after navigating elsewhere.
//!
//! Stored as a single JSON blob under [`STORAGE_KEY`]; per-layout state
//! is keyed by layout ID so switching layouts and coming back restores
//! each one's individual selection independently. Follows the same
//! save-on-mutation pattern as [`crate::preferences`].

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::compound_selection::CompoundDepth;
use crate::storage;

const STORAGE_KEY: &str = "hc-layout-page-state";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct LayoutPageState {
    #[serde(default)]
    pub selected_layout_id: Option<String>,
    #[serde(default = "default_keep_aspect_ratio")]
    pub keep_aspect_ratio: bool,
    #[serde(default)]
    pub per_layout: HashMap<String, PerLayoutState>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(crate) struct PerLayoutState {
    #[serde(default)]
    pub selected_zone_ids: HashSet<String>,
    #[serde(default)]
    pub hidden_zones: HashSet<String>,
    #[serde(default)]
    pub compound_depth: CompoundDepth,
}

impl Default for LayoutPageState {
    fn default() -> Self {
        Self {
            selected_layout_id: None,
            keep_aspect_ratio: true,
            per_layout: HashMap::new(),
        }
    }
}

impl LayoutPageState {
    /// Read the persisted blob. Corrupt or missing data is treated as
    /// "no prior state" — we'd rather start clean than crash on a stale
    /// blob from an older build.
    pub fn load() -> Self {
        storage::get(STORAGE_KEY)
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        if let Ok(json) = serde_json::to_string(self) {
            storage::set(STORAGE_KEY, &json);
        }
    }
}

fn default_keep_aspect_ratio() -> bool {
    true
}
