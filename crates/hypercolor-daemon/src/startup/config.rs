//! Config sources for the daemon, server identity resolution, and
//! instance ID management.
//!
//! Loading itself lives in [`ConfigManager::load_with_sources`] (Spec 76
//! §3.1). What the daemon owns is the source set: which file, which CLI
//! flags overlay it, and the driver-entry seeding hook the builtin driver
//! bundle supplies from above core.

use std::path::PathBuf;

use anyhow::{Context, Result};
use tracing::warn;
use uuid::Uuid;

use hypercolor_core::config::{CliOverrides, ConfigManager, ConfigSources, EnvOverrides};
use hypercolor_types::config::{HypercolorConfig, RenderAccelerationMode, ServoGpuImportMode};
use hypercolor_types::server::ServerIdentity;

use crate::network;

const INSTANCE_ID_FILE_NAME: &str = "instance_id";
const DEFAULT_INSTANCE_NAME: &str = "hypercolor";

/// Everything one daemon start feeds into the load pipeline.
///
/// `file` is the explicit `--config` path (or `HYPERCOLOR_CONFIG`): when
/// set the file must exist, and when absent the platform default path is
/// used, falling back to defaults. The seed hook installs the builtin
/// driver entries that live above core.
#[must_use]
pub fn config_sources(
    file: Option<PathBuf>,
    compositor_acceleration_mode: Option<RenderAccelerationMode>,
    servo_gpu_import_mode: Option<ServoGpuImportMode>,
) -> ConfigSources {
    ConfigSources {
        file,
        cli: CliOverrides {
            compositor_acceleration_mode,
            servo_gpu_import_mode,
        },
        env: EnvOverrides::from_process_env(),
        seed: Some(normalize_daemon_driver_configs),
    }
}

/// Construct a default configuration (all defaults, current schema version).
#[must_use]
pub fn default_config() -> HypercolorConfig {
    let mut config = ConfigManager::default_config();
    normalize_daemon_driver_configs(&mut config);
    config
}

/// Parse a TOML string into a [`HypercolorConfig`] for tests and tooling.
///
/// Runs the daemon's driver seeding on top of the one canonical parser, so
/// a parsed string and a loaded file yield the same config.
///
/// # Errors
///
/// Returns an error if the TOML is malformed or cannot be deserialized.
pub fn parse_config_toml(toml_str: &str) -> Result<HypercolorConfig> {
    let mut config = ConfigManager::parse_toml(toml_str)?;
    normalize_daemon_driver_configs(&mut config);
    Ok(config)
}

pub(crate) fn normalize_daemon_driver_configs(config: &mut HypercolorConfig) {
    network::normalize_builtin_driver_config_entries(config);
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
