//! CLI configuration: connection profiles, defaults, and persistence.
//!
//! Stored at `~/.config/hypercolor/cli.toml` (platform-appropriate via `dirs`).
//! Created lazily on first write; absence on read means compiled-in defaults.

use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

// ── Schema ──────────────────────────────────────────────────────────────

/// Top-level CLI config file.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct CliConfig {
    pub defaults: Defaults,
    #[serde(default)]
    pub profiles: BTreeMap<String, Profile>,
}

/// Global default settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Defaults {
    pub profile: String,
    pub theme: Option<String>,
    pub format: Option<String>,
    pub color: Option<String>,
}

impl Default for Defaults {
    fn default() -> Self {
        Self {
            profile: "local".to_string(),
            theme: None,
            format: None,
            color: None,
        }
    }
}

/// A named connection profile targeting a specific daemon instance.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Profile {
    pub host: String,
    pub port: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

impl Default for Profile {
    fn default() -> Self {
        Self {
            host: "localhost".to_string(),
            port: 9420,
            api_key: None,
            label: None,
            description: None,
        }
    }
}

/// Resolved connection parameters after merging flags, env, and profile.
#[derive(Debug)]
pub struct ResolvedConnection {
    pub host: String,
    pub port: u16,
    pub api_key: Option<String>,
    pub profile_name: String,
}

// ── File Operations ─────────────────────────────────────────────────────

/// Return the path to the CLI config file.
pub fn config_path() -> PathBuf {
    if let Ok(path) = std::env::var("HYPERCOLOR_CLI_CONFIG") {
        return PathBuf::from(path);
    }
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("~/.config"))
        .join("hypercolor")
        .join("cli.toml")
}

/// Load the CLI config from disk. Returns default config if file doesn't exist.
pub fn load() -> Result<CliConfig> {
    let path = config_path();
    if !path.exists() {
        return Ok(CliConfig::default());
    }
    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    let config: CliConfig =
        toml::from_str(&content).with_context(|| format!("invalid TOML in {}", path.display()))?;
    Ok(config)
}

/// Save the CLI config to disk, creating the directory and setting 0600 on Unix.
pub fn save(config: &CliConfig) -> Result<()> {
    let path = config_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let content = toml::to_string_pretty(config).context("failed to serialize config")?;
    std::fs::write(&path, &content)
        .with_context(|| format!("failed to write {}", path.display()))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        std::fs::set_permissions(&path, perms)
            .with_context(|| format!("failed to set permissions on {}", path.display()))?;
    }

    Ok(())
}

// ── Profile Resolution ──────────────────────────────────────────────────

/// Resolve connection parameters from CLI flags, env vars, and profiles.
///
/// Precedence (highest wins):
///   1. Explicit `--host`/`--port`/`--api-key` flags
///   2. `HYPERCOLOR_HOST`/`HYPERCOLOR_PORT`/`HYPERCOLOR_API_KEY` env vars
///   3. Named profile fields from cli.toml
///   4. Compiled-in defaults (localhost:9420, no auth)
pub fn resolve_connection(
    flag_host: &str,
    flag_port: u16,
    flag_api_key: Option<&str>,
    flag_profile: Option<&str>,
) -> Result<ResolvedConnection> {
    let config = load()?;

    let profile_name = flag_profile
        .map(ToOwned::to_owned)
        .or_else(|| std::env::var("HYPERCOLOR_PROFILE").ok())
        .unwrap_or(config.defaults.profile.clone());

    let explicitly_requested =
        flag_profile.is_some() || std::env::var("HYPERCOLOR_PROFILE").is_ok();

    let profile = config.profiles.get(&profile_name);

    if profile.is_none() && explicitly_requested {
        eprintln!(
            "  ! profile {profile_name:?} not found in {} \
             (run `hyper config profile list` to see available profiles)",
            config_path().display()
        );
    }

    let host_is_default = flag_host == "localhost";
    let port_is_default = flag_port == 9420;

    let host = if !host_is_default {
        flag_host.to_string()
    } else if let Ok(env_host) = std::env::var("HYPERCOLOR_HOST") {
        env_host
    } else if let Some(p) = profile {
        p.host.clone()
    } else {
        "localhost".to_string()
    };

    let port = if !port_is_default {
        flag_port
    } else if let Ok(env_port) = std::env::var("HYPERCOLOR_PORT") {
        env_port.parse().unwrap_or(9420)
    } else if let Some(p) = profile {
        p.port
    } else {
        9420
    };

    let api_key = flag_api_key
        .map(ToOwned::to_owned)
        .or_else(|| std::env::var("HYPERCOLOR_API_KEY").ok())
        .or_else(|| profile.and_then(|p| p.api_key.as_ref().filter(|k| !k.is_empty()).cloned()));

    Ok(ResolvedConnection {
        host,
        port,
        api_key,
        profile_name,
    })
}
