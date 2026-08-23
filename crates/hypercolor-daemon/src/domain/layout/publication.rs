use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use hypercolor_core::spatial::SpatialEngine;
use hypercolor_types::scene::SceneId;
use hypercolor_types::spatial::SpatialLayout;

use crate::domain::DomainError;
use crate::domain::context::{RuntimeSessionPersistenceError, RuntimeSessionProjection};
use crate::domain::scene::SceneService;
use crate::domain::spatial::SpatialService;
use crate::network::DaemonDriverHost;
use crate::persistence::AtomicWriteOutcome;
#[cfg(feature = "persistence-test-hooks")]
use crate::scene_transactions::LayoutPublicationTestExecutor;
use crate::scene_transactions::{
    LayoutPersistenceOutcome, LayoutPersistencePhase, LayoutTransactionAuthority,
    LayoutUpdateError, LayoutUpdateGuard, SceneActivationGuard, SceneTransactionQueue,
};

use super::LayoutPersistenceStatus;

const LAYOUT_DURABILITY_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone)]
pub(super) struct LayoutPublication {
    spatial: SpatialService,
    scenes: SceneService,
    transactions: LayoutTransactionAuthority,
    runtime_state_path: PathBuf,
    runtime_projection: RuntimeSessionProjection,
}

impl LayoutPublication {
    pub(super) fn new(
        spatial: SpatialService,
        scenes: SceneService,
        transactions: SceneTransactionQueue,
        runtime_state_path: PathBuf,
        runtime_projection: RuntimeSessionProjection,
    ) -> Self {
        Self {
            transactions: LayoutTransactionAuthority::new(
                spatial.clone(),
                scenes.clone(),
                transactions,
            ),
            spatial,
            scenes,
            runtime_state_path,
            runtime_projection,
        }
    }

    pub(super) fn current(&self) -> SpatialLayout {
        self.spatial.layout().as_ref().clone()
    }

    pub(super) fn scenes(&self) -> &SceneService {
        &self.scenes
    }

    #[cfg(feature = "persistence-test-hooks")]
    pub(super) fn test_executor(&self) -> LayoutPublicationTestExecutor {
        self.transactions.test_executor()
    }

    #[cfg(feature = "persistence-test-hooks")]
    pub(super) fn replace_current(&self, layout: SpatialLayout) {
        let spatial = SpatialEngine::try_new(layout)
            .expect("layout fixture should receive a valid spatial layout");
        self.spatial.replace(spatial);
    }

    #[cfg(feature = "persistence-test-hooks")]
    pub(super) async fn active_primary_ids(&self) -> (SceneId, hypercolor_types::scene::ZoneId) {
        let scenes = self.scenes.snapshot().await;
        let scene = scenes
            .active_scene()
            .expect("layout fixture should have an active scene");
        let zone = scene
            .primary_zone()
            .expect("layout fixture active scene should have a primary zone");
        (scene.id, zone.id)
    }

    pub(super) async fn restore_startup_layout(
        &self,
        layout: SpatialLayout,
    ) -> Result<(), DomainError> {
        let prepared = SpatialEngine::try_new(layout.clone())
            .map_err(|error| DomainError::validation(error.to_string()))?;
        self.spatial.replace(prepared);
        let mut mutation = self.scenes.begin_mutation().await;
        mutation.sync_primary_layout(&layout);
        self.scenes.commit_mutation(mutation).await?;
        Ok(())
    }

    pub(super) async fn acquire_scene_activation_guard(&self) -> SceneActivationGuard {
        self.transactions.acquire_scene_activation_guard().await
    }

    pub(super) async fn acquire_update_guard(&self) -> LayoutUpdateGuard {
        self.transactions.acquire_layout_update_guard().await
    }

    pub(super) async fn apply_prepared_under_guard(
        &self,
        guard: &LayoutUpdateGuard,
        layout: SpatialLayout,
    ) -> Result<(), LayoutUpdateError> {
        self.transactions.apply_under_guard(guard, layout).await
    }

    pub(super) async fn admit_persisted_under_guard(
        &self,
        guard: &LayoutUpdateGuard,
        layout: SpatialLayout,
        driver_host: Arc<DaemonDriverHost>,
    ) -> Result<(), LayoutUpdateError> {
        let persistence = self.persistence_context(driver_host);
        self.transactions
            .apply_under_guard_with_persistence(guard, layout, move |phase| {
                let persistence = persistence.clone();
                async move { persist_layout_runtime_phase(&persistence, phase).await }
            })
            .await
    }

    pub(super) async fn persist_convergence(
        &self,
        driver_host: Arc<DaemonDriverHost>,
    ) -> LayoutPersistenceStatus {
        let persistence = self.persistence_context(driver_host);
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

    fn persistence_context(&self, driver_host: Arc<DaemonDriverHost>) -> LayoutPersistenceContext {
        LayoutPersistenceContext {
            runtime_state_path: self.runtime_state_path.clone(),
            runtime_projection: self.runtime_projection.clone(),
            driver_host,
        }
    }
}

#[derive(Clone)]
struct LayoutPersistenceContext {
    runtime_state_path: PathBuf,
    runtime_projection: RuntimeSessionProjection,
    driver_host: Arc<DaemonDriverHost>,
}

async fn persist_layout_runtime_phase(
    context: &LayoutPersistenceContext,
    phase: LayoutPersistencePhase,
) -> LayoutPersistenceOutcome {
    let requires_durable_completion = matches!(
        &phase,
        LayoutPersistencePhase::Rollback | LayoutPersistencePhase::Converge
    );
    let driver_host = Arc::clone(&context.driver_host);
    let outcome = match context
        .runtime_projection
        .persist_snapshot_with(
            &context.runtime_state_path,
            async move {
                driver_host.refresh_driver_inventory().await;
            },
            move |snapshot| {
                if let LayoutPersistencePhase::Precommit(candidate) = phase {
                    snapshot.active_layout_id = Some(candidate.layout.id);
                    if candidate.active_scene_id == Some(SceneId::DEFAULT) {
                        snapshot.default_scene_groups = candidate.resolved_zones.to_vec();
                    }
                }
            },
        )
        .await
    {
        Ok(AtomicWriteOutcome::Written) => LayoutPersistenceOutcome::Written,
        Ok(AtomicWriteOutcome::Superseded) => LayoutPersistenceOutcome::Superseded,
        Err(RuntimeSessionPersistenceError::RetryArmed(error)) => {
            LayoutPersistenceOutcome::RetryArmed(error.to_string())
        }
        Err(RuntimeSessionPersistenceError::BeforeAdmission(error)) => {
            LayoutPersistenceOutcome::BeforeAdmission(error.to_string())
        }
    };
    if requires_durable_completion
        && matches!(
            &outcome,
            LayoutPersistenceOutcome::Superseded | LayoutPersistenceOutcome::RetryArmed(_)
        )
    {
        return flush_layout_runtime_persistence(context).await;
    }
    outcome
}

async fn flush_layout_runtime_persistence(
    context: &LayoutPersistenceContext,
) -> LayoutPersistenceOutcome {
    match context
        .runtime_projection
        .flush_persistence(&context.runtime_state_path, LAYOUT_DURABILITY_TIMEOUT)
        .await
    {
        Ok(()) => LayoutPersistenceOutcome::Written,
        Err(error) => LayoutPersistenceOutcome::RetryArmed(error.to_string()),
    }
}
