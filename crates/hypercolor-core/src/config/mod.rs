//! Configuration management -- loading, hot-reloading, and path resolution.
//!
//! [`ConfigManager`] owns the live configuration and provides lock-free reads
//! via [`arc_swap::ArcSwap`]. TOML files are parsed into
//! [`HypercolorConfig`](crate::types::config::HypercolorConfig) from
//! `hypercolor-types`.

pub mod paths;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use arc_swap::{ArcSwap, Guard};
use tracing::{debug, info};

use crate::types::config::HypercolorConfig;

// ─── ConfigManager ──────────────────────────────────────────────────────────

/// Manages the live Hypercolor configuration with lock-free reads and reload.
///
/// Configuration is stored behind an [`ArcSwap`] so readers never block and
/// reloads are atomic. The manager remembers which file it was loaded from
/// to support [`reload`](Self::reload).
pub struct ConfigManager {
    /// Lock-free swappable configuration pointer.
    config: Arc<ArcSwap<HypercolorConfig>>,
    /// Path to the TOML configuration file this manager was loaded from.
    config_path: PathBuf,
}

impl ConfigManager {
    /// Creates a new `ConfigManager` by loading configuration from `config_path`.
    ///
    /// If the file does not exist, a default configuration (schema version 3)
    /// is used instead. Any parse errors are propagated as `Err`.
    ///
    /// # Errors
    ///
    /// Returns an error if the file exists but contains invalid TOML or fails
    /// to deserialize into [`HypercolorConfig`].
    pub fn new(config_path: PathBuf) -> Result<Self> {
        let config = if config_path.exists() {
            info!(path = %config_path.display(), "loading configuration");
            Self::load(&config_path)?
        } else {
            debug!(
                path = %config_path.display(),
                "config file not found, using defaults"
            );
            Self::default_config()
        };

        Ok(Self {
            config: Arc::new(ArcSwap::from_pointee(config)),
            config_path,
        })
    }

    /// Parses a TOML file at `path` into a [`HypercolorConfig`].
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be read or the TOML is malformed.
    pub fn load(path: &Path) -> Result<HypercolorConfig> {
        let contents = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read config from {}", path.display()))?;

        Self::parse_toml(&contents)
    }

    /// Returns a snapshot of the current configuration.
    ///
    /// This is a lock-free operation backed by `arc_swap`. The returned guard
    /// dereferences to `Arc<HypercolorConfig>` and is cheap to hold.
    pub fn get(&self) -> Guard<Arc<HypercolorConfig>> {
        self.config.load()
    }

    /// Reloads configuration from the original file path.
    ///
    /// On success, atomically swaps the live config. On failure, the previous
    /// config remains active and the error is returned.
    ///
    /// # Errors
    ///
    /// Returns an error if the config file cannot be read or parsed.
    pub fn reload(&self) -> Result<()> {
        info!(path = %self.config_path.display(), "reloading configuration");
        let new_config = Self::load(&self.config_path)?;
        self.config.store(Arc::new(new_config));
        info!("configuration reloaded successfully");
        Ok(())
    }

    /// Returns the platform-appropriate configuration directory.
    ///
    /// Delegates to [`paths::config_dir`].
    pub fn config_dir() -> PathBuf {
        paths::config_dir()
    }

    /// Returns the platform-appropriate data directory.
    ///
    /// Delegates to [`paths::data_dir`].
    pub fn data_dir() -> PathBuf {
        paths::data_dir()
    }

    /// Returns the platform-appropriate cache directory.
    ///
    /// Delegates to [`paths::cache_dir`].
    pub fn cache_dir() -> PathBuf {
        paths::cache_dir()
    }

    // ── Internal helpers ────────────────────────────────────────────────────

    /// Parses a TOML string into a [`HypercolorConfig`].
    fn parse_toml(toml_str: &str) -> Result<HypercolorConfig> {
        toml::from_str(toml_str).context("failed to parse configuration TOML")
    }

    /// Returns a default config suitable for first-run.
    fn default_config() -> HypercolorConfig {
        HypercolorConfig::default()
    }
}
