//! Spatial layout catalog, mutation, activation, and durability authority.

mod auto_layout;
mod workflows;

use std::collections::{HashMap, HashSet};
#[cfg(feature = "persistence-test-hooks")]
use std::path::Path;
use std::path::PathBuf;
#[cfg(feature = "persistence-test-hooks")]
use std::sync::Mutex as StdMutex;
use std::sync::{Arc, OnceLock, Weak};
use std::time::Duration;

use hypercolor_core::device::DeviceLifecycleManager;
use hypercolor_core::spatial::SpatialEngine;
use hypercolor_types::api::layouts::LayoutSummary;
use hypercolor_types::canvas::SurfaceDescriptor;
use hypercolor_types::device::{DeviceId, DeviceInfo};
use hypercolor_types::scene::{SceneId, Zone, ZoneId};
use hypercolor_types::spatial::{EdgeBehavior, Output, SamplingMode, SpatialLayout};
use tokio::sync::RwLock;
#[cfg(feature = "persistence-test-hooks")]
use tokio::sync::{Notify, Semaphore, watch};

use crate::discovery::DiscoveryRuntime;
use crate::domain::context::RuntimeSessionProjection;
use crate::domain::scene::SceneService;
use crate::domain::spatial::SpatialService;
use crate::domain::{DomainError, ResourceKind};
use crate::layout_auto_exclusions;
use crate::network::DaemonDriverHost;
use crate::persistence::{AtomicFileWriter, AtomicWriteOutcome};
use crate::runtime_state::RuntimeSessionError;
use crate::scene_transactions::{
    LayoutPersistenceOutcome, LayoutPersistencePhase, LayoutTransactionRejection,
    LayoutUpdateError, LayoutUpdateGuard, PreparedLayoutUpdate, SceneActivationGuard,
    SceneTransactionQueue, apply_prepared_layout_update_under_guard_with_persistence,
};

const LAYOUT_DURABILITY_TIMEOUT: Duration = Duration::from_secs(5);

pub use auto_layout::{
    append_auto_layout_zones_for_device, reconcile_auto_layout_zones_for_device,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LayoutPersistenceStatus {
    Synchronized,
    Pending,
}

pub(crate) struct LayoutMutationResult<T> {
    pub data: T,
    pub persistence: LayoutPersistenceStatus,
}

#[cfg(feature = "persistence-test-hooks")]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LayoutMutationTestPoint {
    BeforeGuard,
    AfterMemoryMutation,
    AfterRendererMutation,
    AfterWorkflow,
}

#[cfg(feature = "persistence-test-hooks")]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LayoutMutationTestOperation {
    Create,
    Update,
    Apply,
    Preview,
    Delete,
    ConfigResize,
    SimulatorPrune,
}

#[cfg(feature = "persistence-test-hooks")]
#[derive(Debug)]
pub struct LayoutMutationTestBarrier {
    entered: Notify,
    release: Semaphore,
}

#[cfg(feature = "persistence-test-hooks")]
impl LayoutMutationTestBarrier {
    pub async fn wait_until_entered(&self) {
        self.entered.notified().await;
    }

    pub fn release(&self) {
        self.release.add_permits(1);
    }
}

#[cfg(feature = "persistence-test-hooks")]
#[derive(Debug, Clone, Default)]
pub struct LayoutMutationTestHooks {
    barriers: Arc<StdMutex<HashMap<LayoutMutationTestKey, Arc<LayoutMutationTestBarrier>>>>,
}

#[cfg(feature = "persistence-test-hooks")]
pub struct LayoutTestFixture<'a> {
    context: &'a LayoutContext,
}

#[cfg(feature = "persistence-test-hooks")]
impl<'a> LayoutTestFixture<'a> {
    #[must_use]
    pub fn hooks(&self) -> &'a LayoutMutationTestHooks {
        &self.context.test_hooks
    }

    #[must_use]
    pub fn catalog(&self) -> &'a RwLock<HashMap<String, SpatialLayout>> {
        &self.context.layouts
    }

    #[must_use]
    pub fn catalog_path(&self) -> &'a Path {
        &self.context.layouts_path
    }

    #[must_use]
    pub fn auto_exclusions(&self) -> &'a RwLock<layout_auto_exclusions::LayoutAutoExclusionStore> {
        &self.context.layout_auto_exclusions
    }

    pub fn replace_current(&self, layout: SpatialLayout) {
        let spatial = SpatialEngine::try_new(layout)
            .expect("layout fixture should receive a valid spatial layout");
        self.context.spatial.replace(spatial);
    }

    pub async fn active_primary_ids(&self) -> (SceneId, ZoneId) {
        let scenes = self.context.scenes.snapshot().await;
        let scene = scenes
            .active_scene()
            .expect("layout fixture should have an active scene");
        let zone = scene
            .primary_zone()
            .expect("layout fixture active scene should have a primary zone");
        (scene.id, zone.id)
    }

    pub fn bind_driver_host(&self, driver_host: &Arc<DaemonDriverHost>) {
        self.context.bind_driver_host(driver_host);
    }

    pub async fn sync_active_layout_for_renderable_devices(
        &self,
        runtime: DiscoveryRuntime,
        limit_to_devices: Option<HashSet<DeviceId>>,
    ) {
        self.context
            .sync_active_layout_for_renderable_devices(runtime, limit_to_devices)
            .await;
    }

    pub async fn sync_connectivity(
        &self,
        runtime: DiscoveryRuntime,
        limit_to_devices: Option<HashSet<DeviceId>>,
    ) {
        self.context
            .sync_discovery_connectivity(runtime, limit_to_devices)
            .await;
    }
}

#[cfg(feature = "persistence-test-hooks")]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct LayoutMutationTestKey {
    point: LayoutMutationTestPoint,
    operation: LayoutMutationTestOperation,
    reference: String,
}

#[cfg(feature = "persistence-test-hooks")]
impl LayoutMutationTestHooks {
    pub fn install(
        &self,
        point: LayoutMutationTestPoint,
        operation: LayoutMutationTestOperation,
        reference: impl Into<String>,
    ) -> Arc<LayoutMutationTestBarrier> {
        let barrier = Arc::new(LayoutMutationTestBarrier {
            entered: Notify::new(),
            release: Semaphore::new(0),
        });
        self.barriers
            .lock()
            .expect("layout mutation test hooks should lock")
            .insert(
                LayoutMutationTestKey {
                    point,
                    operation,
                    reference: reference.into(),
                },
                Arc::clone(&barrier),
            );
        barrier
    }

    async fn wait(
        &self,
        point: LayoutMutationTestPoint,
        operation: LayoutMutationTestOperation,
        reference: &str,
    ) {
        let barrier = self
            .barriers
            .lock()
            .expect("layout mutation test hooks should lock")
            .remove(&LayoutMutationTestKey {
                point,
                operation,
                reference: reference.to_owned(),
            });
        if let Some(barrier) = barrier {
            barrier.entered.notify_one();
            let _permit = barrier
                .release
                .acquire()
                .await
                .expect("layout mutation test barrier should remain open");
        }
    }
}

pub(crate) struct LayoutContextResources {
    layouts: HashMap<String, SpatialLayout>,
    layouts_path: PathBuf,
    layout_auto_exclusions: layout_auto_exclusions::LayoutAutoExclusionStore,
    layout_auto_exclusions_path: PathBuf,
}

impl LayoutContextResources {
    pub(crate) fn new(
        layouts: HashMap<String, SpatialLayout>,
        layouts_path: PathBuf,
        layout_auto_exclusions: layout_auto_exclusions::LayoutAutoExclusionStore,
        layout_auto_exclusions_path: PathBuf,
    ) -> Self {
        Self {
            layouts,
            layouts_path,
            layout_auto_exclusions,
            layout_auto_exclusions_path,
        }
    }

    pub(crate) fn load(
        layouts_path: PathBuf,
        layout_auto_exclusions_path: PathBuf,
        default_layout: &SpatialLayout,
    ) -> Self {
        let mut layouts = crate::layout_store::load(&layouts_path).unwrap_or_else(|error| {
            tracing::warn!(
                path = %layouts_path.display(),
                %error,
                "Failed to load persisted layouts; starting with empty store"
            );
            HashMap::new()
        });
        if crate::layout_store::ensure_default_layout(&mut layouts, default_layout) {
            if let Err(error) = crate::layout_store::save(&layouts_path, &layouts) {
                tracing::warn!(
                    path = %layouts_path.display(),
                    %error,
                    "Failed to persist inserted default layout"
                );
            } else {
                tracing::info!(
                    path = %layouts_path.display(),
                    "Inserted missing default layout into persisted layout store"
                );
            }
        }
        let layout_auto_exclusions = layout_auto_exclusions::load(&layout_auto_exclusions_path)
            .unwrap_or_else(|error| {
                tracing::warn!(
                    path = %layout_auto_exclusions_path.display(),
                    %error,
                    "Failed to load layout auto-exclusions; starting with empty store"
                );
                HashMap::new()
            });
        tracing::info!(
            path = %layouts_path.display(),
            count = layouts.len(),
            "Layout store ready"
        );
        tracing::info!(
            path = %layout_auto_exclusions_path.display(),
            "Layout auto-exclusion store ready"
        );
        Self::new(
            layouts,
            layouts_path,
            layout_auto_exclusions,
            layout_auto_exclusions_path,
        )
    }

    pub(crate) fn catalog(&self) -> &HashMap<String, SpatialLayout> {
        &self.layouts
    }
}

#[derive(Clone)]
pub struct LayoutContext {
    layouts: Arc<RwLock<HashMap<String, SpatialLayout>>>,
    layouts_path: PathBuf,
    layout_auto_exclusions: Arc<RwLock<layout_auto_exclusions::LayoutAutoExclusionStore>>,
    layout_auto_exclusions_path: PathBuf,
    spatial: SpatialService,
    scenes: SceneService,
    transactions: SceneTransactionQueue,
    runtime_state_path: PathBuf,
    runtime_projection: RuntimeSessionProjection,
    driver_host: Arc<OnceLock<Weak<DaemonDriverHost>>>,
    #[cfg(feature = "persistence-test-hooks")]
    test_hooks: LayoutMutationTestHooks,
}

impl LayoutContext {
    pub(crate) fn new(
        resources: LayoutContextResources,
        spatial: SpatialService,
        scenes: SceneService,
        transactions: SceneTransactionQueue,
        runtime_state_path: PathBuf,
        runtime_projection: RuntimeSessionProjection,
    ) -> Self {
        Self {
            layouts: Arc::new(RwLock::new(resources.layouts)),
            layouts_path: resources.layouts_path,
            layout_auto_exclusions: Arc::new(RwLock::new(resources.layout_auto_exclusions)),
            layout_auto_exclusions_path: resources.layout_auto_exclusions_path,
            spatial,
            scenes,
            transactions,
            runtime_state_path,
            runtime_projection,
            driver_host: Arc::new(OnceLock::new()),
            #[cfg(feature = "persistence-test-hooks")]
            test_hooks: LayoutMutationTestHooks::default(),
        }
    }

    #[cfg(feature = "persistence-test-hooks")]
    #[allow(
        clippy::too_many_arguments,
        reason = "the fixture mirrors the production composition boundary"
    )]
    pub fn new_test_context(
        layouts: HashMap<String, SpatialLayout>,
        layouts_path: PathBuf,
        layout_auto_exclusions: layout_auto_exclusions::LayoutAutoExclusionStore,
        layout_auto_exclusions_path: PathBuf,
        spatial: SpatialService,
        scenes: SceneService,
        transactions: SceneTransactionQueue,
        runtime_state_path: PathBuf,
    ) -> Self {
        let (power, _) = watch::channel(crate::session::OutputPowerState::default());
        let projection = RuntimeSessionProjection::new(scenes.clone(), spatial.clone(), power);
        Self::new(
            LayoutContextResources {
                layouts,
                layouts_path,
                layout_auto_exclusions,
                layout_auto_exclusions_path,
            },
            spatial,
            scenes,
            transactions,
            runtime_state_path,
            projection,
        )
    }

    pub(crate) fn bind_driver_host(&self, driver_host: &Arc<DaemonDriverHost>) {
        self.driver_host
            .set(Arc::downgrade(driver_host))
            .expect("layout driver host should bind exactly once");
    }

    fn discovery_runtime(&self) -> DiscoveryRuntime {
        self.driver_host
            .get()
            .and_then(Weak::upgrade)
            .expect("layout driver host should be bound before domain use")
            .discovery_runtime()
    }

    pub(crate) async fn restore_startup_layout(
        &self,
        layout_id: &str,
    ) -> Result<Option<SpatialLayout>, DomainError> {
        let Some(layout) = self.layouts.read().await.get(layout_id).cloned() else {
            return Ok(None);
        };
        let prepared = SpatialEngine::try_new(layout.clone())
            .map_err(|error| DomainError::validation(error.to_string()))?;
        self.spatial.replace(prepared);
        let mut mutation = self.scenes.begin_mutation().await;
        mutation.sync_primary_layout(&layout);
        self.scenes.commit_mutation(mutation).await?;
        Ok(Some(layout))
    }

    pub(crate) async fn acquire_scene_activation_guard(&self) -> SceneActivationGuard {
        self.transactions.acquire_scene_activation_guard().await
    }

    pub(crate) async fn acquire_update_guard(&self) -> LayoutUpdateGuard {
        self.transactions.acquire_layout_update_guard().await
    }

    pub(crate) async fn admit_persisted_update_under_guard(
        &self,
        guard: &LayoutUpdateGuard,
        layout: SpatialLayout,
    ) -> Result<(), LayoutUpdateError> {
        let prepared = PreparedLayoutUpdate::try_new(layout)?;
        let persistence = LayoutPersistenceContext {
            runtime_state_path: self.runtime_state_path.clone(),
            runtime_projection: self.runtime_projection.clone(),
            driver_host: self.driver_host.get().cloned(),
        };
        apply_prepared_layout_update_under_guard_with_persistence(
            self.spatial.clone(),
            self.scenes.clone(),
            self.transactions.clone(),
            guard,
            prepared,
            move |phase| {
                let persistence = persistence.clone();
                async move { persist_layout_runtime_phase(&persistence, phase).await }
            },
        )
        .await
    }

    pub(crate) async fn apply_persisted_update(
        &self,
        guard: LayoutUpdateGuard,
        layout: SpatialLayout,
    ) -> Result<(), LayoutUpdateError> {
        self.admit_persisted_update_under_guard(&guard, layout)
            .await?;
        drop(guard);
        let _ = self.converge_persisted_update().await;
        Ok(())
    }

    pub(crate) async fn converge_persisted_update(&self) -> LayoutPersistenceStatus {
        self.sync_connectivity().await;
        let persistence = LayoutPersistenceContext {
            runtime_state_path: self.runtime_state_path.clone(),
            runtime_projection: self.runtime_projection.clone(),
            driver_host: self.driver_host.get().cloned(),
        };
        match persist_layout_runtime_phase(&persistence, LayoutPersistencePhase::Converge).await {
            LayoutPersistenceOutcome::Written => LayoutPersistenceStatus::Synchronized,
            LayoutPersistenceOutcome::Superseded => {
                tracing::warn!("layout committed but convergence persistence was superseded");
                LayoutPersistenceStatus::Pending
            }
            LayoutPersistenceOutcome::BeforeAdmission(error) => {
                tracing::warn!(%error, "layout committed before convergence persistence was admitted");
                LayoutPersistenceStatus::Pending
            }
            LayoutPersistenceOutcome::RetryArmed(error) => {
                tracing::warn!(%error, "layout committed with convergence persistence retry armed");
                LayoutPersistenceStatus::Pending
            }
        }
    }

    /// Resolve the stable layout identity for a tracked device.
    pub async fn resolved_layout_device_id(&self, device_info: &DeviceInfo) -> String {
        let runtime = self.discovery_runtime();
        if let Some(layout_device_id) = {
            let lifecycle = runtime.lifecycle_manager.lock().await;
            lifecycle
                .layout_device_id_for(device_info.id)
                .map(ToOwned::to_owned)
        } {
            return layout_device_id;
        }

        let fingerprint = runtime
            .device_registry
            .fingerprint_for_id(&device_info.id)
            .await;
        DeviceLifecycleManager::canonical_layout_device_id(device_info, fingerprint.as_ref())
    }

    /// Mint the canonical auto-layout outputs for one connected device.
    pub async fn layout_outputs_for(&self, requested_layout_id: &str) -> Vec<Output> {
        let runtime = self.discovery_runtime();
        let tracked = runtime.device_registry.list().await;
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
            let _minted =
                append_auto_layout_zones_for_device(&mut scratch, &layout_device_id, &device.info);
            return scratch.zones;
        }
        Vec::new()
    }

    /// Resolve native display canvases for every renderable device.
    pub async fn connected_display_surface_layouts(
        &self,
    ) -> Vec<(DeviceId, String, SpatialLayout)> {
        self.discovery_runtime()
            .device_registry
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

    pub(crate) async fn active_layout_targets_enabled_device(
        &self,
        runtime: &DiscoveryRuntime,
        physical_id: DeviceId,
        layout_device_id: &str,
    ) -> bool {
        let candidate_ids = {
            let logical_store = runtime.logical_devices.read().await;
            let mut candidates =
                crate::logical_devices::list_for_physical(&logical_store, physical_id)
                    .into_iter()
                    .filter(|entry| entry.enabled)
                    .map(|entry| entry.id)
                    .collect::<HashSet<_>>();

            if logical_store
                .get(layout_device_id)
                .is_none_or(|entry| entry.enabled)
            {
                candidates.insert(layout_device_id.to_owned());
            }
            candidates
        };

        if self
            .current()
            .zones
            .iter()
            .any(|zone| candidate_ids.contains(&zone.device_id))
        {
            return true;
        }

        self.scenes
            .snapshot()
            .await
            .active_render_groups()
            .iter()
            .flat_map(|group| group.layout.zones.iter())
            .any(|zone| candidate_ids.contains(&zone.device_id))
    }

    /// Re-evaluate device eligibility after scene targeting changes.
    pub async fn sync_connectivity(&self) {
        let runtime = self.discovery_runtime();
        self.sync_discovery_connectivity(runtime, None).await;
    }

    pub(crate) async fn sync_discovery_connectivity(
        &self,
        runtime: DiscoveryRuntime,
        limit_to_devices: Option<HashSet<DeviceId>>,
    ) {
        let context = self.clone();
        if let Err(error) = tokio::spawn(async move {
            context
                .sync_connectivity_workflow(&runtime, limit_to_devices.as_ref())
                .await;
        })
        .await
        {
            tracing::warn!(%error, "layout connectivity workflow failed");
        }
    }

    async fn sync_connectivity_workflow(
        &self,
        runtime: &DiscoveryRuntime,
        limit_to_devices: Option<&HashSet<DeviceId>>,
    ) {
        for tracked in runtime.device_registry.list().await {
            let device_id = tracked.info.id;
            if limit_to_devices.is_some_and(|allowed| !allowed.contains(&device_id)) {
                continue;
            }

            let fingerprint = runtime.device_registry.fingerprint_for_id(&device_id).await;
            let connect_behavior = crate::discovery::desired_connect_behavior(
                runtime,
                device_id,
                &tracked.info,
                fingerprint.as_ref(),
                tracked.connect_behavior,
                tracked.user_settings.enabled,
            )
            .await;
            let actions = {
                let mut lifecycle = runtime.lifecycle_manager.lock().await;
                lifecycle.on_discovered_with_behavior(
                    device_id,
                    &tracked.info,
                    fingerprint.as_ref(),
                    connect_behavior,
                )
            };
            if actions.is_empty() {
                continue;
            }

            crate::discovery::execute_lifecycle_actions(runtime.clone(), actions).await;
            crate::discovery::sync_registry_state(runtime, device_id).await;
        }

        self.sync_active_layout_workflow(runtime, limit_to_devices)
            .await;
    }

    pub(crate) async fn sync_active_layout_for_renderable_devices(
        &self,
        runtime: DiscoveryRuntime,
        limit_to_devices: Option<HashSet<DeviceId>>,
    ) {
        let context = self.clone();
        if let Err(error) = tokio::spawn(async move {
            context
                .sync_active_layout_workflow(&runtime, limit_to_devices.as_ref())
                .await;
        })
        .await
        {
            tracing::warn!(%error, "auto-layout repair workflow failed");
        }
    }

    #[allow(
        clippy::too_many_lines,
        reason = "layout reconciliation keeps the full discovery-driven repair flow in one place"
    )]
    async fn sync_active_layout_workflow(
        &self,
        runtime: &DiscoveryRuntime,
        limit_to_devices: Option<&HashSet<DeviceId>>,
    ) {
        let tracked_devices = runtime.device_registry.list().await;
        let logical_store = runtime.logical_devices.read().await.clone();
        let lifecycle_layout_ids = {
            let lifecycle = runtime.lifecycle_manager.lock().await;
            tracked_devices
                .iter()
                .map(|tracked| {
                    let device_id = tracked.info.id;
                    let layout_id = lifecycle
                        .layout_device_id_for(device_id)
                        .map(ToOwned::to_owned);
                    (device_id, layout_id)
                })
                .collect::<HashMap<_, _>>()
        };
        let mut canonical_layout_ids = HashMap::with_capacity(tracked_devices.len());
        for tracked in &tracked_devices {
            let device_id = tracked.info.id;
            let layout_device_id =
                if let Some(Some(layout_device_id)) = lifecycle_layout_ids.get(&device_id) {
                    layout_device_id.clone()
                } else {
                    let fingerprint = runtime.device_registry.fingerprint_for_id(&device_id).await;
                    DeviceLifecycleManager::canonical_layout_device_id(
                        &tracked.info,
                        fingerprint.as_ref(),
                    )
                };
            canonical_layout_ids.insert(device_id, layout_device_id);
        }

        let guard = self.acquire_update_guard().await;
        let original_layout = self.current();
        let mut layout = original_layout.clone();
        let excluded_layout_device_ids = {
            let exclusion_keys = self.active_auto_exclusion_keys(&layout).await;
            let store = self.layout_auto_exclusions.read().await;
            exclusion_keys
                .iter()
                .filter_map(|key| store.get(key))
                .flat_map(|device_ids| device_ids.iter().cloned())
                .collect::<HashSet<_>>()
        };
        let inactive_ids = {
            let manager = runtime.backend_manager.lock().await;
            manager
                .connected_devices_without_layout_targets(&layout)
                .into_iter()
                .map(|(_, device_id)| device_id)
                .collect::<HashSet<_>>()
        };

        let mut repaired_devices = Vec::new();
        let mut repaired_zone_count = 0_usize;
        for tracked in tracked_devices {
            let device_id = tracked.info.id;
            if !tracked.state.is_renderable()
                || limit_to_devices.is_some_and(|allowed| !allowed.contains(&device_id))
            {
                continue;
            }
            let layout_device_id = canonical_layout_ids
                .get(&device_id)
                .expect("tracked device should have a canonical layout id")
                .clone();
            let default_enabled = logical_store
                .get(&layout_device_id)
                .is_none_or(|entry| entry.enabled);
            if !default_enabled || excluded_layout_device_ids.contains(&layout_device_id) {
                continue;
            }

            let repaired = reconcile_auto_layout_zones_for_device(
                &mut layout,
                &layout_device_id,
                &tracked.info,
            );
            if repaired > 0 {
                repaired_zone_count = repaired_zone_count.saturating_add(repaired);
                repaired_devices.push(format!("{} ({device_id})", tracked.info.name));
            }
            if inactive_ids.contains(&device_id) {
                tracing::debug!(
                    device_id = %device_id,
                    layout_device_id = %layout_device_id,
                    "leaving layout-inactive device unmapped until explicitly targeted"
                );
            }
        }

        if repaired_devices.is_empty() {
            return;
        }

        let prepared = match PreparedLayoutUpdate::try_new(layout.clone()) {
            Ok(prepared) => prepared,
            Err(error) => {
                tracing::warn!(%error, "rejected auto-layout repair before persistence");
                return;
            }
        };
        if let Err(error) = crate::scene_transactions::apply_prepared_layout_update_under_guard(
            self.spatial.clone(),
            self.scenes.clone(),
            self.transactions.clone(),
            &guard,
            prepared,
        )
        .await
        {
            tracing::warn!(%error, "rejected auto-layout repair before persistence");
            return;
        }

        let (previous_saved_layout, snapshot) = {
            let mut layouts = self.layouts.write().await;
            let previous = layouts.insert(layout.id.clone(), layout.clone());
            (previous, layouts.clone())
        };
        if let Err(error) = self.save_catalog_snapshot(snapshot).await {
            let rollback_layout = previous_saved_layout
                .as_ref()
                .cloned()
                .unwrap_or(original_layout);
            let rollback_snapshot = {
                let mut layouts = self.layouts.write().await;
                if let Some(previous) = previous_saved_layout {
                    layouts.insert(layout.id.clone(), previous);
                } else {
                    layouts.remove(&layout.id);
                }
                layouts.clone()
            };
            let layout_store_rollback = self.save_catalog_snapshot(rollback_snapshot).await.err();
            let renderer_rollback = match PreparedLayoutUpdate::try_new(rollback_layout) {
                Ok(prepared) => {
                    crate::scene_transactions::apply_prepared_layout_update_under_guard(
                        self.spatial.clone(),
                        self.scenes.clone(),
                        self.transactions.clone(),
                        &guard,
                        prepared,
                    )
                    .await
                    .err()
                    .map(|error| error.to_string())
                }
                Err(error) => Some(error.to_string()),
            };
            tracing::warn!(
                path = %self.layouts_path.display(),
                %error,
                layout_store_rollback = ?layout_store_rollback,
                renderer_rollback = ?renderer_rollback,
                "failed to persist auto-updated layout store; restored previous layout"
            );
            return;
        }

        tracing::info!(
            layout_id = %layout.id,
            repaired_device_count = repaired_devices.len(),
            repaired_zone_count,
            repaired_devices = ?repaired_devices,
            "reconciled existing auto-layout zones in the active layout"
        );
    }

    async fn active_auto_exclusion_keys(
        &self,
        layout: &SpatialLayout,
    ) -> Vec<layout_auto_exclusions::LayoutAutoExclusionKey> {
        let mut keys = vec![layout_auto_exclusions::LayoutAutoExclusionKey::layout(
            layout.id.as_str(),
        )];
        let manager = self.scenes.snapshot().await;
        if let Some(scene) = manager.active_scene()
            && let Some(group) = scene.primary_zone()
        {
            keys.push(layout_auto_exclusions::LayoutAutoExclusionKey::zone(
                scene.id, group.id,
            ));
        }
        keys
    }

    async fn persist_catalog(&self) -> anyhow::Result<()> {
        let snapshot = self.layouts.read().await.clone();
        self.save_catalog_snapshot(snapshot).await
    }

    async fn save_catalog_snapshot(
        &self,
        snapshot: HashMap<String, SpatialLayout>,
    ) -> anyhow::Result<()> {
        let path = self.layouts_path.clone();
        tokio::task::spawn_blocking(move || crate::layout_store::save(&path, &snapshot))
            .await
            .map_err(|error| anyhow::anyhow!("layout store task failed: {error}"))?
    }

    pub(crate) async fn persist_catalog_best_effort(&self) {
        if let Err(error) = self.persist_catalog().await {
            tracing::warn!(
                path = %self.layouts_path.display(),
                %error,
                "Failed to persist layout store"
            );
        }
    }

    async fn reconcile_layout_auto_exclusions(
        &self,
        layout_id: &str,
        previous_zones: &[Output],
        updated_zones: &[Output],
    ) {
        let changed = {
            let mut exclusions = self.layout_auto_exclusions.write().await;
            let key = layout_auto_exclusions::LayoutAutoExclusionKey::layout(layout_id);
            let current = exclusions.get(&key).cloned().unwrap_or_default();
            let next = layout_auto_exclusions::reconcile_layout_device_exclusions(
                previous_zones,
                updated_zones,
                &current,
            );
            if next == current {
                false
            } else {
                if next.is_empty() {
                    exclusions.remove(&key);
                } else {
                    exclusions.insert(key, next);
                }
                true
            }
        };
        if changed {
            self.persist_layout_auto_exclusions().await;
        }
    }

    async fn remove_layout_auto_exclusions(&self, layout_id: &str) {
        let removed = {
            let mut exclusions = self.layout_auto_exclusions.write().await;
            exclusions
                .remove(&layout_auto_exclusions::LayoutAutoExclusionKey::layout(
                    layout_id,
                ))
                .is_some()
        };
        if removed {
            self.persist_layout_auto_exclusions().await;
        }
    }

    /// Reconcile discovery exclusions after zone layouts change.
    pub async fn reconcile_zone_auto_exclusions(
        &self,
        scene_id: SceneId,
        previous_zones: &[Zone],
        updated_zones: &[Zone],
    ) {
        let context = self.clone();
        let previous_zones = previous_zones.to_vec();
        let updated_zones = updated_zones.to_vec();
        if let Err(error) = tokio::spawn(async move {
            context
                .reconcile_zone_auto_exclusions_workflow(scene_id, &previous_zones, &updated_zones)
                .await;
        })
        .await
        {
            tracing::warn!(%error, "zone layout exclusion reconciliation failed");
        }
    }

    async fn reconcile_zone_auto_exclusions_workflow(
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
        let context = self.clone();
        if let Err(error) = tokio::spawn(async move {
            let removed = {
                let mut exclusions = context.layout_auto_exclusions.write().await;
                exclusions
                    .remove(&layout_auto_exclusions::LayoutAutoExclusionKey::zone(
                        scene_id, zone_id,
                    ))
                    .is_some()
            };
            if removed {
                context.persist_layout_auto_exclusions().await;
            }
        })
        .await
        {
            tracing::warn!(%error, "zone layout exclusion removal failed");
        }
    }

    async fn persist_layout_auto_exclusions(&self) {
        let snapshot = self.layout_auto_exclusions.read().await.clone();
        let path = self.layout_auto_exclusions_path.clone();
        let result =
            tokio::task::spawn_blocking(move || layout_auto_exclusions::save(&path, &snapshot))
                .await;
        if let Err(error) = result
            .map_err(|error| anyhow::anyhow!("layout exclusion store task failed: {error}"))
            .and_then(|result| result)
        {
            tracing::warn!(
                path = %self.layout_auto_exclusions_path.display(),
                %error,
                "Failed to persist layout auto-exclusion store"
            );
        }
    }

    #[cfg(feature = "persistence-test-hooks")]
    /// Create the explicit non-production layout fixture capability.
    pub fn test_fixture(&self) -> LayoutTestFixture<'_> {
        LayoutTestFixture { context: self }
    }

    #[cfg(feature = "persistence-test-hooks")]
    /// Wait at one installed persistence integration barrier.
    pub(crate) async fn wait_test_hook(
        &self,
        point: LayoutMutationTestPoint,
        operation: LayoutMutationTestOperation,
        reference: &str,
    ) {
        self.test_hooks.wait(point, operation, reference).await;
    }

    #[cfg(not(feature = "persistence-test-hooks"))]
    async fn wait_test_hook(
        &self,
        _point: LayoutMutationTestPoint,
        _operation: LayoutMutationTestOperation,
        _reference: &str,
    ) {
    }
}

#[cfg(not(feature = "persistence-test-hooks"))]
#[derive(Clone, Copy)]
enum LayoutMutationTestPoint {
    BeforeGuard,
    AfterMemoryMutation,
    AfterRendererMutation,
    AfterWorkflow,
}

#[cfg(not(feature = "persistence-test-hooks"))]
#[derive(Clone, Copy)]
enum LayoutMutationTestOperation {
    Create,
    Update,
    Apply,
    Preview,
    Delete,
    ConfigResize,
    SimulatorPrune,
}

async fn await_layout_workflow<T>(
    workflow: tokio::task::JoinHandle<Result<T, DomainError>>,
) -> Result<T, DomainError> {
    workflow.await.map_err(|error| {
        DomainError::Internal(anyhow::anyhow!("layout mutation workflow failed: {error}"))
    })?
}

fn layout_summary(layout: &SpatialLayout, is_active: bool) -> LayoutSummary {
    LayoutSummary {
        id: layout.id.clone(),
        name: layout.name.clone(),
        canvas_width: layout.canvas_width,
        canvas_height: layout.canvas_height,
        zone_count: layout.zones.len(),
        is_active,
    }
}

fn resolve_layout_key(
    layouts: &HashMap<String, SpatialLayout>,
    id_or_name: &str,
) -> Result<String, DomainError> {
    if layouts.contains_key(id_or_name) {
        return Ok(id_or_name.to_owned());
    }
    let matches: Vec<String> = layouts
        .iter()
        .filter(|(_, layout)| layout.name.eq_ignore_ascii_case(id_or_name))
        .map(|(id, _)| id.clone())
        .collect();
    match matches.as_slice() {
        [] => Err(DomainError::not_found(ResourceKind::Layout, id_or_name)),
        [key] => Ok(key.clone()),
        _ => Err(DomainError::conflict(format!(
            "Layout name is ambiguous: {id_or_name}"
        ))),
    }
}

fn normalize_layout_name(raw: &str) -> Result<String, DomainError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(DomainError::validation("Layout name must not be empty"));
    }
    Ok(trimmed.to_owned())
}

fn validate_canvas_dimensions(width: u32, height: u32) -> Result<(), DomainError> {
    SurfaceDescriptor::rgba8888(width, height)
        .try_non_empty_byte_len()
        .map(|_| ())
        .map_err(|error| DomainError::validation(error.to_string()))
}

pub(crate) fn validate_layout_sampling_radii(layout: &SpatialLayout) -> Result<(), String> {
    validate_sampling_mode_radii(&layout.default_sampling_mode, "default sampling mode")?;
    for output in &layout.zones {
        validate_output_sampling_radii_text(output)?;
    }
    Ok(())
}

fn validate_output_sampling_radii(output: &Output) -> Result<(), DomainError> {
    validate_output_sampling_radii_text(output).map_err(DomainError::validation)
}

fn validate_output_sampling_radii_text(output: &Output) -> Result<(), String> {
    let Some(mode) = &output.sampling_mode else {
        return Ok(());
    };
    validate_sampling_mode_radii(mode, &format!("sampling mode for output '{}'", output.id))
}

fn validate_sampling_mode_radii(mode: &SamplingMode, field: &str) -> Result<(), String> {
    match mode {
        SamplingMode::AreaAverage { radius_x, radius_y } => {
            for (axis, radius) in [("radius_x", radius_x), ("radius_y", radius_y)] {
                if !radius.is_finite() || *radius < 0.0 {
                    return Err(format!(
                        "{field} {axis} must be finite and greater than or equal to 0"
                    ));
                }
            }
        }
        SamplingMode::GaussianArea { sigma, .. } => {
            if !sigma.is_finite() || *sigma < 0.0 {
                return Err(format!(
                    "{field} sigma must be finite and greater than or equal to 0"
                ));
            }
        }
        SamplingMode::Nearest | SamplingMode::Bilinear => {}
    }
    Ok(())
}

fn empty_default_layout(previous: &SpatialLayout) -> SpatialLayout {
    SpatialLayout {
        id: "default".to_owned(),
        name: "Default Layout".to_owned(),
        description: None,
        canvas_width: previous.canvas_width,
        canvas_height: previous.canvas_height,
        zones: Vec::new(),
        default_sampling_mode: previous.default_sampling_mode.clone(),
        default_edge_behavior: previous.default_edge_behavior,
        spaces: previous.spaces.clone(),
        version: previous.version,
    }
}

fn layout_store_persistence_error(
    action: &str,
    error: anyhow::Error,
    rollback_errors: impl IntoIterator<Item = String>,
) -> DomainError {
    let mut message = format!("failed to persist layout store during {action}: {error}");
    for rollback_error in rollback_errors {
        message.push_str("; ");
        message.push_str(&rollback_error);
    }
    DomainError::Internal(anyhow::anyhow!(message))
}

fn layout_update_domain_error(error: LayoutUpdateError) -> DomainError {
    match &error {
        LayoutUpdateError::SpatialPlan(_)
        | LayoutUpdateError::Transaction(LayoutTransactionRejection::PreparationFailed {
            ..
        }) => DomainError::validation(error.to_string()),
        LayoutUpdateError::Transaction(LayoutTransactionRejection::Superseded)
        | LayoutUpdateError::PersistenceSuperseded => DomainError::conflict(error.to_string()),
        LayoutUpdateError::Transaction(LayoutTransactionRejection::RendererStopped)
        | LayoutUpdateError::Coordinator(_)
        | LayoutUpdateError::Persistence(_)
        | LayoutUpdateError::PersistenceRollback(_) => {
            DomainError::Internal(anyhow::anyhow!(error.to_string()))
        }
    }
}

#[derive(Clone)]
struct LayoutPersistenceContext {
    runtime_state_path: PathBuf,
    runtime_projection: RuntimeSessionProjection,
    driver_host: Option<Weak<DaemonDriverHost>>,
}

async fn persist_layout_runtime_phase(
    context: &LayoutPersistenceContext,
    phase: LayoutPersistencePhase,
) -> LayoutPersistenceOutcome {
    let writer = match AtomicFileWriter::new(&context.runtime_state_path) {
        Ok(writer) => writer,
        Err(error) => return LayoutPersistenceOutcome::BeforeAdmission(error.to_string()),
    };
    let pending = match crate::runtime_state::reserve_save(&context.runtime_state_path) {
        Ok(pending) => pending,
        Err(error) => return LayoutPersistenceOutcome::BeforeAdmission(error.to_string()),
    };
    let requires_durable_completion = matches!(
        &phase,
        LayoutPersistencePhase::Rollback | LayoutPersistencePhase::Converge
    );
    let mut snapshot = context.runtime_projection.snapshot().await;
    if let Some(driver_host) = context.driver_host.as_ref().and_then(Weak::upgrade) {
        driver_host.refresh_driver_inventory().await;
    }
    if let LayoutPersistencePhase::Precommit(candidate) = phase {
        snapshot.active_layout_id = Some(candidate.layout.id);
        if candidate.active_scene_id == Some(SceneId::DEFAULT) {
            snapshot.default_scene_groups = candidate.active_render_groups.to_vec();
        }
    }
    let outcome = match crate::runtime_state::save_reserved(pending, &snapshot) {
        Ok(AtomicWriteOutcome::Written) => LayoutPersistenceOutcome::Written,
        Ok(AtomicWriteOutcome::Superseded) => LayoutPersistenceOutcome::Superseded,
        Err(error @ RuntimeSessionError::Persist { .. }) => {
            LayoutPersistenceOutcome::RetryArmed(error.to_string())
        }
        Err(error) => LayoutPersistenceOutcome::BeforeAdmission(error.to_string()),
    };
    if requires_durable_completion
        && matches!(
            &outcome,
            LayoutPersistenceOutcome::Superseded | LayoutPersistenceOutcome::RetryArmed(_)
        )
    {
        return flush_layout_runtime_persistence(writer).await;
    }
    outcome
}

async fn flush_layout_runtime_persistence(writer: AtomicFileWriter) -> LayoutPersistenceOutcome {
    match tokio::task::spawn_blocking(move || writer.flush(LAYOUT_DURABILITY_TIMEOUT)).await {
        Ok(Ok(_)) => LayoutPersistenceOutcome::Written,
        Ok(Err(error)) => LayoutPersistenceOutcome::RetryArmed(error.to_string()),
        Err(error) => LayoutPersistenceOutcome::RetryArmed(format!(
            "layout persistence flush task failed: {error}"
        )),
    }
}
