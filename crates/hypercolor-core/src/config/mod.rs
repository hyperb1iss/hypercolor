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

use crate::types::config::{
    CURRENT_SCHEMA_VERSION, HypercolorConfig, InteractionRoutePolicy, default_driver_configs,
};

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
    /// Serializes read-modify-write mutations and file writes so concurrent
    /// writers (config API, capture restore-token sink) cannot lose updates
    /// or interleave partial file contents.
    write_lock: std::sync::Mutex<()>,
}

impl ConfigManager {
    /// Creates a new `ConfigManager` by loading configuration from `config_path`.
    ///
    /// If the file does not exist, a default configuration at the current schema
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
            write_lock: std::sync::Mutex::new(()),
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

    /// Whether `snapshot` is still the manager's current immutable value.
    ///
    /// This gives long-running preparation work a cheap compare step before it
    /// commits derived runtime state. Every update installs a distinct `Arc`,
    /// so pointer identity is the snapshot generation.
    #[must_use]
    pub fn is_current(&self, snapshot: &Arc<HypercolorConfig>) -> bool {
        let current = self.config.load();
        Arc::ptr_eq(snapshot, &current)
    }

    /// Replace the live configuration snapshot without re-reading from disk.
    pub fn update(&self, config: HypercolorConfig) {
        let _guard = self
            .write_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.config.store(Arc::new(normalize_config(config)));
    }

    /// Atomically read-modify-write the live configuration.
    ///
    /// Unlike [`update`](Self::update), which replaces the snapshot wholesale
    /// (and can therefore lose a concurrent writer's change), `modify` runs
    /// the closure against the freshest config under the write lock. Use this
    /// for targeted mutations like the capture restore-token sink.
    pub fn modify(&self, mutate: impl FnOnce(&mut HypercolorConfig)) {
        let _guard = self
            .write_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut config = (**self.config.load()).clone();
        mutate(&mut config);
        self.config.store(Arc::new(normalize_config(config)));
    }

    /// Atomically mutate the live configuration only while `expected` is current.
    ///
    /// Preparation can happen without the write lock, then use this compare-and-
    /// modify seam to reject a stale commit without executing `mutate`.
    pub fn modify_if_current(
        &self,
        expected: &Arc<HypercolorConfig>,
        mutate: impl FnOnce(&mut HypercolorConfig),
    ) -> bool {
        let _guard = self
            .write_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let current = self.config.load();
        if !Arc::ptr_eq(expected, &current) {
            return false;
        }
        let mut config = (**current).clone();
        mutate(&mut config);
        self.config.store(Arc::new(normalize_config(config)));
        true
    }

    /// Persist and publish a mutation only while `expected` is current.
    ///
    /// The candidate is written before it becomes visible to lock-free readers,
    /// so a persistence failure leaves both the live snapshot and disk unchanged.
    ///
    /// # Errors
    ///
    /// Returns an error if serialization or the atomic file replacement fails.
    pub fn modify_and_save_if_current(
        &self,
        expected: &Arc<HypercolorConfig>,
        mutate: impl FnOnce(&mut HypercolorConfig),
    ) -> Result<bool> {
        let _guard = self
            .write_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let current = self.config.load();
        if !Arc::ptr_eq(expected, &current) {
            return Ok(false);
        }
        let mut candidate = (**current).clone();
        mutate(&mut candidate);
        let candidate = normalize_config(candidate);
        self.persist(&candidate)?;
        self.config.store(Arc::new(candidate));
        Ok(true)
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

    /// Persist the current live configuration to disk.
    ///
    /// The write goes through a temp file and rename so a concurrent reader
    /// (or a crash mid-write) never observes a torn config file; the write
    /// lock keeps concurrent savers from interleaving.
    ///
    /// # Errors
    ///
    /// Returns an error if serialization fails or the file cannot be written.
    pub fn save(&self) -> Result<()> {
        let _guard = self
            .write_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let snapshot = self.config.load();
        self.persist(&snapshot)
    }

    fn persist(&self, config: &HypercolorConfig) -> Result<()> {
        let toml = toml::to_string_pretty(config).context("failed to serialize config")?;
        if let Some(parent) = self.config_path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
        let tmp_path = self.config_path.with_extension("toml.tmp");
        std::fs::write(&tmp_path, toml)
            .with_context(|| format!("failed to write {}", tmp_path.display()))?;
        std::fs::rename(&tmp_path, &self.config_path)
            .with_context(|| format!("failed to replace {}", self.config_path.display()))
    }

    /// Path backing this manager's configuration file.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.config_path
    }

    /// Returns the platform-appropriate configuration directory.
    ///
    /// Delegates to [`paths::config_dir`].
    pub fn config_dir() -> PathBuf {
        paths::config_dir()
    }

    /// Override the resolved configuration directory.
    ///
    /// This is intended for integration tests that need isolated config state.
    #[doc(hidden)]
    pub fn set_config_dir_override(path: Option<PathBuf>) {
        paths::set_config_dir_override(path);
    }

    /// Returns the platform-appropriate data directory.
    ///
    /// Delegates to [`paths::data_dir`].
    pub fn data_dir() -> PathBuf {
        paths::data_dir()
    }

    /// Override the resolved data directory.
    ///
    /// This is intended for integration tests that need isolated persistence.
    #[doc(hidden)]
    pub fn set_data_dir_override(path: Option<PathBuf>) {
        paths::set_data_dir_override(path);
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
        let document = toml::from_str::<toml::Value>(toml_str)
            .context("failed to parse configuration TOML")?;
        let daemon_route_missing = input_field_missing(&document, "daemon_route");
        let preview_route_missing = input_field_missing(&document, "preview_route");
        let config = document
            .try_into::<HypercolorConfig>()
            .context("failed to parse configuration TOML")?;
        Ok(normalize_config(migrate_config(
            config,
            daemon_route_missing,
            preview_route_missing,
        )))
    }

    /// Returns a default config suitable for first-run.
    fn default_config() -> HypercolorConfig {
        normalize_config(HypercolorConfig::default())
    }
}

fn input_field_missing(document: &toml::Value, field: &str) -> bool {
    document
        .get("input")
        .and_then(toml::Value::as_table)
        .is_none_or(|input| !input.contains_key(field))
}

fn migrate_config(
    mut config: HypercolorConfig,
    daemon_route_missing: bool,
    preview_route_missing: bool,
) -> HypercolorConfig {
    if config.schema_version <= 3 {
        if daemon_route_missing {
            config.input.daemon_route = InteractionRoutePolicy::Merge;
        }
        if preview_route_missing {
            config.input.preview_route = InteractionRoutePolicy::Browser;
        }
        config.schema_version = CURRENT_SCHEMA_VERSION;
    }
    config
}

fn normalize_config(mut config: HypercolorConfig) -> HypercolorConfig {
    config.audio.device = canonical_audio_device_id(&config.audio.device);
    normalize_driver_configs(&mut config);
    config
}

fn normalize_driver_configs(config: &mut HypercolorConfig) {
    let defaults = default_driver_configs();
    for (driver_id, entry) in defaults {
        config.drivers.entry(driver_id).or_insert(entry);
    }
}

pub fn canonical_audio_device_id(device: &str) -> String {
    let trimmed = device.trim();
    if trimmed.eq_ignore_ascii_case("auto") || trimmed.eq_ignore_ascii_case("default") {
        "default".to_owned()
    } else if trimmed.eq_ignore_ascii_case("mic") || trimmed.eq_ignore_ascii_case("microphone") {
        "microphone".to_owned()
    } else if trimmed.eq_ignore_ascii_case("none") {
        "none".to_owned()
    } else {
        trimmed.to_owned()
    }
}
