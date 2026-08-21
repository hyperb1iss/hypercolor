//! Narrow dependency handles for daemon domain services (Spec 76 §6.4).

use std::path::PathBuf;
use std::sync::Arc;

use hypercolor_core::scene::SceneManager;
use hypercolor_network::DriverModuleRegistry;
use tokio::sync::{RwLock, watch};

use crate::domain::DomainError;
use crate::domain::commit::SceneCommit;
use crate::domain::scene::{COMMIT_ATTEMPTS, SceneMutation, SceneService};
use crate::domain::spatial::SpatialService;
use crate::network::DaemonDriverHost;
use crate::runtime_state::{self, RuntimeSessionSnapshot};
use crate::scene_store::SceneStore;
use crate::session::{OutputPowerState, current_global_brightness};
use crate::zone_layout_preview::ZoneLayoutPreviewStore;

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
}

impl SceneContext {
    pub(crate) fn new(
        scenes: SceneService,
        scene_store: Arc<RwLock<SceneStore>>,
        zone_layout_previews: Arc<ZoneLayoutPreviewStore>,
        runtime_session: RuntimeSessionService,
    ) -> Self {
        Self {
            scenes,
            scene_store,
            zone_layout_previews,
            runtime_session,
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
}
