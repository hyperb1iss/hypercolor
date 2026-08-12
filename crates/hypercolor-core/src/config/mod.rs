//! Configuration management -- loading, hot-reloading, and path resolution.
//!
//! [`ConfigManager`] owns the live configuration and provides lock-free reads
//! via [`arc_swap::ArcSwap`]. TOML files are parsed into
//! [`HypercolorConfig`](crate::types::config::HypercolorConfig) from
//! `hypercolor-types`.

pub mod paths;

use std::collections::BTreeSet;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use arc_swap::{ArcSwap, Guard};
use tracing::{debug, info};

use crate::types::config::{
    CURRENT_SCHEMA_VERSION, CaptureConfig, HypercolorConfig, InteractionRoutePolicy,
    default_driver_configs,
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
    write_lock: std::sync::Mutex<ConfigWriterState>,
    #[cfg(test)]
    persistence_fault: std::sync::Mutex<Option<ConfigPersistenceStage>>,
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ConfigPersistenceStage {
    Serialize,
    CreateParent,
    CreateTemporary,
    WriteTemporary,
    SyncTemporary,
    Replace,
    SyncDirectory,
    AfterDurableWrite,
}

#[derive(Default)]
struct ConfigWriterState {
    next_capture_persistence_epoch: u64,
    staged_capture_persistence_epochs: BTreeSet<u64>,
    capture_persistence_authority: Option<CapturePersistenceAuthority>,
    applied_capture: Option<CaptureConfig>,
}

struct CapturePersistenceAuthority {
    epoch: CapturePersistenceEpoch,
    config: Arc<HypercolorConfig>,
    source: Option<CapturePersistenceSource>,
}

/// Opaque authority reserved for one prepared capture-source lifetime.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct CapturePersistenceEpoch(u64);

/// Generation-fenced identity of the source session publishing capture metadata.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CapturePersistenceSource {
    source_id: Arc<str>,
    source_graph_generation: u64,
    session_generation: u64,
}

impl CapturePersistenceSource {
    /// Build an identity from one canonical input-source status snapshot.
    #[must_use]
    pub fn new(source_id: Arc<str>, source_graph_generation: u64, session_generation: u64) -> Self {
        Self {
            source_id,
            source_graph_generation,
            session_generation,
        }
    }
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
            write_lock: std::sync::Mutex::new(ConfigWriterState::default()),
            #[cfg(test)]
            persistence_fault: std::sync::Mutex::new(None),
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
        let mut writer = self
            .write_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let current = self.config.load_full();
        self.publish_config(&mut writer, &current, Arc::new(normalize_config(config)));
    }

    /// Atomically read-modify-write the live configuration.
    ///
    /// Unlike [`update`](Self::update), which replaces the snapshot wholesale
    /// (and can therefore lose a concurrent writer's change), `modify` runs
    /// the closure against the freshest config under the write lock. Use this
    /// for targeted mutations like the capture restore-token sink.
    pub fn modify(&self, mutate: impl FnOnce(&mut HypercolorConfig)) {
        let mut writer = self
            .write_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let current = self.config.load_full();
        let mut config = (*current).clone();
        mutate(&mut config);
        self.publish_config(&mut writer, &current, Arc::new(normalize_config(config)));
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
        let mut writer = self
            .write_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let current = self.config.load_full();
        if !Arc::ptr_eq(expected, &current) {
            return false;
        }
        let mut config = (*current).clone();
        mutate(&mut config);
        self.publish_config(&mut writer, &current, Arc::new(normalize_config(config)));
        true
    }

    /// Persist and publish a mutation only while `expected` is current.
    ///
    /// The candidate reaches the native durability boundary before it becomes
    /// visible to lock-free readers. A failure never publishes the candidate.
    /// Replacement uncertainty is safe to replay because the file is either
    /// the previous complete snapshot or the complete candidate.
    ///
    /// # Errors
    ///
    /// Returns an error if serialization, file replacement, or durable sync fails.
    pub fn modify_and_save_if_current(
        &self,
        expected: &Arc<HypercolorConfig>,
        mutate: impl FnOnce(&mut HypercolorConfig),
    ) -> Result<bool> {
        self.modify_and_save_if_current_snapshot(expected, mutate)
            .map(|installed| installed.is_some())
    }

    /// Persist and publish a mutation only while `expected` is current, returning
    /// the exact immutable snapshot installed by the successful mutation.
    ///
    /// # Errors
    ///
    /// Returns an error if serialization, file replacement, or durable sync fails.
    pub fn modify_and_save_if_current_snapshot(
        &self,
        expected: &Arc<HypercolorConfig>,
        mutate: impl FnOnce(&mut HypercolorConfig),
    ) -> Result<Option<Arc<HypercolorConfig>>> {
        let mut writer = self
            .write_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let current = self.config.load_full();
        if !Arc::ptr_eq(expected, &current) {
            return Ok(None);
        }
        let mut candidate = (*current).clone();
        mutate(&mut candidate);
        let candidate = Arc::new(normalize_config(candidate));
        self.persist(&candidate)?;
        #[cfg(test)]
        self.persistence_checkpoint(ConfigPersistenceStage::AfterDurableWrite)?;
        self.publish_config(&mut writer, &current, Arc::clone(&candidate));
        Ok(Some(candidate))
    }

    /// Reserve a non-serialized persistence epoch for a prepared capture source.
    #[must_use]
    pub fn reserve_capture_persistence(
        &self,
        expected: &Arc<HypercolorConfig>,
    ) -> Option<CapturePersistenceEpoch> {
        let mut writer = self
            .write_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let current = self.config.load_full();
        if !Arc::ptr_eq(expected, &current) {
            return None;
        }
        writer.next_capture_persistence_epoch = writer
            .next_capture_persistence_epoch
            .checked_add(1)
            .expect("capture persistence epoch exhausted");
        let epoch = CapturePersistenceEpoch(writer.next_capture_persistence_epoch);
        writer.staged_capture_persistence_epochs.insert(epoch.0);
        Some(epoch)
    }

    /// Activate a reserved epoch without changing the serialized configuration.
    pub fn activate_capture_persistence(
        &self,
        expected: &Arc<HypercolorConfig>,
        epoch: CapturePersistenceEpoch,
        source: Option<CapturePersistenceSource>,
    ) -> bool {
        let mut writer = self
            .write_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let current = self.config.load_full();
        if !Arc::ptr_eq(expected, &current)
            || !writer.staged_capture_persistence_epochs.remove(&epoch.0)
        {
            return false;
        }
        writer.capture_persistence_authority = Some(CapturePersistenceAuthority {
            epoch,
            config: current,
            source,
        });
        true
    }

    /// Persist a capture config and activate its prepared source in one writer epoch.
    ///
    /// # Errors
    ///
    /// Returns an error if serialization or the atomic file replacement fails.
    pub fn save_capture_and_activate_if_current(
        &self,
        expected: &Arc<HypercolorConfig>,
        epoch: CapturePersistenceEpoch,
        source: Option<CapturePersistenceSource>,
        capture: CaptureConfig,
    ) -> Result<Option<Arc<HypercolorConfig>>> {
        let mut writer = self
            .write_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let current = self.config.load_full();
        if !Arc::ptr_eq(expected, &current)
            || !writer.staged_capture_persistence_epochs.remove(&epoch.0)
        {
            return Ok(None);
        }
        let mut candidate = (*current).clone();
        candidate.capture = capture;
        let candidate = Arc::new(normalize_config(candidate));
        if let Err(error) = self.persist(&candidate) {
            return Err(error);
        }
        writer.capture_persistence_authority = Some(CapturePersistenceAuthority {
            epoch,
            config: Arc::clone(&candidate),
            source,
        });
        writer.staged_capture_persistence_epochs.clear();
        self.config.store(Arc::clone(&candidate));
        Ok(Some(candidate))
    }

    /// Persist a source-resolved capture identity only for the active source epoch.
    ///
    /// # Errors
    ///
    /// Returns an error if serialization or the atomic file replacement fails.
    pub fn modify_capture_if_authorized(
        &self,
        epoch: CapturePersistenceEpoch,
        source: CapturePersistenceSource,
        mutate: impl FnOnce(&mut CaptureConfig),
    ) -> Result<Option<Arc<HypercolorConfig>>> {
        let mut writer = self
            .write_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let current = self.config.load_full();
        let Some(authority) = writer.capture_persistence_authority.as_ref() else {
            return Ok(None);
        };
        if authority.epoch != epoch
            || !Arc::ptr_eq(&authority.config, &current)
            || authority
                .source
                .as_ref()
                .is_some_and(|active| active != &source)
        {
            return Ok(None);
        }
        let mut candidate = (*current).clone();
        mutate(&mut candidate.capture);
        let candidate = Arc::new(normalize_config(candidate));
        self.persist(&candidate)?;
        let authority = writer
            .capture_persistence_authority
            .as_mut()
            .expect("capture authority was validated under the writer lock");
        authority.config = Arc::clone(&candidate);
        authority.source.get_or_insert(source);
        writer.applied_capture = Some(candidate.capture.clone());
        self.config.store(Arc::clone(&candidate));
        Ok(Some(candidate))
    }

    /// Mutate capture config while the persistence epoch is current, without
    /// the pinned-source check.
    ///
    /// Restore tokens serialize through the capture worker's session-epoch
    /// guard, and their session generations legitimately advance across
    /// in-worker reconnects, so a first-persist source pin would reject
    /// every rotation after the first and strand consumed tokens on disk.
    ///
    /// # Errors
    ///
    /// Returns the underlying persistence error when writing the candidate
    /// config fails.
    pub fn modify_capture_if_epoch_current(
        &self,
        epoch: CapturePersistenceEpoch,
        mutate: impl FnOnce(&mut CaptureConfig),
    ) -> Result<Option<Arc<HypercolorConfig>>> {
        let mut writer = self
            .write_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let current = self.config.load_full();
        let Some(authority) = writer.capture_persistence_authority.as_ref() else {
            return Ok(None);
        };
        if authority.epoch != epoch || !Arc::ptr_eq(&authority.config, &current) {
            return Ok(None);
        }
        let mut candidate = (*current).clone();
        mutate(&mut candidate.capture);
        let candidate = Arc::new(normalize_config(candidate));
        self.persist(&candidate)?;
        let authority = writer
            .capture_persistence_authority
            .as_mut()
            .expect("capture authority was validated under the writer lock");
        authority.config = Arc::clone(&candidate);
        writer.applied_capture = Some(candidate.capture.clone());
        self.config.store(Arc::clone(&candidate));
        Ok(Some(candidate))
    }

    /// Revoke a staged or active capture persistence epoch.
    pub fn revoke_capture_persistence(&self, epoch: CapturePersistenceEpoch) {
        let mut writer = self
            .write_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        writer.staged_capture_persistence_epochs.remove(&epoch.0);
        if writer
            .capture_persistence_authority
            .as_ref()
            .is_some_and(|authority| authority.epoch == epoch)
        {
            writer.capture_persistence_authority = None;
        }
    }

    /// Record the capture config represented by the installed runtime source graph.
    pub fn mark_capture_runtime_applied(&self, capture: &CaptureConfig) {
        let mut writer = self
            .write_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        writer.applied_capture = Some(capture.clone());
    }

    /// Forget which capture config the installed runtime source graph represents.
    pub fn invalidate_capture_runtime_applied(&self) {
        let mut writer = self
            .write_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        writer.applied_capture = None;
    }

    /// Whether the installed capture runtime was built from this exact config.
    #[must_use]
    pub fn capture_runtime_matches(&self, capture: &CaptureConfig) -> bool {
        let writer = self
            .write_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        writer.applied_capture.as_ref() == Some(capture)
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
        let mut writer = self
            .write_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        info!(path = %self.config_path.display(), "reloading configuration");
        let new_config = Arc::new(normalize_config(Self::load(&self.config_path)?));
        let current = self.config.load_full();
        self.publish_config(&mut writer, &current, new_config);
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
        let _writer = self
            .write_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let snapshot = self.config.load();
        self.persist(&snapshot)
    }

    fn persist(&self, config: &HypercolorConfig) -> Result<()> {
        #[cfg(test)]
        self.persistence_checkpoint(ConfigPersistenceStage::Serialize)?;
        let toml = toml::to_string_pretty(config).context("failed to serialize config")?;
        if let Some(parent) = self.config_path.parent() {
            #[cfg(test)]
            self.persistence_checkpoint(ConfigPersistenceStage::CreateParent)?;
            std::fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
        let tmp_path = self.config_path.with_extension("toml.tmp");
        #[cfg(test)]
        self.persistence_checkpoint(ConfigPersistenceStage::CreateTemporary)?;
        let mut temporary = std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&tmp_path)
            .with_context(|| format!("failed to create {}", tmp_path.display()))?;
        #[cfg(test)]
        self.persistence_checkpoint(ConfigPersistenceStage::WriteTemporary)?;
        temporary
            .write_all(toml.as_bytes())
            .with_context(|| format!("failed to write {}", tmp_path.display()))?;
        #[cfg(test)]
        self.persistence_checkpoint(ConfigPersistenceStage::SyncTemporary)?;
        temporary
            .sync_all()
            .with_context(|| format!("failed to sync {}", tmp_path.display()))?;
        drop(temporary);
        #[cfg(test)]
        self.persistence_checkpoint(ConfigPersistenceStage::Replace)?;
        hypercolor_platform_fs::replace_file(&tmp_path, &self.config_path)
            .with_context(|| format!("failed to replace {}", self.config_path.display()))?;
        #[cfg(test)]
        self.persistence_checkpoint(ConfigPersistenceStage::SyncDirectory)?;
        sync_parent_directory(&self.config_path)
    }

    #[cfg(test)]
    fn persistence_checkpoint(&self, stage: ConfigPersistenceStage) -> Result<()> {
        let mut armed = self
            .persistence_fault
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if *armed == Some(stage) {
            *armed = None;
            anyhow::bail!("injected config persistence failure at {stage:?}");
        }
        Ok(())
    }

    #[cfg(test)]
    fn fail_next_persistence_at(&self, stage: ConfigPersistenceStage) {
        *self
            .persistence_fault
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(stage);
    }

    fn publish_config(
        &self,
        writer: &mut ConfigWriterState,
        current: &Arc<HypercolorConfig>,
        candidate: Arc<HypercolorConfig>,
    ) {
        writer.staged_capture_persistence_epochs.clear();
        if current.capture == candidate.capture {
            if let Some(authority) = writer.capture_persistence_authority.as_mut() {
                authority.config = Arc::clone(&candidate);
            }
        } else {
            writer.capture_persistence_authority = None;
        }
        self.config.store(candidate);
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

#[cfg_attr(target_os = "windows", allow(clippy::unnecessary_wraps))]
fn sync_parent_directory(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        let parent = parent_directory(path);
        std::fs::File::open(parent)
            .with_context(|| format!("failed to open {} for sync", parent.display()))?
            .sync_all()
            .with_context(|| format!("failed to sync {}", parent.display()))
    }

    #[cfg(target_os = "windows")]
    {
        // The Windows replacement primitive itself requests write-through.
        let _ = path;
        Ok(())
    }

    #[cfg(not(any(unix, target_os = "windows")))]
    anyhow::bail!("durable config replacement is unsupported on this platform")
}

#[cfg(any(unix, test))]
fn parent_directory(path: &Path) -> &Path {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
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

#[cfg(test)]
mod persistence_tests {
    use super::*;

    const UPDATED_PORT: u16 = 17_777;

    fn manager_with_persisted_default(dir: &Path, name: &str) -> (ConfigManager, PathBuf) {
        let path = dir.join(name).join("config.toml");
        let manager = ConfigManager::new(path.clone()).expect("config manager");
        manager.save().expect("persist default");
        (manager, path)
    }

    #[test]
    fn bare_config_path_syncs_the_current_directory() {
        assert_eq!(parent_directory(Path::new("config.toml")), Path::new("."));
        assert_eq!(
            parent_directory(Path::new("nested/config.toml")),
            Path::new("nested")
        );
    }

    #[test]
    fn persistence_faults_before_replace_change_neither_disk_nor_live_snapshot() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let stages = [
            ConfigPersistenceStage::Serialize,
            ConfigPersistenceStage::CreateParent,
            ConfigPersistenceStage::CreateTemporary,
            ConfigPersistenceStage::WriteTemporary,
            ConfigPersistenceStage::SyncTemporary,
            ConfigPersistenceStage::Replace,
        ];

        for stage in stages {
            let (manager, path) =
                manager_with_persisted_default(dir.as_ref(), &format!("{stage:?}"));
            let expected = manager.get().clone();
            let original_port = expected.daemon.port;
            manager.fail_next_persistence_at(stage);

            let result = manager.modify_and_save_if_current_snapshot(&expected, |config| {
                config.daemon.port = UPDATED_PORT;
            });

            assert!(result.is_err(), "{stage:?} must fail");
            assert_eq!(manager.get().daemon.port, original_port, "{stage:?} live");
            assert_eq!(
                ConfigManager::load(&path)
                    .expect("persisted config")
                    .daemon
                    .port,
                original_port,
                "{stage:?} disk"
            );
            assert!(
                manager
                    .modify_and_save_if_current_snapshot(&expected, |config| {
                        config.daemon.port = UPDATED_PORT;
                    })
                    .expect("retry")
                    .is_some(),
                "{stage:?} retry"
            );
        }
    }

    #[test]
    fn post_replace_failures_keep_memory_unpublished_and_visible_file_parseable() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let stages = [
            ConfigPersistenceStage::SyncDirectory,
            ConfigPersistenceStage::AfterDurableWrite,
        ];

        for stage in stages {
            let (manager, path) =
                manager_with_persisted_default(dir.as_ref(), &format!("{stage:?}"));
            let expected = manager.get().clone();
            let original_port = expected.daemon.port;
            manager.fail_next_persistence_at(stage);

            let result = manager.modify_and_save_if_current_snapshot(&expected, |config| {
                config.daemon.port = UPDATED_PORT;
            });

            assert!(result.is_err(), "{stage:?} must fail");
            assert_eq!(manager.get().daemon.port, original_port, "{stage:?} live");
            assert_eq!(
                ConfigManager::new(path.clone())
                    .expect("reopen visible replacement")
                    .get()
                    .daemon
                    .port,
                UPDATED_PORT,
                "{stage:?} reopened disk"
            );
            assert!(
                manager
                    .modify_and_save_if_current_snapshot(&expected, |config| {
                        config.daemon.port = UPDATED_PORT;
                    })
                    .expect("same-process retry")
                    .is_some(),
                "{stage:?} retry"
            );
            assert_eq!(manager.get().daemon.port, UPDATED_PORT, "{stage:?} live");
        }
    }
}
