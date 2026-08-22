//! Spatial layout ownership and durable activation workflow.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use hypercolor_types::identity::LayoutId;
use hypercolor_types::scene::SceneId;
use hypercolor_types::spatial::SpatialLayout;
use tokio::sync::RwLock;

use crate::domain::context::{DeviceContext, RuntimeSessionService};
use crate::domain::scene::SceneService;
use crate::domain::spatial::SpatialService;
use crate::persistence::{AtomicFileWriter, AtomicWriteOutcome};
use crate::runtime_state::RuntimeSessionError;
use crate::scene_transactions::{
    LayoutPersistenceOutcome, LayoutPersistencePhase, LayoutUpdateError, LayoutUpdateGuard,
    PreparedLayoutUpdate, SceneActivationGuard, SceneTransactionQueue,
    apply_prepared_layout_update_under_guard_with_persistence,
};

const LAYOUT_DURABILITY_TIMEOUT: Duration = Duration::from_secs(5);

/// Result of converging a committed layout's runtime snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LayoutPersistenceStatus {
    Synchronized,
    Pending,
}

/// Layout catalog, activation ordering, publication, and durability authority.
#[derive(Clone)]
pub struct LayoutContext {
    layouts: Arc<RwLock<HashMap<String, SpatialLayout>>>,
    spatial: SpatialService,
    scenes: SceneService,
    transactions: SceneTransactionQueue,
    runtime_state_path: PathBuf,
    runtime_session: RuntimeSessionService,
    devices: DeviceContext,
}

impl LayoutContext {
    pub(crate) fn new(
        layouts: Arc<RwLock<HashMap<String, SpatialLayout>>>,
        spatial: SpatialService,
        scenes: SceneService,
        transactions: SceneTransactionQueue,
        runtime_state_path: PathBuf,
        runtime_session: RuntimeSessionService,
        devices: DeviceContext,
    ) -> Self {
        Self {
            layouts,
            spatial,
            scenes,
            transactions,
            runtime_state_path,
            runtime_session,
            devices,
        }
    }

    /// Resolve one persisted layout by canonical id.
    pub async fn get(&self, layout_id: &LayoutId) -> Option<SpatialLayout> {
        self.layouts.read().await.get(layout_id.as_str()).cloned()
    }

    /// Capture the currently published layout.
    #[must_use]
    pub fn current(&self) -> SpatialLayout {
        self.spatial.layout().as_ref().clone()
    }

    /// Resolve the current layout id for scene snapshots.
    ///
    /// # Errors
    ///
    /// Returns an error when the active layout carries an invalid id.
    pub fn active_layout_id(&self) -> Result<LayoutId, crate::domain::DomainError> {
        LayoutId::new(self.current().id).map_err(|error| {
            crate::domain::DomainError::Internal(anyhow::anyhow!(
                "active layout has an invalid id: {error}"
            ))
        })
    }

    /// Serialize scene activation against every competing activation.
    pub async fn acquire_scene_activation_guard(&self) -> SceneActivationGuard {
        self.transactions.acquire_scene_activation_guard().await
    }

    /// Serialize a layout change against activation and layout writers.
    pub async fn acquire_update_guard(&self) -> LayoutUpdateGuard {
        self.transactions.acquire_layout_update_guard().await
    }

    /// Admit a prepared layout while the caller retains the layout guard.
    pub(crate) async fn admit_persisted_update_under_guard(
        &self,
        guard: &LayoutUpdateGuard,
        layout: SpatialLayout,
    ) -> Result<(), LayoutUpdateError> {
        let prepared = PreparedLayoutUpdate::try_new(layout)?;
        let persistence = LayoutPersistenceContext {
            runtime_state_path: self.runtime_state_path.clone(),
            runtime_session: self.runtime_session.clone(),
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

    /// Admit a layout, release its guard, then converge connectivity and durability.
    pub async fn apply_persisted_update(
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

    /// Reconcile device connectivity and the runtime snapshot after activation.
    pub(crate) async fn converge_persisted_update(&self) -> LayoutPersistenceStatus {
        self.devices.sync_connectivity().await;
        let persistence = LayoutPersistenceContext {
            runtime_state_path: self.runtime_state_path.clone(),
            runtime_session: self.runtime_session.clone(),
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
}

#[derive(Clone)]
struct LayoutPersistenceContext {
    runtime_state_path: PathBuf,
    runtime_session: RuntimeSessionService,
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
    let mut snapshot = context.runtime_session.snapshot().await;
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
