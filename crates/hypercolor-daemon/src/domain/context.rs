//! Narrow dependency handles for daemon domain services (Spec 76 §6.4).

use std::path::PathBuf;
use std::sync::Arc;

use hypercolor_core::asset::AssetLibrary;
use hypercolor_core::config::ConfigManager;
use hypercolor_core::engine::RenderLoop;
use hypercolor_core::scene::SceneManager;
use hypercolor_network::DriverModuleRegistry;
use hypercolor_types::layer::{LayerSource, SceneLayer};
use hypercolor_types::scene::{Scene, SceneId, Zone, ZoneId};
use tokio::sync::{RwLock, watch};

use crate::domain::DomainError;
use crate::domain::commit::SceneCommit;
use crate::domain::scene::{
    COMMIT_ATTEMPTS, MEDIA_SOFT_PRODUCER_COST_US, MediaAdmissionContext, SceneMediaAdmission,
    SceneMutation, SceneService,
};
use crate::domain::spatial::SpatialService;
use crate::network::DaemonDriverHost;
use crate::runtime_state::{self, RuntimeSessionSnapshot};
use crate::scene_store::SceneStore;
use crate::session::{OutputPowerState, current_global_brightness};
use crate::zone_layout_preview::ZoneLayoutPreviewStore;
use crate::{discovery, layout_auto_exclusions};

/// Owning runtime-session persistence boundary.
#[derive(Clone)]
pub struct RuntimeSessionService {
    path: PathBuf,
    scenes: SceneService,
    scene_store: Arc<RwLock<SceneStore>>,
    spatial: SpatialService,
    power: watch::Sender<OutputPowerState>,
    driver_host: Arc<DaemonDriverHost>,
    driver_registry: Arc<DriverModuleRegistry>,
}

impl RuntimeSessionService {
    pub(crate) fn new(
        path: PathBuf,
        scenes: SceneService,
        scene_store: Arc<RwLock<SceneStore>>,
        spatial: SpatialService,
        power: watch::Sender<OutputPowerState>,
        driver_host: Arc<DaemonDriverHost>,
        driver_registry: Arc<DriverModuleRegistry>,
    ) -> Self {
        Self {
            path,
            scenes,
            scene_store,
            spatial,
            power,
            driver_host,
            driver_registry,
        }
    }

    /// Capture the complete durable runtime-session projection.
    pub async fn snapshot(&self) -> RuntimeSessionSnapshot {
        let mut snapshot = {
            let manager = self.scenes.snapshot().await;
            runtime_state::snapshot_from_scene_manager(&manager)
        };
        snapshot.active_layout_id = Some(self.spatial.layout().id.clone());
        snapshot.global_brightness = current_global_brightness(&self.power);
        snapshot.manual_paused = self.power.borrow().manually_paused();
        self.driver_host
            .driver_inventory()
            .refresh(self.driver_registry.as_ref(), self.driver_host.as_ref())
            .await;
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
        let pending = {
            let manager = self.scenes.snapshot().await;
            let store = self.scene_store.read().await;
            store.reserve_save(manager.list().into_iter().cloned())?
        };

        self.scene_store
            .write()
            .await
            .save_reserved(pending)
            .map(|_| ())
    }
}

/// Scene transaction authority shared by every transport and daemon worker.
#[derive(Clone)]
pub struct SceneContext {
    scenes: SceneService,
    scene_store: Arc<RwLock<SceneStore>>,
    zone_layout_previews: Arc<ZoneLayoutPreviewStore>,
    runtime_session: RuntimeSessionService,
    asset_library: Arc<RwLock<AssetLibrary>>,
    config_manager: Option<Arc<ConfigManager>>,
    render_loop: Arc<RwLock<RenderLoop>>,
    devices: DeviceContext,
}

impl SceneContext {
    pub(crate) fn new(
        scenes: SceneService,
        scene_store: Arc<RwLock<SceneStore>>,
        zone_layout_previews: Arc<ZoneLayoutPreviewStore>,
        runtime_session: RuntimeSessionService,
        asset_library: Arc<RwLock<AssetLibrary>>,
        config_manager: Option<Arc<ConfigManager>>,
        render_loop: Arc<RwLock<RenderLoop>>,
        devices: DeviceContext,
    ) -> Self {
        Self {
            scenes,
            scene_store,
            zone_layout_previews,
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

    /// Start an owned candidate mutation.
    pub async fn begin_mutation(&self) -> SceneMutation {
        self.scenes.begin_mutation().await
    }

    /// Commit an owned candidate through the one ordered scene authority.
    pub async fn commit(&self, mutation: SceneMutation) -> Result<SceneCommit, DomainError> {
        self.scenes
            .commit_mutation(
                self.scene_store.as_ref(),
                self.zone_layout_previews.as_ref(),
                mutation,
            )
            .await
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
    driver_host: Arc<DaemonDriverHost>,
    layout_auto_exclusions: Arc<RwLock<layout_auto_exclusions::LayoutAutoExclusionStore>>,
    layout_auto_exclusions_path: PathBuf,
}

impl DeviceContext {
    pub(crate) fn new(
        driver_host: Arc<DaemonDriverHost>,
        layout_auto_exclusions: Arc<RwLock<layout_auto_exclusions::LayoutAutoExclusionStore>>,
        layout_auto_exclusions_path: PathBuf,
    ) -> Self {
        Self {
            driver_host,
            layout_auto_exclusions,
            layout_auto_exclusions_path,
        }
    }

    /// Re-evaluate device eligibility after scene targeting changes.
    pub async fn sync_connectivity(&self) {
        let runtime = self.driver_host.discovery_runtime();
        discovery::sync_active_layout_connectivity(&runtime, None).await;
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
