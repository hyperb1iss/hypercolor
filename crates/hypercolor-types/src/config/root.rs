use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::session::SessionConfig;

use super::{
    AudioConfig, CaptureConfig, DaemonConfig, DbusConfig, DiscoveryConfig, DisplayConfig,
    DriverConfigs, EffectEngineConfig, FeatureFlags, InputConfig, McpConfig, MediaConfig,
    NetworkConfig, RenderingConfig, TuiConfig, WebConfig, default_driver_configs,
};

// ─── Top-Level Config ────────────────────────────────────────────────────────

/// Root configuration loaded from `hypercolor.toml`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HypercolorConfig {
    /// Schema version for migration tracking.
    pub schema_version: u32,

    /// Additional TOML files to merge (relative paths).
    #[serde(default)]
    pub include: Vec<String>,

    #[serde(default)]
    pub daemon: DaemonConfig,

    #[serde(default)]
    pub web: WebConfig,

    #[serde(default)]
    pub mcp: McpConfig,

    #[serde(default)]
    pub effect_engine: EffectEngineConfig,

    #[serde(default)]
    pub rendering: RenderingConfig,

    #[serde(default)]
    pub media: MediaConfig,

    #[serde(default)]
    pub audio: AudioConfig,

    #[serde(default)]
    pub capture: CaptureConfig,

    #[serde(default)]
    pub input: InputConfig,

    #[serde(default)]
    pub display: DisplayConfig,

    #[serde(default)]
    pub discovery: DiscoveryConfig,

    #[serde(default)]
    pub network: NetworkConfig,

    #[serde(default = "default_driver_configs")]
    pub drivers: DriverConfigs,

    #[serde(default)]
    pub dbus: DbusConfig,

    #[serde(default)]
    pub tui: TuiConfig,

    #[serde(default)]
    pub session: SessionConfig,

    #[serde(default)]
    pub features: FeatureFlags,

    /// Top-level sections this build does not model, preserved verbatim.
    ///
    /// The daemon persists config as a whole-file rewrite, and extension
    /// crates (the official cloud daemon's `[cloud]` section, for one)
    /// share this file. Without a catch-all, every save silently deletes
    /// their configuration.
    #[serde(default, flatten, skip_serializing_if = "BTreeMap::is_empty")]
    pub extensions: BTreeMap<String, serde_json::Value>,
}

/// Current schema version for newly created configurations.
pub const CURRENT_SCHEMA_VERSION: u32 = 5;

impl Default for HypercolorConfig {
    fn default() -> Self {
        Self {
            schema_version: CURRENT_SCHEMA_VERSION,
            include: Vec::new(),
            daemon: DaemonConfig::default(),
            web: WebConfig::default(),
            mcp: McpConfig::default(),
            effect_engine: EffectEngineConfig::default(),
            rendering: RenderingConfig::default(),
            media: MediaConfig::default(),
            audio: AudioConfig::default(),
            capture: CaptureConfig::default(),
            input: InputConfig::default(),
            display: DisplayConfig::default(),
            discovery: DiscoveryConfig::default(),
            network: NetworkConfig::default(),
            drivers: default_driver_configs(),
            dbus: DbusConfig::default(),
            tui: TuiConfig::default(),
            session: SessionConfig::default(),
            features: FeatureFlags::default(),
            extensions: BTreeMap::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::HypercolorConfig;

    #[test]
    fn unknown_top_level_sections_survive_a_round_trip() {
        let source = r"
schema_version = 5

[daemon]
port = 9420

[cloud]
enabled = true
connect_on_start = true
";
        let parsed: HypercolorConfig = toml::from_str(source).expect("parses");
        assert_eq!(
            parsed
                .extensions
                .get("cloud")
                .and_then(|section| section.get("enabled"))
                .and_then(serde_json::Value::as_bool),
            Some(true),
            "the [cloud] section lands in the catch-all"
        );

        let rewritten = toml::to_string_pretty(&parsed).expect("serializes");
        let reparsed: HypercolorConfig = toml::from_str(&rewritten).expect("reparses");
        assert_eq!(
            reparsed.extensions.get("cloud"),
            parsed.extensions.get("cloud"),
            "a persist rewrite must not delete extension config"
        );
    }
}
