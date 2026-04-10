//! Configuration loading, server identity resolution, and instance ID management.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use tracing::{info, warn};
use uuid::Uuid;

use hypercolor_core::config::ConfigManager;
use hypercolor_types::config::HypercolorConfig;
use hypercolor_types::server::ServerIdentity;

/// Default configuration file name within the config directory.
const CONFIG_FILE_NAME: &str = "hypercolor.toml";
const INSTANCE_ID_FILE_NAME: &str = "instance_id";
const DEFAULT_INSTANCE_NAME: &str = "hypercolor";

/// Load and validate configuration from the filesystem.
///
/// Resolution order:
/// 1. Explicit path from `--config` CLI argument
/// 2. Platform-specific config directory (`$XDG_CONFIG_HOME/hypercolor/hypercolor.toml`
///    on Linux, `%APPDATA%\hypercolor\hypercolor.toml` on Windows)
/// 3. Fall back to compile-time defaults (no file needed)
///
/// # Errors
///
/// Returns an error if an explicit config path is provided but the file
/// cannot be read or parsed. When falling back to defaults, this always
/// succeeds.
#[expect(
    clippy::unused_async,
    reason = "will be async when config loading gains network support"
)]
pub async fn load_config(config_path: Option<&Path>) -> Result<(HypercolorConfig, PathBuf)> {
    let resolved_path = resolve_config_path(config_path);

    info!(path = %resolved_path.display(), "Resolved config path");

    if resolved_path.exists() {
        let config = ConfigManager::load(&resolved_path)
            .with_context(|| format!("failed to load config from {}", resolved_path.display()))?;
        info!(
            schema_version = config.schema_version,
            "Configuration loaded from file"
        );
        Ok((config, resolved_path))
    } else if config_path.is_some() {
        // Explicit path was given but doesn't exist — that's an error.
        anyhow::bail!("config file not found: {}", resolved_path.display());
    } else {
        // No explicit path, no file found — use defaults.
        warn!("No config file found, using built-in defaults");
        let config = default_config();
        Ok((config, resolved_path))
    }
}

/// Resolve which config file path to use.
///
/// If an explicit path is provided, it is used directly. Otherwise the
/// platform-specific config directory is checked for `hypercolor.toml`.
fn resolve_config_path(explicit: Option<&Path>) -> PathBuf {
    explicit.map_or_else(
        || ConfigManager::config_dir().join(CONFIG_FILE_NAME),
        Path::to_path_buf,
    )
}

/// Construct a default configuration (all defaults, current schema version).
pub fn default_config() -> HypercolorConfig {
    HypercolorConfig::default()
}

/// Parse a TOML string into a [`HypercolorConfig`].
///
/// Convenience wrapper around `toml::from_str` for tests and tooling.
///
/// # Errors
///
/// Returns an error if the TOML is malformed or cannot be deserialized.
pub fn parse_config_toml(toml_str: &str) -> Result<HypercolorConfig> {
    toml::from_str(toml_str).context("failed to parse config TOML")
}

pub(super) fn resolve_server_identity(config: &HypercolorConfig) -> Result<ServerIdentity> {
    let instance_id = load_or_create_instance_id()?;
    let instance_name = config
        .network
        .instance_name
        .clone()
        .unwrap_or_else(default_instance_name);

    Ok(ServerIdentity {
        instance_id,
        instance_name,
        version: env!("CARGO_PKG_VERSION").to_owned(),
    })
}

fn load_or_create_instance_id() -> Result<String> {
    let instance_id_path = ConfigManager::data_dir().join(INSTANCE_ID_FILE_NAME);

    if let Ok(raw) = std::fs::read_to_string(&instance_id_path) {
        let trimmed = raw.trim();
        if !trimmed.is_empty() && Uuid::parse_str(trimmed).is_ok() {
            return Ok(trimmed.to_owned());
        }

        warn!(
            path = %instance_id_path.display(),
            "Ignoring invalid persisted instance ID; generating a replacement"
        );
    }

    if let Some(parent) = instance_id_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }

    let instance_id = Uuid::now_v7().to_string();
    std::fs::write(&instance_id_path, format!("{instance_id}\n"))
        .with_context(|| format!("failed to write {}", instance_id_path.display()))?;

    Ok(instance_id)
}

fn default_instance_name() -> String {
    env_hostname()
        .or_else(os_hostname)
        .unwrap_or_else(|| DEFAULT_INSTANCE_NAME.to_owned())
}

fn env_hostname() -> Option<String> {
    ["HOSTNAME", "COMPUTERNAME"].iter().find_map(|key| {
        std::env::var(key).ok().and_then(|value| {
            let trimmed = value.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_owned())
            }
        })
    })
}

#[cfg(unix)]
fn os_hostname() -> Option<String> {
    std::fs::read_to_string("/etc/hostname")
        .ok()
        .and_then(|value| {
            let trimmed = value.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_owned())
            }
        })
}

#[cfg(not(unix))]
fn os_hostname() -> Option<String> {
    None
}
