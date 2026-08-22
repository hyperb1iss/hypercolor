//! Narrow dependency handles for daemon domain services (Spec 76 §6.4).

use std::path::PathBuf;
use std::sync::Arc;

use hypercolor_core::asset::AssetLibrary;
use hypercolor_core::bus::HypercolorBus;
use hypercolor_core::config::ConfigManager;
use hypercolor_core::effect::EffectRegistry;
use hypercolor_core::engine::RenderLoop;
use hypercolor_core::scene::SceneManager;
use hypercolor_network::DriverModuleRegistry;
use hypercolor_types::device::{DriverModuleKind, DriverTransportKind};
use hypercolor_types::layer::{LayerSource, SceneLayer};
use hypercolor_types::scene::{Scene, SceneId};
use tokio::sync::{RwLock, watch};

use crate::discovery;
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
use crate::runtime_state::{self, RuntimeSessionSnapshot};
use crate::session::{OutputPowerState, current_global_brightness};

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
            layout.clone(),
            output.clone(),
        );
        let scene_library = SceneLibraryContext::new(
            scene.clone(),
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
    projection: RuntimeSessionProjection,
    driver_host: std::sync::Weak<DaemonDriverHost>,
}

#[derive(Clone)]
pub(crate) struct RuntimeSessionProjection {
    scenes: SceneService,
    spatial: SpatialService,
    power: watch::Sender<OutputPowerState>,
}

impl RuntimeSessionProjection {
    pub(crate) fn new(
        scenes: SceneService,
        spatial: SpatialService,
        power: watch::Sender<OutputPowerState>,
    ) -> Self {
        Self {
            scenes,
            spatial,
            power,
        }
    }

    pub(crate) async fn snapshot(&self) -> RuntimeSessionSnapshot {
        let mut snapshot = {
            let manager = self.scenes.snapshot().await;
            runtime_state::snapshot_from_scene_manager(&manager)
        };
        snapshot.active_layout_id = Some(self.spatial.layout().id.clone());
        snapshot.global_brightness = current_global_brightness(&self.power);
        snapshot.manual_paused = self.power.borrow().manually_paused();
        snapshot
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

    /// Persist the current scene store before the runtime-session pointer.
    pub async fn save(&self) {
        let pending_save = match runtime_state::reserve_save(&self.path) {
            Ok(pending_save) => pending_save,
            Err(error) => {
                tracing::warn!(
                    path = %self.path.display(),
                    %error,
                    "Failed to reserve runtime session snapshot"
                );
                return;
            }
        };
        let snapshot = self.snapshot().await;

        if let Err(error) = self.save_scene_store_snapshot().await {
            tracing::warn!(%error, "Failed to persist scene store before runtime snapshot save");
        }

        if let Err(error) = runtime_state::save_reserved(pending_save, &snapshot) {
            tracing::warn!(
                path = %self.path.display(),
                %error,
                "Failed to persist runtime session snapshot"
            );
        }
    }

    async fn save_scene_store_snapshot(&self) -> anyhow::Result<()> {
        self.projection.scenes.save_snapshot().await
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
}

impl SceneContext {
    pub(crate) fn new(
        scenes: SceneService,
        runtime_session: RuntimeSessionService,
        asset_library: Arc<RwLock<AssetLibrary>>,
        config_manager: Option<Arc<ConfigManager>>,
        render_loop: Arc<RwLock<RenderLoop>>,
        layout: LayoutContext,
    ) -> Self {
        Self {
            scenes,
            runtime_session,
            asset_library,
            config_manager,
            render_loop,
            layout,
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

    /// Layout connectivity authority needed after scene targeting changes.
    #[must_use]
    pub const fn layout(&self) -> &LayoutContext {
        &self.layout
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
