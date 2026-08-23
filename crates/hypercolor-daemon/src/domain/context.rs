//! Narrow dependency handles for daemon domain services (Spec 76 §6.4).

use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use hypercolor_core::asset::AssetLibrary;
use hypercolor_core::bus::HypercolorBus;
use hypercolor_core::config::ConfigManager;
use hypercolor_core::device::DeviceRegistry;
use hypercolor_core::effect::EffectRegistry;
use hypercolor_core::engine::RenderLoop;
use hypercolor_core::input::{SourceStatusRegistry, SourceStatusRegistrySnapshot};
use hypercolor_core::scene::SceneManager;
use hypercolor_network::DriverModuleRegistry;
use hypercolor_types::device::{DriverModuleKind, DriverTransportKind};
use hypercolor_types::layer::{LayerSource, SceneLayer};
use hypercolor_types::scene::{Scene, SceneId};
use tokio::sync::{OwnedRwLockReadGuard, OwnedRwLockWriteGuard, RwLock};

use crate::device_metrics::DeviceMetricsSnapshotStore;
use crate::discovery;
use crate::display_frames::DisplayFrameRuntime;
use crate::display_preferences::DisplayPreferencesStore;
use crate::domain::DomainError;
use crate::domain::commit::SceneCommit;
use crate::domain::diagnostics::DiagnosticsContext;
use crate::domain::display::DisplayContext;
use crate::domain::effect::{EffectContext, EffectIdentityResources};
use crate::domain::layout::{LayoutContext, LayoutRuntime};
use crate::domain::output::OutputContext;
use crate::domain::scene::{
    COMMIT_ATTEMPTS, MEDIA_SOFT_PRODUCER_COST_US, MediaAdmissionContext, SceneLibraryContext,
    SceneMediaAdmission, SceneMutation, SceneService,
};
use crate::domain::scene_tree::SceneTreeContext;
use crate::domain::spatial::SpatialService;
use crate::network::DaemonDriverHost;
use crate::output_power::OutputPower;
use crate::persistence::{AtomicFileWriter, AtomicWriteOutcome};
use crate::runtime_state::{self, RuntimeSessionSnapshot};

/// Complete daemon domain graph assembled once by the composition root.
#[derive(Clone)]
pub struct DomainContexts {
    /// Runtime-session snapshot and persistence authority.
    pub runtime_session: RuntimeSessionService,
    /// Device lifecycle and discovery reconciliation authority.
    pub devices: DeviceContext,
    /// Live scene transaction authority.
    pub scene: SceneContext,
    /// Layout catalog and activation authority.
    pub layout: LayoutContext,
    /// Global output power and brightness authority.
    pub output: OutputContext,
    /// Platform input health and live configuration authority.
    pub platform: PlatformContext,
    /// Display default-face and preview-frame authority.
    pub display: DisplayContext,
    /// Daemon health and diagnostics authority.
    pub diagnostics: DiagnosticsContext,
    /// Effect catalog, validation, and activation authority.
    pub effects: EffectContext,
    /// Live scene-tree mutation authority.
    pub scene_tree: SceneTreeContext,
    /// Named scene library and activation authority.
    pub scene_library: SceneLibraryContext,
}

pub(crate) struct DomainContextResources {
    pub effect_registry: Arc<RwLock<EffectRegistry>>,
    pub effect_identity: EffectIdentityResources,
    pub spatial: SpatialService,
    pub event_bus: Arc<HypercolorBus>,
    pub display_preferences: Arc<RwLock<DisplayPreferencesStore>>,
    pub display_frames: Arc<RwLock<DisplayFrameRuntime>>,
    pub device_metrics: DeviceMetricsSnapshotStore,
    pub input_manager: Arc<tokio::sync::Mutex<hypercolor_core::input::InputManager>>,
}

impl DomainContexts {
    pub(crate) fn assemble(
        runtime_session: RuntimeSessionService,
        devices: DeviceContext,
        scene: SceneContext,
        layout: LayoutContext,
        output: OutputContext,
        platform: PlatformContext,
        resources: DomainContextResources,
    ) -> Self {
        let effects = EffectContext::new(
            Arc::clone(&resources.effect_registry),
            scene.clone(),
            resources.spatial.clone(),
            output.clone(),
            resources.effect_identity,
            Arc::clone(&resources.event_bus),
        );
        let scene_tree = SceneTreeContext::new(
            scene.clone(),
            effects.clone(),
            layout.clone(),
            output.clone(),
        );
        let scene_library = SceneLibraryContext::new(
            scene.clone(),
            effects.clone(),
            layout.clone(),
            output.clone(),
            resources.event_bus,
        );
        let display = DisplayContext::new(
            resources.display_preferences,
            resources.display_frames,
            scene.clone(),
            effects.clone(),
            layout.clone(),
            devices.clone(),
        );
        let diagnostics = DiagnosticsContext::new(
            platform.clone(),
            output.clone(),
            devices.clone(),
            display.clone(),
            resources.device_metrics,
            resources.input_manager,
            resources.spatial,
        );
        Self {
            runtime_session,
            devices,
            scene,
            layout,
            output,
            platform,
            display,
            diagnostics,
            effects,
            scene_tree,
            scene_library,
        }
    }
}

/// Platform input health projection dependencies.
#[derive(Clone)]
pub struct PlatformContext {
    input_status: SourceStatusRegistry,
    config_manager: Option<Arc<ConfigManager>>,
}

impl PlatformContext {
    pub(crate) fn new(
        input_status: SourceStatusRegistry,
        config_manager: Option<Arc<ConfigManager>>,
    ) -> Self {
        Self {
            input_status,
            config_manager,
        }
    }

    #[must_use]
    pub(crate) fn source_status_snapshot(&self) -> Arc<SourceStatusRegistrySnapshot> {
        self.input_status.snapshot()
    }

    /// Whether a live configuration manager backs this projection.
    #[must_use]
    pub(crate) fn config_available(&self) -> bool {
        self.config_manager.is_some()
    }

    #[must_use]
    pub(crate) fn input_enabled(&self) -> bool {
        self.config_manager
            .as_ref()
            .is_some_and(|manager| manager.get().input.enabled)
    }

    #[cfg(test)]
    pub(crate) fn source_status_registry(&self) -> SourceStatusRegistry {
        self.input_status.clone()
    }
}

/// Owning runtime-session persistence boundary.
#[derive(Clone)]
pub struct RuntimeSessionService {
    path: PathBuf,
    projection: RuntimeSessionProjection,
    driver_host: std::sync::Weak<DaemonDriverHost>,
}

#[derive(Clone)]
pub(crate) struct RuntimeSessionProjection {
    scenes: SceneService,
    spatial: SpatialService,
    output_power: OutputPower,
    identity_publication_gate: Arc<RwLock<()>>,
    #[cfg(all(test, feature = "persistence-test-hooks"))]
    save_admission_test_barrier:
        Arc<std::sync::Mutex<Option<Arc<RuntimeSessionSaveAdmissionTestBarrier>>>>,
}

#[cfg(all(test, feature = "persistence-test-hooks"))]
pub(crate) struct RuntimeSessionSaveAdmissionTestBarrier {
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
}

pub(crate) struct RuntimeSessionEffectIdMigrationAdmission {
    projection: RuntimeSessionProjection,
    path: PathBuf,
    publication_guard: OwnedRwLockWriteGuard<()>,
}

pub(crate) struct RuntimeSessionEffectIdMigration {
    pending: runtime_state::PreparedRuntimeSnapshotSave,
    publication_guard: OwnedRwLockWriteGuard<()>,
}

pub(crate) struct AdmittedRuntimeSessionEffectIdMigration {
    pending: runtime_state::AdmittedRuntimeSnapshotSave,
    publication_guard: OwnedRwLockWriteGuard<()>,
}

pub(crate) struct PersistedRuntimeSessionEffectIdMigration {
    _publication_guard: OwnedRwLockWriteGuard<()>,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum RuntimeSessionPersistenceError {
    #[error("{0}")]
    BeforeAdmission(runtime_state::RuntimeSessionError),
    #[error("{0}")]
    RetryArmed(runtime_state::RuntimeSessionError),
}

struct RuntimeSessionSave {
    pending: runtime_state::RuntimeSnapshotSave,
    snapshot: RuntimeSessionSnapshot,
    publication_guard: OwnedRwLockReadGuard<()>,
}

impl RuntimeSessionProjection {
    pub(crate) fn new(
        scenes: SceneService,
        spatial: SpatialService,
        output_power: OutputPower,
    ) -> Self {
        Self {
            scenes,
            spatial,
            output_power,
            identity_publication_gate: Arc::new(RwLock::new(())),
            #[cfg(all(test, feature = "persistence-test-hooks"))]
            save_admission_test_barrier: Arc::new(std::sync::Mutex::new(None)),
        }
    }

    pub(crate) async fn snapshot(&self) -> RuntimeSessionSnapshot {
        let mut snapshot = {
            let manager = self.scenes.snapshot().await;
            runtime_state::snapshot_from_scene_manager(&manager)
        };
        snapshot.active_layout_id = Some(self.spatial.layout().id.clone());
        let power = self.output_power.snapshot();
        snapshot.manual_paused = power.manually_paused();
        snapshot
    }

    pub(crate) async fn begin_effect_id_migration(
        &self,
        path: PathBuf,
    ) -> RuntimeSessionEffectIdMigrationAdmission {
        let publication_guard = Arc::clone(&self.identity_publication_gate)
            .write_owned()
            .await;
        RuntimeSessionEffectIdMigrationAdmission {
            projection: self.clone(),
            path,
            publication_guard,
        }
    }

    pub(crate) async fn persist_snapshot_with<F, Fut>(
        &self,
        path: &Path,
        before_snapshot: Fut,
        update: F,
    ) -> Result<AtomicWriteOutcome, RuntimeSessionPersistenceError>
    where
        F: FnOnce(&mut RuntimeSessionSnapshot),
        Fut: Future<Output = ()>,
    {
        let mut save = self
            .prepare_save(path, before_snapshot)
            .await
            .map_err(RuntimeSessionPersistenceError::BeforeAdmission)?;
        update(&mut save.snapshot);
        save.commit().map_err(|error| match error {
            error @ runtime_state::RuntimeSessionError::Persist { .. } => {
                RuntimeSessionPersistenceError::RetryArmed(error)
            }
            error => RuntimeSessionPersistenceError::BeforeAdmission(error),
        })
    }

    pub(crate) async fn flush_persistence(
        &self,
        path: &Path,
        timeout: Duration,
    ) -> anyhow::Result<()> {
        let _publication_guard = Arc::clone(&self.identity_publication_gate)
            .read_owned()
            .await;
        let writer = AtomicFileWriter::new(path)?;
        tokio::task::spawn_blocking(move || writer.flush(timeout)).await??;
        Ok(())
    }

    async fn prepare_save<Fut>(
        &self,
        path: &Path,
        before_snapshot: Fut,
    ) -> Result<RuntimeSessionSave, runtime_state::RuntimeSessionError>
    where
        Fut: Future<Output = ()>,
    {
        #[cfg(all(test, feature = "persistence-test-hooks"))]
        self.pause_before_save_admission_for_test().await;
        let publication_guard = Arc::clone(&self.identity_publication_gate)
            .read_owned()
            .await;
        let pending = runtime_state::reserve_save(path)?;
        before_snapshot.await;
        let snapshot = self.snapshot().await;
        Ok(RuntimeSessionSave {
            pending,
            snapshot,
            publication_guard,
        })
    }

    #[cfg(all(test, feature = "persistence-test-hooks"))]
    fn pause_next_save_before_admission_for_test(
        &self,
    ) -> Arc<RuntimeSessionSaveAdmissionTestBarrier> {
        let barrier = Arc::new(RuntimeSessionSaveAdmissionTestBarrier {
            entered: tokio::sync::Notify::new(),
            release: tokio::sync::Notify::new(),
        });
        *self
            .save_admission_test_barrier
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Arc::clone(&barrier));
        barrier
    }

    #[cfg(all(test, feature = "persistence-test-hooks"))]
    async fn pause_before_save_admission_for_test(&self) {
        let barrier = self
            .save_admission_test_barrier
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some(barrier) = barrier {
            barrier.entered.notify_one();
            barrier.release.notified().await;
        }
    }
}

impl RuntimeSessionService {
    pub(crate) fn new(
        path: PathBuf,
        projection: RuntimeSessionProjection,
        driver_host: &Arc<DaemonDriverHost>,
    ) -> Self {
        Self {
            path,
            projection,
            driver_host: Arc::downgrade(driver_host),
        }
    }

    /// Capture the complete durable runtime-session projection.
    pub async fn snapshot(&self) -> RuntimeSessionSnapshot {
        let snapshot = self.projection.snapshot().await;
        if let Some(driver_host) = self.driver_host.upgrade() {
            driver_host.refresh_driver_inventory().await;
        }
        snapshot
    }

    pub(crate) async fn begin_effect_id_migration(
        &self,
    ) -> RuntimeSessionEffectIdMigrationAdmission {
        self.projection
            .begin_effect_id_migration(self.path.clone())
            .await
    }

    /// Persist the current scene store before the runtime-session pointer.
    pub async fn save(&self) {
        let driver_host = self.driver_host.upgrade();
        let refresh_inventory = async move {
            if let Some(driver_host) = driver_host {
                driver_host.refresh_driver_inventory().await;
            }
        };
        let save = match self
            .projection
            .prepare_save(&self.path, refresh_inventory)
            .await
        {
            Ok(save) => save,
            Err(error) => {
                tracing::warn!(
                    path = %self.path.display(),
                    %error,
                    "Failed to reserve runtime session snapshot"
                );
                return;
            }
        };
        if let Err(error) = self.save_scene_store_snapshot().await {
            tracing::warn!(%error, "Failed to persist scene store before runtime snapshot save");
        }

        if let Err(error) = save.commit() {
            tracing::warn!(
                path = %self.path.display(),
                %error,
                "Failed to persist runtime session snapshot"
            );
        }
    }

    pub(crate) async fn persist_snapshot(
        &self,
    ) -> Result<AtomicWriteOutcome, RuntimeSessionPersistenceError> {
        let driver_host = self.driver_host.upgrade();
        self.projection
            .persist_snapshot_with(
                &self.path,
                async move {
                    if let Some(driver_host) = driver_host {
                        driver_host.refresh_driver_inventory().await;
                    }
                },
                |_| {},
            )
            .await
    }

    async fn save_scene_store_snapshot(&self) -> anyhow::Result<()> {
        self.projection.scenes.save_snapshot().await
    }

    #[cfg(all(test, feature = "persistence-test-hooks"))]
    pub(crate) fn pause_next_save_before_admission_for_test(
        &self,
    ) -> Arc<RuntimeSessionSaveAdmissionTestBarrier> {
        self.projection.pause_next_save_before_admission_for_test()
    }
}

impl RuntimeSessionSave {
    fn commit(self) -> Result<AtomicWriteOutcome, runtime_state::RuntimeSessionError> {
        let Self {
            pending,
            snapshot,
            publication_guard,
        } = self;
        let outcome = runtime_state::save_reserved(pending, &snapshot);
        drop(publication_guard);
        outcome
    }
}

#[cfg(all(test, feature = "persistence-test-hooks"))]
impl RuntimeSessionSaveAdmissionTestBarrier {
    pub(crate) async fn wait_until_entered(&self) {
        self.entered.notified().await;
    }

    pub(crate) fn release(&self) {
        self.release.notify_one();
    }
}

impl RuntimeSessionEffectIdMigration {
    pub(crate) fn admit(self) -> AdmittedRuntimeSessionEffectIdMigration {
        AdmittedRuntimeSessionEffectIdMigration {
            pending: self.pending.admit(),
            publication_guard: self.publication_guard,
        }
    }
}

impl AdmittedRuntimeSessionEffectIdMigration {
    pub(crate) fn persist(
        self,
    ) -> (
        PersistedRuntimeSessionEffectIdMigration,
        crate::domain::effect::IdentityMigrationPersistence,
    ) {
        let Self {
            pending,
            publication_guard,
        } = self;
        let persistence = match pending.commit_stage_aware() {
            crate::persistence::AtomicWriteCommitResult::DurableWritten => {
                crate::domain::effect::IdentityMigrationPersistence::Written
            }
            crate::persistence::AtomicWriteCommitResult::Superseded => {
                crate::domain::effect::IdentityMigrationPersistence::Superseded
            }
            crate::persistence::AtomicWriteCommitResult::FailedBeforeReplacement(error)
            | crate::persistence::AtomicWriteCommitResult::ReplacementVisibleButNotDurable(error) => {
                crate::domain::effect::IdentityMigrationPersistence::Retrying(error.to_string())
            }
        };
        (
            PersistedRuntimeSessionEffectIdMigration {
                _publication_guard: publication_guard,
            },
            persistence,
        )
    }
}

impl RuntimeSessionEffectIdMigrationAdmission {
    pub(crate) fn prepare(
        self,
        manager: &SceneManager,
    ) -> Result<RuntimeSessionEffectIdMigration, DomainError> {
        let Self {
            projection,
            path,
            publication_guard,
        } = self;
        let mut snapshot = runtime_state::snapshot_from_scene_manager(manager);
        snapshot.active_layout_id = Some(projection.spatial.layout().id.clone());
        snapshot.manual_paused = projection.output_power.snapshot().manually_paused();
        let pending = runtime_state::reserve_save(&path)
            .map_err(|error| DomainError::Internal(error.into()))?;
        let pending = runtime_state::prepare_reserved(pending, &snapshot)
            .map_err(|error| DomainError::Internal(error.into()))?;
        Ok(RuntimeSessionEffectIdMigration {
            pending,
            publication_guard,
        })
    }
}

/// Scene transaction authority shared by every transport and daemon worker.
#[derive(Clone)]
pub struct SceneContext {
    scenes: SceneService,
    runtime_session: RuntimeSessionService,
    asset_library: Arc<RwLock<AssetLibrary>>,
    config_manager: Option<Arc<ConfigManager>>,
    render_loop: Arc<RwLock<RenderLoop>>,
    layout: LayoutContext,
    layout_runtime: LayoutRuntime,
}

impl SceneContext {
    pub(crate) fn new(
        scenes: SceneService,
        runtime_session: RuntimeSessionService,
        asset_library: Arc<RwLock<AssetLibrary>>,
        config_manager: Option<Arc<ConfigManager>>,
        render_loop: Arc<RwLock<RenderLoop>>,
        layout: LayoutContext,
        layout_runtime: LayoutRuntime,
    ) -> Self {
        Self {
            scenes,
            runtime_session,
            asset_library,
            config_manager,
            render_loop,
            layout,
            layout_runtime,
        }
    }

    /// Capture an owned scene-manager snapshot.
    pub async fn snapshot(&self) -> SceneManager {
        self.scenes.snapshot().await
    }

    /// Current scene commit generation for optimistic concurrency.
    #[must_use]
    pub fn revision(&self) -> u64 {
        self.scenes.revision()
    }

    /// Start an owned candidate mutation.
    pub async fn begin_mutation(&self) -> SceneMutation {
        self.scenes.begin_mutation().await
    }

    /// Commit an owned candidate through the one ordered scene authority.
    pub async fn commit(&self, mutation: SceneMutation) -> Result<SceneCommit, DomainError> {
        self.scenes.commit_mutation(mutation).await
    }

    /// Rebuild and commit an idempotent reconciliation after conflicts.
    pub async fn commit_retrying<T>(
        &self,
        mut build: impl FnMut(&mut SceneMutation) -> Result<Option<T>, DomainError>,
    ) -> Result<Option<(T, SceneCommit)>, DomainError> {
        let mut last_conflict = None;
        for _ in 0..COMMIT_ATTEMPTS {
            let mut mutation = self.begin_mutation().await;
            let Some(value) = build(&mut mutation)? else {
                return Ok(None);
            };
            match self.commit(mutation).await {
                Ok(commit) => return Ok(Some((value, commit))),
                Err(conflict @ DomainError::Conflict { .. }) => {
                    last_conflict = Some(conflict);
                }
                Err(error) => return Err(error),
            }
        }
        Err(last_conflict.unwrap_or_else(|| {
            DomainError::conflict("scene commit did not converge after repeated concurrent writes")
        }))
    }

    /// Persist the durable runtime-session projection after a scene mutation.
    pub async fn save_runtime_session(&self) {
        self.runtime_session.save().await;
    }

    pub(crate) async fn begin_runtime_effect_id_migration(
        &self,
    ) -> RuntimeSessionEffectIdMigrationAdmission {
        self.runtime_session.begin_effect_id_migration().await
    }

    pub(crate) async fn prepare_effect_id_migration(
        &self,
        migrations: &std::collections::HashMap<
            hypercolor_types::effect::EffectId,
            hypercolor_types::effect::EffectId,
        >,
    ) -> Result<crate::domain::scene::SceneEffectIdMigration, DomainError> {
        self.scenes.prepare_effect_id_migration(migrations).await
    }

    pub(crate) async fn prepare_effect_id_migration_publication(
        &self,
        migration: crate::domain::scene::PersistedSceneEffectIdMigration,
    ) -> Result<crate::domain::scene::SceneEffectIdMigrationPublication, DomainError> {
        self.scenes
            .prepare_effect_id_migration_publication(migration)
            .await
    }

    pub(crate) fn publish_effect_id_migration(
        &self,
        publication: &mut crate::domain::scene::SceneEffectIdMigrationPublication,
    ) -> SceneCommit {
        self.scenes.publish_effect_id_migration(publication)
    }

    /// Layout connectivity authority needed after scene targeting changes.
    #[must_use]
    pub const fn layout(&self) -> &LayoutContext {
        &self.layout
    }

    pub(crate) const fn layout_runtime(&self) -> &LayoutRuntime {
        &self.layout_runtime
    }

    /// Resolve the current media producer policy and asset vocabulary.
    pub async fn media_admission_context(&self) -> MediaAdmissionContext {
        let asset_mime_types = {
            let library = self.asset_library.read().await;
            library
                .records()
                .iter()
                .map(|record| (record.id, record.mime_type.clone()))
                .collect()
        };
        let media_config = self
            .config_manager
            .as_ref()
            .map_or_else(Default::default, |manager| manager.get().media.clone());
        MediaAdmissionContext::new(asset_mime_types, media_config)
    }

    /// Resolve admission inputs only when a layer adds a media producer.
    pub async fn media_admission_for_layer(
        &self,
        layer: &SceneLayer,
    ) -> Option<MediaAdmissionContext> {
        if !matches!(layer.source, LayerSource::Media { .. }) {
            return None;
        }
        Some(self.media_admission_context().await)
    }

    /// Evaluate a complete scene against the current media producer policy.
    pub async fn evaluate_media_admission(&self, scene: &Scene) -> SceneMediaAdmission {
        self.media_admission_context().await.evaluate(scene)
    }

    /// Preemptively lower the render tier when admitted media cost exceeds the soft cap.
    pub async fn apply_media_soft_admission(
        &self,
        scene_id: SceneId,
        scene_name: &str,
        estimated_cost_us: u64,
    ) {
        if estimated_cost_us <= MEDIA_SOFT_PRODUCER_COST_US {
            return;
        }

        let mut render_loop = self.render_loop.write().await;
        let current_tier = render_loop.stats().tier;
        let Some(next_tier) = current_tier.downshift() else {
            tracing::warn!(
                %scene_id,
                scene_name,
                estimated_cost_us,
                soft_cap_us = MEDIA_SOFT_PRODUCER_COST_US,
                current_tier = %current_tier,
                "Scene media producer cost exceeds soft cap but render loop is already at minimum tier"
            );
            return;
        };

        tracing::warn!(
            %scene_id,
            scene_name,
            estimated_cost_us,
            soft_cap_us = MEDIA_SOFT_PRODUCER_COST_US,
            previous_tier = %current_tier,
            next_tier = %next_tier,
            "Scene media producer cost exceeds soft cap; preemptively downshifting render loop"
        );
        render_loop.set_tier(next_tier);
    }
}

/// Device lifecycle operations that do not own layout policy.
#[derive(Clone)]
pub struct DeviceContext {
    driver_host: Arc<DaemonDriverHost>,
    driver_registry: Arc<DriverModuleRegistry>,
    config_manager: Option<Arc<ConfigManager>>,
}

impl DeviceContext {
    pub(crate) fn new(
        driver_host: Arc<DaemonDriverHost>,
        driver_registry: Arc<DriverModuleRegistry>,
        config_manager: Option<Arc<ConfigManager>>,
    ) -> Self {
        Self {
            driver_host,
            driver_registry,
            config_manager,
        }
    }

    pub(crate) fn layout_runtime(&self) -> LayoutRuntime {
        LayoutRuntime::new(
            self.driver_host.discovery_runtime(),
            Arc::clone(&self.driver_host),
        )
    }

    /// The live device registry every discovery lane writes through.
    #[must_use]
    pub fn device_registry(&self) -> DeviceRegistry {
        self.driver_host.discovery_runtime().device_registry
    }

    /// Schedule discovery after released output ownership becomes available.
    pub fn schedule_output_reconnect(&self, network_only: bool) {
        let Some(config_manager) = self.config_manager.as_ref() else {
            return;
        };
        let config_guard = config_manager.get();
        let config = Arc::clone(&*config_guard);
        let target_ids = network_only.then(|| {
            self.driver_registry
                .discovery_drivers()
                .into_iter()
                .filter_map(|driver| {
                    let descriptor = driver.module_descriptor();
                    let is_network_driver = descriptor.module_kind == DriverModuleKind::Network
                        || descriptor
                            .transports
                            .contains(&DriverTransportKind::Network);
                    is_network_driver.then_some(descriptor.id)
                })
                .collect::<Vec<_>>()
        });
        if target_ids.as_ref().is_some_and(Vec::is_empty) {
            return;
        }
        let targets = match discovery::resolve_targets(
            target_ids.as_deref(),
            &config,
            self.driver_registry.as_ref(),
        ) {
            Ok(targets) => targets,
            Err(error) => {
                tracing::warn!(%error, network_only, "Skipping reconnect scan after output release");
                return;
            }
        };
        if targets.is_empty() {
            return;
        }

        discovery::schedule_discovery_scan(
            self.driver_host.discovery_runtime(),
            Arc::clone(&self.driver_registry),
            Arc::clone(&self.driver_host),
            config,
            targets,
            discovery::default_timeout(),
        );
    }

    /// Release network output ownership after an effect stop.
    pub async fn release_renderable_network_devices(&self) -> usize {
        discovery::release_renderable_network_devices(&self.driver_host.discovery_runtime()).await
    }
}
