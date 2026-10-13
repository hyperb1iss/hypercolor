use std::collections::BTreeMap;

use serde::{Deserialize, Deserializer, Serialize};

use crate::session::SessionConfig;

use super::{
    AudioConfig, CaptureConfig, DaemonConfig, DiscoveryConfig, DisplayConfig, DriverConfigs,
    EffectEngineConfig, InputConfig, McpConfig, MediaConfig, NetworkConfig, RenderingConfig,
    WebConfig, default_driver_configs,
};

// ─── Top-Level Config ────────────────────────────────────────────────────────

/// Root configuration loaded from `hypercolor.toml`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HypercolorConfig {
    /// Schema version for migration tracking.
    pub schema_version: u32,

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
    pub session: SessionConfig,

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

impl HypercolorConfig {
    /// Decode a config document from an authoritative source, refusing any
    /// key this build does not model.
    ///
    /// Clients decode the daemon's config payload with the plain
    /// `Deserialize` impl, which skips fields it does not know so an older
    /// UI can still read a newer daemon. The config file, startup overlays,
    /// and REST writes must not skip anything: a skipped key vanishes on the
    /// next whole-file rewrite, and a misspelled one silently does nothing.
    /// This decode collects every key the derive skipped and refuses the
    /// document, naming each by its dotted path. Root extension sections and
    /// driver entries are open maps, so their keys are captured rather than
    /// skipped and stay accepted.
    ///
    /// # Errors
    ///
    /// Returns the deserializer's own error when the document does not fit
    /// the schema, and an error naming every unknown key when it fits but
    /// carries keys this build does not read.
    pub fn deserialize_strict<'de, D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        super::strict::deserialize_strict_under(None, deserializer)
    }
}

impl Default for HypercolorConfig {
    fn default() -> Self {
        Self {
            schema_version: CURRENT_SCHEMA_VERSION,
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
            session: SessionConfig::default(),
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

    #[test]
    fn retired_sections_load_and_survive_a_persist_rewrite() {
        let source = r#"
schema_version = 5
include = ["local.toml"]

[daemon]
port = 9420

[dbus]
enabled = true
bus_name = "tech.hyperbliss.hypercolor1"

[tui]
theme = "silkcircuit"

[features]
wasm_plugins = true
"#;
        let parsed: HypercolorConfig = toml::from_str(source).expect("an old config still loads");
        assert_eq!(parsed.daemon.port, 9420);
        for retired in ["include", "dbus", "tui", "features"] {
            assert!(
                parsed.extensions.contains_key(retired),
                "[{retired}] should land in the catch-all"
            );
        }

        let rewritten = toml::to_string_pretty(&parsed).expect("serializes");
        let reparsed: HypercolorConfig = toml::from_str(&rewritten).expect("reparses");
        assert_eq!(reparsed.extensions, parsed.extensions);
    }
}
