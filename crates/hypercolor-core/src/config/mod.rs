//! Configuration management -- loading, hot-reloading, and path resolution.
//!
//! [`ConfigManager`] owns the live configuration and provides lock-free reads
//! via [`arc_swap::ArcSwap`]. TOML files are parsed into
//! [`HypercolorConfig`](hypercolor_types::config::HypercolorConfig) from
//! `hypercolor-types`.

mod change_stream;
pub mod paths;
pub mod servers;
pub mod sources;

pub use sources::{
    BootConfig, CliOverrides, ConfigProvenance, ConfigSources, EnvOverrides, LoadedConfig,
    SourceLayer,
};

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result, bail};
use arc_swap::{ArcSwap, ArcSwapOption, Guard};
use tracing::{debug, info, warn};

use crate::bus::HypercolorBus;
use crate::persistence::{AtomicFileWriter, AtomicWriteCommitResult};
use hypercolor_types::config::{
    CURRENT_SCHEMA_VERSION, CaptureConfig, HypercolorConfig, default_driver_configs,
};

// ─── ConfigManager ──────────────────────────────────────────────────────────

/// A borrowed live-config snapshot: cheap to take, plain to hold.
///
/// Dereferences to [`HypercolorConfig`]. Holding one across an await
/// pins the snapshot, not a lock — writers are never blocked. Storing
/// a clone of the inner config in a long-lived struct is the
/// anti-pattern this type exists to discourage (Spec 76 §3.2).
pub struct LiveConfigSnapshot(Guard<Arc<HypercolorConfig>>);

impl LiveConfigSnapshot {
    /// An owned copy of the snapshot's config, for mutation staging.
    #[must_use]
    pub fn clone_inner(&self) -> HypercolorConfig {
        (**self.0).clone()
    }
}

impl std::ops::Deref for LiveConfigSnapshot {
    type Target = HypercolorConfig;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

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
    /// Process-wide persistence coordinator for the config destination.
    persistence: AtomicFileWriter,
    /// Serializes read-modify-write mutations and file writes so concurrent
    /// writers (config API, capture restore-token sink) cannot lose updates
    /// or interleave partial file contents.
    write_lock: std::sync::Mutex<ConfigWriterState>,
    /// Boot-time fingerprint and sticky overlays for restart
    /// reporting. Set exactly once by `load_with_sources`; managers
    /// built without a boot baseline report nothing pending.
    boot_state: std::sync::OnceLock<sources::BootState>,
    /// Bus the persisted-change stream publishes on, once the daemon
    /// attaches it. Managers without one persist silently.
    change_stream: ArcSwapOption<HypercolorBus>,
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
}

#[derive(Default)]
struct ConfigWriterState {
    next_capture_persistence_epoch: u64,
    staged_capture_persistence_epochs: BTreeSet<u64>,
    capture_persistence_authority: Option<CapturePersistenceAuthority>,
    applied_capture: Option<CaptureConfig>,
    /// The document the change stream last published against: the
    /// baseline the next persisted document is diffed from.
    published: Option<Arc<HypercolorConfig>>,
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

/// Serialized capture configuration staged durably beside its destination.
#[must_use = "staged capture configuration must be committed or discarded"]
pub struct StagedCaptureConfig {
    candidate: Arc<HypercolorConfig>,
    file: StagedConfigFile,
}

struct StagedConfigFile {
    path: Option<PathBuf>,
}

impl StagedConfigFile {
    fn path(&self) -> &Path {
        self.path
            .as_deref()
            .expect("staged config file consumed once")
    }

    fn durable_replace(mut self, destination: &Path) -> Result<()> {
        hypercolor_platform_fs::durable_replace(self.path(), destination)
            .with_context(|| format!("failed to replace {}", destination.display()))?;
        self.path = None;
        Ok(())
    }
}

impl Drop for StagedConfigFile {
    fn drop(&mut self) {
        if let Some(path) = self.path.take() {
            drop(std::fs::remove_file(path));
        }
    }
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

        let persistence = AtomicFileWriter::new(&config_path)
            .with_context(|| format!("failed to prepare {}", config_path.display()))?;
        Ok(Self {
            config: Arc::new(ArcSwap::from_pointee(config)),
            config_path,
            persistence,
            write_lock: std::sync::Mutex::new(ConfigWriterState::default()),
            boot_state: std::sync::OnceLock::new(),
            change_stream: ArcSwapOption::empty(),
            #[cfg(test)]
            persistence_fault: std::sync::Mutex::new(None),
        })
    }

    /// Build a manager over a config the caller already materialized,
    /// running the same normalize every load runs.
    ///
    /// Everything else the pipeline in
    /// [`load_with_sources`](Self::load_with_sources) does is skipped:
    /// the daemon's driver-seeding hook never runs, capture config is
    /// never validated, no env or CLI overlay is applied, and no
    /// provenance or boot fingerprint is recorded — so a manager built
    /// this way reports no pending restarts. It exists for callers that
    /// already own a fully materialized config, which in practice means
    /// tests driving daemon initialization directly.
    #[doc(hidden)]
    #[must_use]
    pub fn from_config_unchecked(config_path: PathBuf, config: HypercolorConfig) -> Self {
        Self::with_config(config_path, normalize_config(config))
            .expect("unchecked config path should support atomic persistence")
    }

    /// Build a manager over an already-normalized config (the
    /// `load_with_sources` pipeline owns parse/overlay/validate).
    pub(super) fn with_config(config_path: PathBuf, config: HypercolorConfig) -> Result<Self> {
        let persistence = AtomicFileWriter::new(&config_path)
            .with_context(|| format!("failed to prepare {}", config_path.display()))?;
        Ok(Self {
            config: Arc::new(ArcSwap::from_pointee(config)),
            config_path,
            persistence,
            write_lock: std::sync::Mutex::new(ConfigWriterState::default()),
            boot_state: std::sync::OnceLock::new(),
            change_stream: ArcSwapOption::empty(),
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
            .with_context(|| format!("failed to load config from {}", path.display()))
    }

    /// Returns a snapshot of the current configuration.
    ///
    /// This is a lock-free operation backed by `arc_swap`. The returned guard
    /// dereferences to `Arc<HypercolorConfig>` and is cheap to hold.
    pub fn get(&self) -> Guard<Arc<HypercolorConfig>> {
        self.config.load()
    }

    /// A live snapshot behind a plain `Deref` — the public read
    /// surface (Spec 76 §3.2). The swap machinery never appears in
    /// the signature, so callers cannot depend on it.
    pub fn live(&self) -> LiveConfigSnapshot {
        LiveConfigSnapshot(self.config.load())
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
        self.publish_config(&mut writer, &current, Arc::clone(&candidate));
        self.note_persisted(&mut writer, &candidate);
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
        self.note_persisted(&mut writer, &candidate);
        Ok(Some(candidate))
    }

    /// Serialize and sync a capture candidate without holding the config writer lock.
    ///
    /// # Errors
    ///
    /// Returns an error if serialization or durable temporary-file creation fails.
    pub fn stage_capture_config(
        &self,
        expected: &Arc<HypercolorConfig>,
        capture: CaptureConfig,
    ) -> Result<Option<StagedCaptureConfig>> {
        if !self.is_current(expected) {
            return Ok(None);
        }
        let mut candidate = (**expected).clone();
        candidate.capture = capture;
        let candidate = Arc::new(normalize_config(candidate));
        let file = self.stage_config(&candidate)?;
        Ok(Some(StagedCaptureConfig { candidate, file }))
    }

    /// Durably replace capture config, commit runtime, then install the live pointer.
    ///
    /// `durable_replace` is the sole fallible operation after every identity check.
    /// Runtime mutation, authority installation, and live publication are infallible.
    ///
    /// # Errors
    ///
    /// Returns an error if the atomic durable replacement fails.
    pub fn commit_staged_capture_if_current<T>(
        &self,
        expected: &Arc<HypercolorConfig>,
        mut persistence: Option<(CapturePersistenceEpoch, Option<CapturePersistenceSource>)>,
        staged: StagedCaptureConfig,
        commit_runtime: impl FnOnce(&mut dyn FnMut()) -> T,
    ) -> Result<Option<(Arc<HypercolorConfig>, T)>> {
        let mut writer = self
            .write_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let current = self.config.load_full();
        if !Arc::ptr_eq(expected, &current) {
            return Ok(None);
        }
        if let Some((epoch, _)) = &persistence
            && !writer.staged_capture_persistence_epochs.remove(&epoch.0)
        {
            return Ok(None);
        }
        #[cfg(test)]
        self.persistence_checkpoint(ConfigPersistenceStage::Replace)?;
        staged.file.durable_replace(&self.config_path)?;
        let candidate = Arc::clone(&staged.candidate);
        let mut installed = false;
        let committed = {
            let mut install_live = || {
                assert!(!installed, "staged capture config installed more than once");
                installed = true;
                writer.capture_persistence_authority =
                    persistence
                        .take()
                        .map(|(epoch, source)| CapturePersistenceAuthority {
                            epoch,
                            config: Arc::clone(&candidate),
                            source,
                        });
                writer.staged_capture_persistence_epochs.clear();
                writer.applied_capture = Some(candidate.capture.clone());
                self.config.store(Arc::clone(&candidate));
            };
            commit_runtime(&mut install_live)
        };
        assert!(
            installed,
            "runtime commit did not install staged capture config"
        );
        self.note_persisted(&mut writer, &candidate);
        Ok(Some((candidate, committed)))
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
        self.note_persisted(&mut writer, &candidate);
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
        self.note_persisted(&mut writer, &candidate);
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
        self.publish_config(&mut writer, &current, Arc::clone(&new_config));
        self.note_persisted(&mut writer, &new_config);
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
        let mut writer = self
            .write_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let snapshot = self.config.load_full();
        self.persist(&snapshot)?;
        self.note_persisted(&mut writer, &snapshot);
        Ok(())
    }

    /// Publish every persisted config change on `bus`.
    ///
    /// The live document at attach time becomes the diff baseline, so
    /// the first event describes the first write after attach, and a
    /// later attach replaces the bus and resets that baseline. Each
    /// persisted document then fans out as exactly one
    /// [`HypercolorEvent::ConfigChanged`](hypercolor_types::event::HypercolorEvent::ConfigChanged),
    /// whichever path wrote it; see [`change_stream`] for the payload
    /// rules.
    pub fn attach_change_stream(&self, bus: Arc<HypercolorBus>) {
        let mut writer = self
            .write_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        writer.published = Some(self.config.load_full());
        self.change_stream.store(Some(bus));
    }

    /// Record that `persisted` is now the on-disk document and publish
    /// the change it represents.
    ///
    /// Every persisting path calls this under the writer lock after the
    /// live pointer is installed, so a subscriber that reads the manager
    /// on receipt already sees the document the event describes. The
    /// soft persistence failures `persist` reports as `Ok` count too:
    /// the mutation is authoritative and the flush registry retries the
    /// file.
    fn note_persisted(&self, writer: &mut ConfigWriterState, persisted: &Arc<HypercolorConfig>) {
        let Some(bus) = self.change_stream.load_full() else {
            return;
        };
        let Some(previous) = writer.published.replace(Arc::clone(persisted)) else {
            return;
        };
        if Arc::ptr_eq(&previous, persisted) {
            return;
        }
        if let Some(event) = change_stream::config_changed_event(&previous, persisted) {
            bus.publish(event);
        }
    }

    fn persist(&self, config: &HypercolorConfig) -> Result<()> {
        #[cfg(test)]
        self.persistence_checkpoint(ConfigPersistenceStage::Serialize)?;
        let toml = toml::to_string_pretty(config).context("failed to serialize config")?;
        match self
            .persistence
            .reserve()
            .admit(toml.into_bytes())
            .commit_stage_aware()
        {
            AtomicWriteCommitResult::DurableWritten => Ok(()),
            AtomicWriteCommitResult::Superseded => {
                bail!("config persistence was superseded before publication")
            }
            AtomicWriteCommitResult::FailedBeforeReplacement(error)
            | AtomicWriteCommitResult::ReplacementVisibleButNotDurable(error) => {
                warn!(
                    path = %self.config_path.display(),
                    %error,
                    "Config mutation is authoritative and persistence will retry"
                );
                Ok(())
            }
        }
    }

    fn stage_config(&self, config: &HypercolorConfig) -> Result<StagedConfigFile> {
        static NEXT_TEMPORARY_ID: AtomicU64 = AtomicU64::new(0);

        #[cfg(test)]
        self.persistence_checkpoint(ConfigPersistenceStage::Serialize)?;
        let toml = toml::to_string_pretty(config).context("failed to serialize config")?;
        if let Some(parent) = self.config_path.parent() {
            #[cfg(test)]
            self.persistence_checkpoint(ConfigPersistenceStage::CreateParent)?;
            std::fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
        let file_name = self
            .config_path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("hypercolor.toml");
        #[cfg(test)]
        self.persistence_checkpoint(ConfigPersistenceStage::CreateTemporary)?;
        #[cfg(test)]
        self.persistence_checkpoint(ConfigPersistenceStage::WriteTemporary)?;
        #[cfg(test)]
        self.persistence_checkpoint(ConfigPersistenceStage::SyncTemporary)?;
        loop {
            let temporary_id = NEXT_TEMPORARY_ID.fetch_add(1, Ordering::Relaxed);
            let tmp_path = self.config_path.with_file_name(format!(
                ".{file_name}.{}.{}.tmp",
                std::process::id(),
                temporary_id
            ));
            match hypercolor_platform_fs::write_secret(&tmp_path, toml.as_bytes()) {
                Ok(()) => {
                    return Ok(StagedConfigFile {
                        path: Some(tmp_path),
                    });
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => {
                    return Err(error)
                        .with_context(|| format!("failed to write {}", tmp_path.display()));
                }
            }
        }
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

    /// Returns the platform-appropriate machine-local state directory.
    ///
    /// Delegates to [`paths::state_dir`].
    pub fn state_dir() -> PathBuf {
        paths::state_dir()
    }

    /// Override the resolved machine-local state directory.
    ///
    /// This is intended for integration tests that need isolated daemon state.
    #[doc(hidden)]
    pub fn set_state_dir_override(path: Option<PathBuf>) {
        paths::set_state_dir_override(path);
    }

    /// Returns the platform-appropriate cache directory.
    ///
    /// Delegates to [`paths::cache_dir`].
    pub fn cache_dir() -> PathBuf {
        paths::cache_dir()
    }

    /// Parses a TOML string into a [`HypercolorConfig`].
    ///
    /// This is THE config parser: file loads, tooling, and tests all run
    /// the same parse and normalize, so no caller can materialize a
    /// config that skips them. Schema v4 is upgraded in memory by
    /// renaming `daemon.start_profile` to `daemon.start_scene` and the
    /// effect fallback value `clear_groups` to `clear_zones`; every other
    /// older or newer schema is refused rather than guessed at.
    ///
    /// # Errors
    ///
    /// Returns an error if the TOML is malformed, does not deserialize
    /// into [`HypercolorConfig`], declares conflicting v4 start keys, or
    /// declares any unsupported schema version.
    pub fn parse_toml(toml_str: &str) -> Result<HypercolorConfig> {
        let mut document = toml::from_str::<toml::Value>(toml_str)
            .context("failed to parse configuration TOML")?;
        let schema_version = document
            .get("schema_version")
            .and_then(toml::Value::as_integer)
            .and_then(|version| u32::try_from(version).ok())
            .ok_or_else(|| {
                anyhow::anyhow!("config schema_version must be a non-negative integer")
            })?;
        if schema_version == 4 {
            migrate_v4_to_v5(&mut document)?;
        } else if schema_version != CURRENT_SCHEMA_VERSION {
            bail!(schema_mismatch_message(schema_version));
        }
        let config: HypercolorConfig = document
            .try_into()
            .context("failed to deserialize configuration TOML")?;
        Ok(normalize_config(config))
    }

    /// Returns a default config suitable for first-run.
    ///
    /// Normalized like every other materialized config, so a daemon that
    /// never found a file behaves exactly like one that loaded a file of
    /// pure defaults.
    #[must_use]
    pub fn default_config() -> HypercolorConfig {
        normalize_config(HypercolorConfig::default())
    }
}

fn migrate_v4_to_v5(document: &mut toml::Value) -> Result<()> {
    let root = document
        .as_table_mut()
        .ok_or_else(|| anyhow::anyhow!("configuration root must be a TOML table"))?;
    if let Some(daemon) = root.get_mut("daemon").and_then(toml::Value::as_table_mut) {
        if daemon.contains_key("start_profile") && daemon.contains_key("start_scene") {
            bail!(
                "schema v4 config declares both daemon.start_profile and \
                 daemon.start_scene; keep only start_profile before migration"
            );
        }
        if let Some(start_scene) = daemon.remove("start_profile") {
            daemon.insert("start_scene".to_owned(), start_scene);
        }
    }
    if let Some(effect_engine) = root
        .get_mut("effect_engine")
        .and_then(toml::Value::as_table_mut)
        && effect_engine
            .get("effect_error_fallback")
            .and_then(toml::Value::as_str)
            == Some("clear_groups")
    {
        effect_engine.insert(
            "effect_error_fallback".to_owned(),
            toml::Value::String("clear_zones".to_owned()),
        );
    }
    root.insert(
        "schema_version".to_owned(),
        toml::Value::Integer(i64::from(CURRENT_SCHEMA_VERSION)),
    );
    Ok(())
}

/// The refusal text for a config file this build cannot read.
///
/// Old files get the exact hand-migration; new files get told to upgrade.
/// Both name the version they declared, and the caller adds the path.
fn schema_mismatch_message(found: u32) -> String {
    if found > CURRENT_SCHEMA_VERSION {
        return format!(
            "config declares schema_version {found} but this build reads \
             schema {CURRENT_SCHEMA_VERSION}: the file was written by a \
             newer hypercolor. Upgrade hypercolor, or move the file aside \
             to start from defaults. This build will not guess at a shape \
             it does not know."
        );
    }
    format!(
        "config declares schema_version {found} but this build reads \
         schema {CURRENT_SCHEMA_VERSION}; hypercolor no longer migrates \
         older config files. Edit the file by hand, setting\n\n    \
         schema_version = {CURRENT_SCHEMA_VERSION}\n\nand, when the file \
         predates the interaction-route split, adding both routes under \
         [input]:\n\n    daemon_route = \"merge\"\n    preview_route = \
         \"browser\"\n\nWithout those two lines the version bump \
         silently adopts the new \"host\" default for daemon_route. \
         Moving the file aside starts from defaults instead."
    )
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
    use hypercolor_types::event::HypercolorEvent;

    const UPDATED_PORT: u16 = 17_777;

    fn attach_bus(
        manager: &ConfigManager,
    ) -> tokio::sync::broadcast::Receiver<crate::bus::TimestampedEvent> {
        let bus = Arc::new(HypercolorBus::new());
        let events = bus.subscribe_all();
        manager.attach_change_stream(bus);
        events
    }

    fn config_changed_keys(
        events: &mut tokio::sync::broadcast::Receiver<crate::bus::TimestampedEvent>,
    ) -> Vec<String> {
        let mut keys = Vec::new();
        while let Ok(timestamped) = events.try_recv() {
            if let HypercolorEvent::ConfigChanged { key, .. } = timestamped.event {
                keys.push(key);
            }
        }
        keys
    }

    fn manager_with_persisted_default(dir: &Path, name: &str) -> (ConfigManager, PathBuf) {
        let path = dir.join(name).join("config.toml");
        let manager = ConfigManager::new(path.clone()).expect("config manager");
        manager.save().expect("persist default");
        (manager, path)
    }

    fn staged_temporary_files(path: &Path) -> Vec<PathBuf> {
        let prefix = format!(
            ".{}.",
            path.file_name()
                .and_then(|name| name.to_str())
                .expect("test config path should have a UTF-8 file name")
        );
        std::fs::read_dir(
            path.parent()
                .expect("test config path should have a parent directory"),
        )
        .expect("test config parent should be readable")
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|candidate| {
            candidate
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| {
                    name.starts_with(&prefix)
                        && Path::new(name)
                            .extension()
                            .is_some_and(|extension| extension == "tmp")
                })
        })
        .collect()
    }

    #[test]
    fn serialization_failure_changes_neither_disk_nor_live_snapshot() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let (manager, path) = manager_with_persisted_default(dir.as_ref(), "serialize");
        let expected = manager.get().clone();
        let original_port = expected.daemon.port;
        manager.fail_next_persistence_at(ConfigPersistenceStage::Serialize);

        let result = manager.modify_and_save_if_current_snapshot(&expected, |config| {
            config.daemon.port = UPDATED_PORT;
        });

        assert!(result.is_err());
        assert_eq!(manager.get().daemon.port, original_port);
        assert_eq!(
            ConfigManager::load(&path)
                .expect("persisted config")
                .daemon
                .port,
            original_port
        );
    }

    #[test]
    fn failed_persistence_publishes_no_config_changed() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let (manager, _path) = manager_with_persisted_default(dir.as_ref(), "silent-failure");
        let mut events = attach_bus(&manager);
        let expected = manager.get().clone();

        manager.fail_next_persistence_at(ConfigPersistenceStage::Serialize);
        assert!(
            manager
                .modify_and_save_if_current_snapshot(&expected, |config| {
                    config.daemon.port = UPDATED_PORT;
                })
                .is_err()
        );
        assert!(config_changed_keys(&mut events).is_empty());

        manager.modify(|config| config.daemon.port = UPDATED_PORT);
        manager.fail_next_persistence_at(ConfigPersistenceStage::Serialize);
        assert!(manager.save().is_err());
        assert!(config_changed_keys(&mut events).is_empty());

        manager.save().expect("the next save should land");
        assert_eq!(
            config_changed_keys(&mut events),
            vec!["daemon.port".to_owned()],
            "the stream stays live after a failed write"
        );
    }

    #[test]
    fn admitted_failure_publishes_and_flushes_the_authoritative_config() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let (manager, path) = manager_with_persisted_default(dir.as_ref(), "retry");
        let mut events = attach_bus(&manager);
        let expected = manager.get().clone();
        let original_port = expected.daemon.port;
        manager
            .persistence
            .set_injected_replace_failures(usize::MAX);

        assert!(
            manager
                .modify_and_save_if_current_snapshot(&expected, |config| {
                    config.daemon.port = UPDATED_PORT;
                })
                .expect("admitted mutation should remain authoritative")
                .is_some()
        );
        assert_eq!(manager.get().daemon.port, UPDATED_PORT);
        assert_eq!(
            config_changed_keys(&mut events),
            vec!["daemon.port".to_owned()],
            "an admitted mutation is authoritative, so it publishes even before the file lands"
        );
        assert_eq!(
            ConfigManager::load(&path)
                .expect("previous config remains readable")
                .daemon
                .port,
            original_port
        );

        manager.persistence.set_injected_replace_failures(0);
        let report = crate::persistence::flush_all(std::time::Duration::from_secs(5));
        assert!(report.is_complete());
        assert!(report.written() >= 1);
        assert_eq!(
            ConfigManager::load(&path)
                .expect("converged config should read")
                .daemon
                .port,
            UPDATED_PORT
        );
    }

    #[test]
    fn concurrent_capture_stages_use_distinct_owned_temporary_files() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let (manager, path) = manager_with_persisted_default(dir.as_ref(), "capture-stages");
        let expected = manager.get().clone();
        let mut first_capture = expected.capture.clone();
        first_capture.capture_fps = first_capture.capture_fps.saturating_add(1);
        let mut second_capture = expected.capture.clone();
        second_capture.capture_fps = second_capture.capture_fps.saturating_add(2);

        let (first, second) = std::thread::scope(|scope| {
            let first = scope.spawn(|| {
                manager
                    .stage_capture_config(&expected, first_capture)
                    .expect("first capture stage should succeed")
                    .expect("expected pointer should remain current")
            });
            let second = scope.spawn(|| {
                manager
                    .stage_capture_config(&expected, second_capture)
                    .expect("second capture stage should succeed")
                    .expect("expected pointer should remain current")
            });
            (
                first.join().expect("first capture stage should join"),
                second.join().expect("second capture stage should join"),
            )
        });
        let first_path = first.file.path().to_owned();
        let second_path = second.file.path().to_owned();

        assert_ne!(first_path, second_path);
        assert!(first_path.exists());
        assert!(second_path.exists());
        assert_eq!(staged_temporary_files(&path).len(), 2);
        drop((first, second));
        assert!(staged_temporary_files(&path).is_empty());
    }

    #[test]
    fn staged_capture_commit_orders_durability_runtime_and_live_publication() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let (manager, path) = manager_with_persisted_default(dir.as_ref(), "capture-ordering");
        let expected = manager.get().clone();
        let mut capture = expected.capture.clone();
        capture.enabled = !capture.enabled;
        let staged = manager
            .stage_capture_config(&expected, capture.clone())
            .expect("capture staging should succeed")
            .expect("expected pointer should remain current");
        let failed_temporary_path = staged.file.path().to_owned();
        let runtime_committed = std::sync::atomic::AtomicBool::new(false);
        manager.fail_next_persistence_at(ConfigPersistenceStage::Replace);

        assert!(
            manager
                .commit_staged_capture_if_current(&expected, None, staged, |install_live| {
                    runtime_committed.store(true, Ordering::Release);
                    install_live();
                })
                .is_err()
        );
        assert!(!runtime_committed.load(Ordering::Acquire));
        assert!(!failed_temporary_path.exists());
        assert_eq!(manager.get().capture, expected.capture);
        assert_eq!(
            ConfigManager::load(&path)
                .expect("persisted config should remain readable")
                .capture,
            expected.capture
        );

        let staged = manager
            .stage_capture_config(&expected, capture.clone())
            .expect("capture restaging should succeed")
            .expect("expected pointer should remain current");
        manager
            .commit_staged_capture_if_current(&expected, None, staged, |install_live| {
                assert_eq!(manager.get().capture, expected.capture);
                runtime_committed.store(true, Ordering::Release);
                install_live();
            })
            .expect("durable replacement should succeed")
            .expect("expected pointer should commit");
        assert!(runtime_committed.load(Ordering::Acquire));
        assert_eq!(manager.get().capture, capture);
        assert_eq!(
            ConfigManager::load(&path)
                .expect("committed config should reopen")
                .capture,
            capture
        );
    }
}
