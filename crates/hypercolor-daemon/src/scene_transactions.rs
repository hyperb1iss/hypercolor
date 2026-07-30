use std::collections::VecDeque;
use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};

use thiserror::Error;
use tokio::sync::{Mutex, OwnedMutexGuard, RwLock, oneshot};

use hypercolor_core::scene::SceneManager;
use hypercolor_core::spatial::{SpatialEngine, SpatialPlanError};
use hypercolor_types::scene::{SceneId, UnassignedBehavior, Zone};
use hypercolor_types::spatial::SpatialLayout;

#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum LayoutTransactionRejection {
    #[error("render pipeline stopped before applying the layout")]
    RendererStopped,
    #[error("render pipeline rejected the layout: {message}")]
    PreparationFailed { message: String },
}

#[derive(Debug, Error)]
pub enum LayoutUpdateError {
    #[error(transparent)]
    SpatialPlan(#[from] SpatialPlanError),
    #[error(transparent)]
    Transaction(#[from] LayoutTransactionRejection),
    #[error("layout update coordinator failed: {0}")]
    Coordinator(String),
    #[error("layout persistence failed: {0}")]
    Persistence(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LayoutTransactionToken(u64);

#[derive(Debug, Clone)]
pub struct PreparedLayoutUpdate {
    spatial_engine: SpatialEngine,
}

impl PreparedLayoutUpdate {
    pub fn try_new(layout: SpatialLayout) -> Result<Self, SpatialPlanError> {
        Ok(Self {
            spatial_engine: SpatialEngine::try_new(layout)?,
        })
    }

    #[must_use]
    pub fn spatial_engine(&self) -> &SpatialEngine {
        &self.spatial_engine
    }

    fn into_spatial_engine(self) -> SpatialEngine {
        self.spatial_engine
    }
}

#[derive(Debug)]
pub struct PrepareLayoutTransaction {
    token: LayoutTransactionToken,
    spatial_engine: SpatialEngine,
    active_scene_id: Option<SceneId>,
    active_render_groups: Arc<[Zone]>,
    active_render_groups_revision: u64,
    unassigned_behavior: UnassignedBehavior,
    acknowledgment: oneshot::Sender<Result<(), LayoutTransactionRejection>>,
}

impl PrepareLayoutTransaction {
    #[must_use]
    pub const fn token(&self) -> LayoutTransactionToken {
        self.token
    }

    #[must_use]
    pub fn spatial_engine(&self) -> &SpatialEngine {
        &self.spatial_engine
    }

    #[must_use]
    pub fn active_scene_id(&self) -> Option<SceneId> {
        self.active_scene_id
    }

    #[must_use]
    pub fn active_render_groups(&self) -> Arc<[Zone]> {
        Arc::clone(&self.active_render_groups)
    }

    #[must_use]
    pub const fn active_render_groups_revision(&self) -> u64 {
        self.active_render_groups_revision
    }

    #[must_use]
    pub fn unassigned_behavior(&self) -> &UnassignedBehavior {
        &self.unassigned_behavior
    }

    pub(crate) fn is_cancelled(&self) -> bool {
        self.acknowledgment.is_closed()
    }

    #[doc(hidden)]
    pub fn accept(self) {
        let _ = self.acknowledgment.send(Ok(()));
    }

    #[doc(hidden)]
    pub fn reject(self, rejection: LayoutTransactionRejection) {
        let _ = self.acknowledgment.send(Err(rejection));
    }
}

#[derive(Debug)]
pub struct ResolveLayoutTransaction {
    token: LayoutTransactionToken,
    acknowledgment: oneshot::Sender<Result<(), LayoutTransactionRejection>>,
}

impl ResolveLayoutTransaction {
    #[must_use]
    pub const fn token(&self) -> LayoutTransactionToken {
        self.token
    }

    #[doc(hidden)]
    pub fn accept(self) {
        let _ = self.acknowledgment.send(Ok(()));
    }

    #[doc(hidden)]
    pub fn reject(self, rejection: LayoutTransactionRejection) {
        let _ = self.acknowledgment.send(Err(rejection));
    }
}

#[derive(Debug)]
pub enum SceneTransaction {
    PrepareLayout(PrepareLayoutTransaction),
    CommitLayout(ResolveLayoutTransaction),
    AbortLayout(ResolveLayoutTransaction),
    SetScreenCaptureConfigured(bool),
}

#[derive(Default)]
struct SceneTransactionQueueState {
    pending: VecDeque<SceneTransaction>,
    closed: bool,
}

#[derive(Clone, Default)]
pub struct SceneTransactionQueue {
    inner: Arc<StdMutex<SceneTransactionQueueState>>,
    layout_update_lock: Arc<Mutex<()>>,
    next_layout_token: Arc<AtomicU64>,
}

pub struct LayoutUpdateGuard {
    guard: Arc<OwnedMutexGuard<()>>,
}

impl Clone for LayoutUpdateGuard {
    fn clone(&self) -> Self {
        Self {
            guard: Arc::clone(&self.guard),
        }
    }
}

pub struct SceneTransactionConsumer {
    queue: SceneTransactionQueue,
}

impl Drop for SceneTransactionConsumer {
    fn drop(&mut self) {
        self.queue.close();
    }
}

impl SceneTransactionQueue {
    pub fn push(&self, transaction: SceneTransaction) -> Result<(), LayoutTransactionRejection> {
        let mut state = self
            .inner
            .lock()
            .expect("scene transaction queue should lock");
        if state.closed {
            return Err(LayoutTransactionRejection::RendererStopped);
        }
        state.pending.push_back(transaction);
        Ok(())
    }

    fn submit_layout_preparation(
        &self,
        token: LayoutTransactionToken,
        spatial_engine: SpatialEngine,
        active_scene_id: Option<SceneId>,
        active_render_groups: Arc<[Zone]>,
        active_render_groups_revision: u64,
        unassigned_behavior: UnassignedBehavior,
    ) -> Result<LayoutTransactionReceipt, LayoutTransactionRejection> {
        let (acknowledgment, receipt) = oneshot::channel();
        self.push(SceneTransaction::PrepareLayout(PrepareLayoutTransaction {
            token,
            spatial_engine,
            active_scene_id,
            active_render_groups,
            active_render_groups_revision,
            unassigned_behavior,
            acknowledgment,
        }))?;
        Ok(LayoutTransactionReceipt(receipt))
    }

    fn submit_layout_resolution(
        &self,
        token: LayoutTransactionToken,
        commit: bool,
    ) -> Result<LayoutTransactionReceipt, LayoutTransactionRejection> {
        let (acknowledgment, receipt) = oneshot::channel();
        let transaction = ResolveLayoutTransaction {
            token,
            acknowledgment,
        };
        self.push(if commit {
            SceneTransaction::CommitLayout(transaction)
        } else {
            SceneTransaction::AbortLayout(transaction)
        })?;
        Ok(LayoutTransactionReceipt(receipt))
    }

    fn next_layout_token(&self) -> LayoutTransactionToken {
        LayoutTransactionToken(
            self.next_layout_token
                .fetch_add(1, Ordering::Relaxed)
                .saturating_add(1),
        )
    }

    #[must_use]
    pub fn drain(&self) -> Vec<SceneTransaction> {
        self.inner
            .lock()
            .expect("scene transaction queue should lock")
            .pending
            .drain(..)
            .collect()
    }

    #[must_use]
    pub fn consumer(&self) -> SceneTransactionConsumer {
        SceneTransactionConsumer {
            queue: self.clone(),
        }
    }

    pub async fn acquire_layout_update_guard(&self) -> LayoutUpdateGuard {
        LayoutUpdateGuard {
            guard: Arc::new(Arc::clone(&self.layout_update_lock).lock_owned().await),
        }
    }

    pub fn close(&self) {
        let pending = {
            let mut state = self
                .inner
                .lock()
                .expect("scene transaction queue should lock");
            state.closed = true;
            state.pending.drain(..).collect::<Vec<_>>()
        };
        for transaction in pending {
            match transaction {
                SceneTransaction::PrepareLayout(transaction) => {
                    transaction.reject(LayoutTransactionRejection::RendererStopped);
                }
                SceneTransaction::CommitLayout(transaction)
                | SceneTransaction::AbortLayout(transaction) => {
                    transaction.reject(LayoutTransactionRejection::RendererStopped);
                }
                SceneTransaction::SetScreenCaptureConfigured(_) => {}
            }
        }
    }

    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.inner
            .lock()
            .expect("scene transaction queue should lock")
            .closed
    }

    #[doc(hidden)]
    #[must_use]
    pub fn pending_len(&self) -> usize {
        self.inner
            .lock()
            .expect("scene transaction queue should lock")
            .pending
            .len()
    }
}

struct LayoutTransactionReceipt(oneshot::Receiver<Result<(), LayoutTransactionRejection>>);

impl LayoutTransactionReceipt {
    async fn wait(self) -> Result<(), LayoutTransactionRejection> {
        self.0
            .await
            .unwrap_or(Err(LayoutTransactionRejection::RendererStopped))
    }
}

pub async fn apply_prepared_layout_update_under_guard(
    spatial_engine: Arc<RwLock<SpatialEngine>>,
    scene_manager: Arc<RwLock<SceneManager>>,
    scene_transactions: SceneTransactionQueue,
    guard: &LayoutUpdateGuard,
    prepared: PreparedLayoutUpdate,
) -> Result<(), LayoutUpdateError> {
    apply_prepared_layout_update_under_guard_with_persistence(
        spatial_engine,
        scene_manager,
        scene_transactions,
        guard,
        prepared,
        |_| async { Ok(()) },
    )
    .await
}

pub async fn apply_prepared_layout_update_under_guard_with_persistence<F, Fut>(
    spatial_engine: Arc<RwLock<SpatialEngine>>,
    scene_manager: Arc<RwLock<SceneManager>>,
    scene_transactions: SceneTransactionQueue,
    guard: &LayoutUpdateGuard,
    prepared: PreparedLayoutUpdate,
    persist: F,
) -> Result<(), LayoutUpdateError>
where
    F: FnOnce(SpatialLayout) -> Fut + Send + 'static,
    Fut: Future<Output = anyhow::Result<()>> + Send + 'static,
{
    let retained_guard = guard.clone();
    tokio::spawn(async move {
        let retained_guard = retained_guard;
        let mut manager = scene_manager.write().await;
        let mut authoritative_spatial_engine = spatial_engine.write().await;
        let prepared_engine = prepared.into_spatial_engine();
        let (active_render_groups, active_render_groups_revision) =
            manager.active_render_groups_for_primary_layout(prepared_engine.layout().as_ref());
        let active_scene_id = manager.active_scene_id().copied();
        let unassigned_behavior = manager
            .active_scene()
            .map(|scene| scene.unassigned_behavior.clone())
            .unwrap_or_default();
        let token = scene_transactions.next_layout_token();
        let receipt = scene_transactions.submit_layout_preparation(
            token,
            prepared_engine.clone(),
            active_scene_id,
            active_render_groups,
            active_render_groups_revision,
            unassigned_behavior,
        )?;
        receipt.wait().await?;
        if let Err(error) = persist(prepared_engine.layout().as_ref().clone()).await {
            if let Ok(receipt) = scene_transactions.submit_layout_resolution(token, false) {
                let _ = receipt.wait().await;
            }
            return Err(LayoutUpdateError::Persistence(error.to_string()));
        }
        let commit = scene_transactions.submit_layout_resolution(token, true)?;
        if let Err(error) = commit.wait().await {
            if let Ok(receipt) = scene_transactions.submit_layout_resolution(token, false) {
                let _ = receipt.wait().await;
            }
            return Err(error.into());
        }
        manager.sync_primary_group_layout(prepared_engine.layout().as_ref());
        *authoritative_spatial_engine = prepared_engine;
        drop(retained_guard);
        Ok::<(), LayoutUpdateError>(())
    })
    .await
    .map_err(|error| LayoutUpdateError::Coordinator(error.to_string()))?
}

pub async fn apply_layout_update(
    spatial_engine: &Arc<RwLock<SpatialEngine>>,
    scene_manager: &Arc<RwLock<SceneManager>>,
    scene_transactions: &SceneTransactionQueue,
    layout: SpatialLayout,
) -> Result<(), LayoutUpdateError> {
    let guard = scene_transactions.acquire_layout_update_guard().await;
    let prepared = PreparedLayoutUpdate::try_new(layout)?;
    apply_prepared_layout_update_under_guard(
        Arc::clone(spatial_engine),
        Arc::clone(scene_manager),
        scene_transactions.clone(),
        &guard,
        prepared,
    )
    .await
}

#[cfg(test)]
mod tests;
