//! Narrow dependency handles for daemon domain services (Spec 76 §6.4).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use hypercolor_core::asset::AssetLibrary;
use hypercolor_core::bus::HypercolorBus;
use hypercolor_core::config::ConfigManager;
use hypercolor_core::device::{DeviceLifecycleManager, DeviceRegistry};
use hypercolor_core::effect::EffectRegistry;
use hypercolor_core::engine::RenderLoop;
use hypercolor_core::scene::SceneManager;
use hypercolor_network::DriverModuleRegistry;
use hypercolor_types::device::{DeviceInfo, DriverModuleKind, DriverTransportKind};
use hypercolor_types::layer::{LayerSource, SceneLayer};
use hypercolor_types::scene::{Scene, SceneId, Zone, ZoneId};
use hypercolor_types::spatial::{EdgeBehavior, Output, SamplingMode, SpatialLayout};
use tokio::sync::{Mutex, OwnedRwLockReadGuard, OwnedRwLockWriteGuard, RwLock};

use crate::domain::DomainError;
use crate::domain::commit::SceneCommit;
use crate::domain::effect::EffectContext;
use crate::domain::layout::LayoutContext;
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
use crate::{discovery, layout_auto_exclusions};

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
    /// Effect catalog, validation, and activation authority.
    pub effects: EffectContext,
    /// Live scene-tree mutation authority.
    pub scene_tree: SceneTreeContext,
    /// Named scene library and activation authority.
    pub scene_library: SceneLibraryContext,
}

pub(crate) struct DomainContextResources {
    pub effect_registry: Arc<RwLock<EffectRegistry>>,
    pub spatial: SpatialService,
    pub event_bus: Arc<HypercolorBus>,
}

impl DomainContexts {
    pub(crate) fn assemble(
        runtime_session: RuntimeSessionService,
        devices: DeviceContext,
        scene: SceneContext,
        layout: LayoutContext,
        output: OutputContext,
        resources: DomainContextResources,
    ) -> Self {
        let effects = EffectContext::new(
            Arc::clone(&resources.effect_registry),
            scene.clone(),
            resources.spatial,
            output.clone(),
        );
        let scene_tree = SceneTreeContext::new(
            scene.clone(),
            effects.clone(),
            devices.clone(),
            output.clone(),
        );
        let scene_library = SceneLibraryContext::new(
            scene.clone(),
            effects.clone(),
            layout.clone(),
            output.clone(),
            resources.event_bus,
        );
        Self {
            runtime_session,
            devices,
            scene,
            layout,
            output,
            effects,
            scene_tree,
            scene_library,
        }
    }
}

/// Owning runtime-session persistence boundary.
#[derive(Clone)]
pub struct RuntimeSessionService {
    path: PathBuf,
    scenes: SceneService,
    spatial: SpatialService,
    output_power: OutputPower,
    driver_host: Arc<DaemonDriverHost>,
    driver_registry: Arc<DriverModuleRegistry>,
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
    runtime_session: RuntimeSessionService,
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

impl RuntimeSessionService {
    pub(crate) fn new(
        path: PathBuf,
        scenes: SceneService,
        spatial: SpatialService,
        output_power: OutputPower,
        driver_host: Arc<DaemonDriverHost>,
        driver_registry: Arc<DriverModuleRegistry>,
    ) -> Self {
        Self {
            path,
            scenes,
            spatial,
            output_power,
            driver_host,
            driver_registry,
            identity_publication_gate: Arc::new(RwLock::new(())),
            #[cfg(all(test, feature = "persistence-test-hooks"))]
            save_admission_test_barrier: Arc::new(std::sync::Mutex::new(None)),
        }
    }

    /// Capture the complete durable runtime-session projection.
    pub async fn snapshot(&self) -> RuntimeSessionSnapshot {
        let mut snapshot = {
            let manager = self.scenes.snapshot().await;
            runtime_state::snapshot_from_scene_manager(&manager)
        };
        snapshot.active_layout_id = Some(self.spatial.layout().id.clone());
        snapshot.manual_paused = self.output_power.snapshot().manually_paused();
        self.driver_host
            .driver_inventory()
            .refresh(self.driver_registry.as_ref(), self.driver_host.as_ref())
            .await;
        snapshot
    }

    pub(crate) async fn begin_effect_id_migration(
        &self,
    ) -> RuntimeSessionEffectIdMigrationAdmission {
        let publication_guard = Arc::clone(&self.identity_publication_gate)
            .write_owned()
            .await;
        RuntimeSessionEffectIdMigrationAdmission {
            runtime_session: self.clone(),
            publication_guard,
        }
    }

    /// Persist the current scene store before the runtime-session pointer.
    pub async fn save(&self) {
        let save = match self.prepare_save().await {
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
        self.persist_snapshot_with(|_| {}).await
    }

    pub(crate) async fn persist_snapshot_with<F>(
        &self,
        update: F,
    ) -> Result<AtomicWriteOutcome, RuntimeSessionPersistenceError>
    where
        F: FnOnce(&mut RuntimeSessionSnapshot),
    {
        let mut save = self
            .prepare_save()
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

    pub(crate) async fn flush_persistence(&self, timeout: Duration) -> anyhow::Result<()> {
        let _publication_guard = Arc::clone(&self.identity_publication_gate)
            .read_owned()
            .await;
        let writer = AtomicFileWriter::new(&self.path)?;
        tokio::task::spawn_blocking(move || writer.flush(timeout)).await??;
        Ok(())
    }

    async fn prepare_save(&self) -> Result<RuntimeSessionSave, runtime_state::RuntimeSessionError> {
        #[cfg(all(test, feature = "persistence-test-hooks"))]
        self.pause_before_save_admission_for_test().await;
        let publication_guard = Arc::clone(&self.identity_publication_gate)
            .read_owned()
            .await;
        let pending = runtime_state::reserve_save(&self.path)?;
        let snapshot = self.snapshot().await;
        Ok(RuntimeSessionSave {
            pending,
            snapshot,
            publication_guard,
        })
    }

    async fn save_scene_store_snapshot(&self) -> anyhow::Result<()> {
        self.scenes.save_snapshot().await
    }

    #[cfg(all(test, feature = "persistence-test-hooks"))]
    pub(crate) fn pause_next_save_before_admission_for_test(
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
            runtime_session,
            publication_guard,
        } = self;
        let mut snapshot = runtime_state::snapshot_from_scene_manager(manager);
        snapshot.active_layout_id = Some(runtime_session.spatial.layout().id.clone());
        snapshot.manual_paused = runtime_session.output_power.snapshot().manually_paused();
        let pending = runtime_state::reserve_save(&runtime_session.path)
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
    devices: DeviceContext,
}

impl SceneContext {
    pub(crate) fn new(
        scenes: SceneService,
        runtime_session: RuntimeSessionService,
        asset_library: Arc<RwLock<AssetLibrary>>,
        config_manager: Option<Arc<ConfigManager>>,
        render_loop: Arc<RwLock<RenderLoop>>,
        devices: DeviceContext,
    ) -> Self {
        Self {
            scenes,
            runtime_session,
            asset_library,
            config_manager,
            render_loop,
            devices,
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

    /// Device lifecycle authority needed after scene targeting changes.
    #[must_use]
    pub const fn devices(&self) -> &DeviceContext {
        &self.devices
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

/// Device lifecycle and discovery-layout reconciliation authority.
#[derive(Clone)]
pub struct DeviceContext {
    device_registry: DeviceRegistry,
    lifecycle_manager: Arc<Mutex<DeviceLifecycleManager>>,
    driver_host: Arc<DaemonDriverHost>,
    driver_registry: Arc<DriverModuleRegistry>,
    config_manager: Option<Arc<ConfigManager>>,
    layout_auto_exclusions: Arc<RwLock<layout_auto_exclusions::LayoutAutoExclusionStore>>,
    layout_auto_exclusions_path: PathBuf,
}

impl DeviceContext {
    pub(crate) fn new(
        device_registry: DeviceRegistry,
        lifecycle_manager: Arc<Mutex<DeviceLifecycleManager>>,
        driver_host: Arc<DaemonDriverHost>,
        driver_registry: Arc<DriverModuleRegistry>,
        config_manager: Option<Arc<ConfigManager>>,
        layout_auto_exclusions: Arc<RwLock<layout_auto_exclusions::LayoutAutoExclusionStore>>,
        layout_auto_exclusions_path: PathBuf,
    ) -> Self {
        Self {
            device_registry,
            lifecycle_manager,
            driver_host,
            driver_registry,
            config_manager,
            layout_auto_exclusions,
            layout_auto_exclusions_path,
        }
    }

    /// Resolve the stable layout identity for a tracked device.
    pub async fn resolved_layout_device_id(&self, device_info: &DeviceInfo) -> String {
        if let Some(layout_device_id) = {
            let lifecycle = self.lifecycle_manager.lock().await;
            lifecycle
                .layout_device_id_for(device_info.id)
                .map(ToOwned::to_owned)
        } {
            return layout_device_id;
        }

        let fingerprint = self
            .device_registry
            .fingerprint_for_id(&device_info.id)
            .await;
        DeviceLifecycleManager::canonical_layout_device_id(device_info, fingerprint.as_ref())
    }

    /// Mint the canonical auto-layout outputs for one connected device.
    pub async fn layout_outputs_for(&self, requested_layout_id: &str) -> Vec<Output> {
        let tracked = self.device_registry.list().await;
        for device in &tracked {
            let layout_device_id = self.resolved_layout_device_id(&device.info).await;
            if layout_device_id != requested_layout_id {
                continue;
            }
            let mut scratch = SpatialLayout {
                id: format!("mint-{layout_device_id}"),
                name: device.info.name.clone(),
                description: None,
                canvas_width: 1,
                canvas_height: 1,
                zones: Vec::new(),
                default_sampling_mode: SamplingMode::Bilinear,
                default_edge_behavior: EdgeBehavior::Clamp,
                spaces: None,
                version: 1,
            };
            let _minted = discovery::auto_layout::append_auto_layout_zones_for_device(
                &mut scratch,
                &layout_device_id,
                &device.info,
            );
            return scratch.zones;
        }
        Vec::new()
    }

    /// Resolve native display canvases for every renderable device.
    pub async fn connected_display_surface_layouts(
        &self,
    ) -> Vec<(hypercolor_types::device::DeviceId, String, SpatialLayout)> {
        self.device_registry
            .list()
            .await
            .into_iter()
            .filter(|tracked| tracked.state.is_renderable())
            .filter_map(|tracked| {
                let surface = crate::domain::display::display_surface_info(&tracked.info)?;
                Some((
                    tracked.info.id,
                    tracked.info.name.clone(),
                    crate::domain::display::display_face_layout(
                        tracked.info.id,
                        tracked.info.name.as_str(),
                        surface,
                    ),
                ))
            })
            .collect()
    }

    /// Re-evaluate device eligibility after scene targeting changes.
    pub async fn sync_connectivity(&self) {
        let runtime = self.driver_host.discovery_runtime();
        discovery::sync_active_layout_connectivity(&runtime, None).await;
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

    /// Reconcile discovery exclusions after zone layouts change.
    pub async fn reconcile_zone_auto_exclusions(
        &self,
        scene_id: SceneId,
        previous_zones: &[Zone],
        updated_zones: &[Zone],
    ) {
        let changed = {
            let mut exclusions = self.layout_auto_exclusions.write().await;
            let mut changed = false;
            for previous_zone in previous_zones {
                let Some(updated_zone) = updated_zones
                    .iter()
                    .find(|zone| zone.id == previous_zone.id)
                else {
                    continue;
                };
                if previous_zone.layout.zones == updated_zone.layout.zones {
                    continue;
                }

                let key = layout_auto_exclusions::LayoutAutoExclusionKey::zone(
                    scene_id,
                    previous_zone.id,
                );
                let current = exclusions.get(&key).cloned().unwrap_or_default();
                let next = layout_auto_exclusions::reconcile_layout_device_exclusions(
                    &previous_zone.layout.zones,
                    &updated_zone.layout.zones,
                    &current,
                );
                if next == current {
                    continue;
                }
                if next.is_empty() {
                    exclusions.remove(&key);
                } else {
                    exclusions.insert(key, next);
                }
                changed = true;
            }
            changed
        };

        if changed {
            self.persist_layout_auto_exclusions().await;
        }
    }

    /// Drop discovery exclusions owned by a removed zone.
    pub async fn remove_zone_auto_exclusions(&self, scene_id: SceneId, zone_id: ZoneId) {
        let removed = {
            let mut exclusions = self.layout_auto_exclusions.write().await;
            exclusions
                .remove(&layout_auto_exclusions::LayoutAutoExclusionKey::zone(
                    scene_id, zone_id,
                ))
                .is_some()
        };

        if removed {
            self.persist_layout_auto_exclusions().await;
        }
    }

    /// Persist the discovery auto-sync exclusion store.
    pub async fn persist_layout_auto_exclusions(&self) {
        let exclusions = self.layout_auto_exclusions.read().await;
        if let Err(error) =
            layout_auto_exclusions::save(&self.layout_auto_exclusions_path, &exclusions)
        {
            tracing::warn!(
                path = %self.layout_auto_exclusions_path.display(),
                %error,
                "Failed to persist layout auto-exclusion store"
            );
        }
    }
}
