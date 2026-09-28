//! CLI configuration: connection profiles, defaults, and persistence.
//!
//! Stored as `cli.toml` inside the Hypercolor config directory resolved by
//! [`hypercolor_core::config::paths`] (`~/.config/hypercolor/` on Linux,
//! `%APPDATA%\hypercolor\` on Windows). Created lazily on first write;
//! absence on read means compiled-in defaults.

use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// File name for the CLI config within the Hypercolor config directory.
const CONFIG_FILE_NAME: &str = "cli.toml";

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
            host: DEFAULT_HOST.to_string(),
            port: DEFAULT_PORT,
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
///
/// `HYPERCOLOR_CLI_CONFIG` overrides the location outright; otherwise the file
/// lives in the same config directory the daemon resolves, so every Hypercolor
/// binary agrees on where user state lives.
pub fn config_path() -> PathBuf {
    if let Ok(path) = std::env::var("HYPERCOLOR_CLI_CONFIG") {
        return PathBuf::from(path);
    }
    resolve_config_path(Some(hypercolor_core::config::paths::config_dir()))
        .expect("a resolved config directory always yields a config file path")
}

/// Place the CLI config file inside a resolved config directory.
///
/// Split out from [`config_path`] so the unresolvable case is reachable from a
/// test: without a directory this must yield nothing rather than fabricate a
/// relative path. The environment half cannot be driven directly because
/// edition 2024 makes `std::env::set_var` unsafe and this crate forbids it.
fn resolve_config_path(config_dir: Option<PathBuf>) -> Option<PathBuf> {
    Some(config_dir?.join(CONFIG_FILE_NAME))
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

/// Host a connection falls back to when neither the invocation nor a profile
/// names one.
pub const DEFAULT_HOST: &str = "localhost";

/// Port a connection falls back to when neither the invocation nor a profile
/// names one.
pub const DEFAULT_PORT: u16 = 9420;

/// Connection settings the invocation named outright, through a flag or its
/// `HYPERCOLOR_*` environment variable. `None` leaves the setting to the
/// profile, then to the compiled-in default.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConnectionRequest {
    pub host: Option<String>,
    pub port: Option<u16>,
    pub api_key: Option<String>,
    pub profile: Option<String>,
}

/// Resolve connection parameters from CLI flags, env vars, and profiles.
///
/// Precedence (highest wins):
///   1. Explicit `--host`/`--port`/`--api-key` flags
///   2. `HYPERCOLOR_HOST`/`HYPERCOLOR_PORT`/`HYPERCOLOR_API_KEY` env vars
///   3. Named profile fields from cli.toml
///   4. Compiled-in defaults (localhost:9420, no auth)
///
/// A named value always wins, including one that happens to equal the
/// default: `--port 9420` means port 9420 even when a profile says otherwise.
///
/// # Errors
///
/// Returns an error when cli.toml exists but cannot be read or parsed.
pub fn resolve_connection(request: &ConnectionRequest) -> Result<ResolvedConnection> {
    let config = load()?;
    let resolved = resolve_with_config(request, &config);
    if request.profile.is_some() && !config.profiles.contains_key(&resolved.profile_name) {
        eprintln!(
            "  ! profile {:?} not found in {} \
             (run `hypercolor config profile list` to see available profiles)",
            resolved.profile_name,
            config_path().display()
        );
    }
    Ok(resolved)
}

/// Merge a request over the loaded config, without touching the environment
/// or the filesystem.
fn resolve_with_config(request: &ConnectionRequest, config: &CliConfig) -> ResolvedConnection {
    let profile_name = request
        .profile
        .clone()
        .unwrap_or_else(|| config.defaults.profile.clone());
    let profile = config.profiles.get(&profile_name);

    let host = request
        .host
        .clone()
        .or_else(|| profile.map(|p| p.host.clone()))
        .unwrap_or_else(|| DEFAULT_HOST.to_owned());
    let port = request
        .port
        .or_else(|| profile.map(|p| p.port))
        .unwrap_or(DEFAULT_PORT);
    let api_key = request
        .api_key
        .clone()
        .or_else(|| profile.and_then(|p| p.api_key.as_ref().filter(|k| !k.is_empty()).cloned()));

    ResolvedConnection {
        host,
        port,
        api_key,
        profile_name,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CONFIG_FILE_NAME, CliConfig, ConnectionRequest, DEFAULT_HOST, DEFAULT_PORT, Profile,
        config_path, resolve_config_path, resolve_with_config,
    };

    /// A config whose default profile points somewhere no request names.
    fn config_with_default_profile() -> CliConfig {
        let mut config = CliConfig::default();
        config.profiles.insert(
            config.defaults.profile.clone(),
            Profile {
                host: "profile.example".to_owned(),
                port: 7777,
                api_key: Some("profile-key".to_owned()),
                ..Profile::default()
            },
        );
        config
    }

    #[test]
    fn named_values_equal_to_the_defaults_still_beat_the_profile() {
        let request = ConnectionRequest {
            host: Some(DEFAULT_HOST.to_owned()),
            port: Some(DEFAULT_PORT),
            ..ConnectionRequest::default()
        };

        let resolved = resolve_with_config(&request, &config_with_default_profile());

        assert_eq!(resolved.host, DEFAULT_HOST);
        assert_eq!(resolved.port, DEFAULT_PORT);
    }

    #[test]
    fn named_values_beat_the_profile() {
        let request = ConnectionRequest {
            host: Some("127.0.0.1".to_owned()),
            port: Some(41_000),
            api_key: Some("flag-key".to_owned()),
            profile: None,
        };

        let resolved = resolve_with_config(&request, &config_with_default_profile());

        assert_eq!(resolved.host, "127.0.0.1");
        assert_eq!(resolved.port, 41_000);
        assert_eq!(resolved.api_key.as_deref(), Some("flag-key"));
    }

    #[test]
    fn unnamed_values_come_from_the_profile() {
        let resolved = resolve_with_config(
            &ConnectionRequest::default(),
            &config_with_default_profile(),
        );

        assert_eq!(resolved.host, "profile.example");
        assert_eq!(resolved.port, 7777);
        assert_eq!(resolved.api_key.as_deref(), Some("profile-key"));
    }

    #[test]
    fn unnamed_values_without_a_profile_use_the_compiled_in_defaults() {
        let request = ConnectionRequest {
            profile: Some("missing".to_owned()),
            ..ConnectionRequest::default()
        };

        let resolved = resolve_with_config(&request, &config_with_default_profile());

        assert_eq!(resolved.profile_name, "missing");
        assert_eq!(resolved.host, DEFAULT_HOST);
        assert_eq!(resolved.port, DEFAULT_PORT);
        assert_eq!(resolved.api_key, None);
    }

    /// The env override is caller-owned and cannot be cleared from a test:
    /// edition 2024 makes `std::env::set_var` unsafe and `unsafe_code` is
    /// forbidden here, so tests of the resolved default skip when it is set.
    fn env_override_active() -> bool {
        std::env::var_os("HYPERCOLOR_CLI_CONFIG").is_some()
    }

    #[test]
    fn unresolvable_config_dir_yields_no_path() {
        assert_eq!(resolve_config_path(None), None);
    }

    #[test]
    fn resolved_path_is_absolute_and_tilde_free() {
        if env_override_active() {
            return;
        }
        let path = config_path();
        assert!(
            path.is_absolute(),
            "resolved {} is not absolute",
            path.display()
        );
        assert!(
            !path.components().any(|part| part.as_os_str() == "~"),
            "resolved {} contains a literal tilde component",
            path.display()
        );
    }

    #[test]
    fn resolved_path_lives_in_the_shared_config_dir() {
        if env_override_active() {
            return;
        }
        assert_eq!(
            config_path(),
            hypercolor_core::config::paths::config_dir().join(CONFIG_FILE_NAME)
        );
    }
}
