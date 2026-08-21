use serde::{Deserialize, Serialize};

// ─── Feature Flags ───────────────────────────────────────────────────────────

/// Opt-in experimental features (all default to `false`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FeatureFlags {
    #[serde(default)]
    pub wasm_plugins: bool,

    #[serde(default)]
    pub hue_entertainment: bool,

    #[serde(default)]
    pub midi_input: bool,
}
