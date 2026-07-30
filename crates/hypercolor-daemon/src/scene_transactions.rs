use std::collections::VecDeque;
use std::sync::{Arc, Mutex as StdMutex};

use thiserror::Error;
use tokio::sync::{Mutex, OwnedMutexGuard, RwLock, oneshot};

use hypercolor_core::scene::SceneManager;
use hypercolor_core::spatial::{SpatialEngine, SpatialPlanError};
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
}

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
pub struct ApplyLayoutTransaction {
    spatial_engine: SpatialEngine,
    acknowledgment: oneshot::Sender<Result<(), LayoutTransactionRejection>>,
}

impl ApplyLayoutTransaction {
    #[must_use]
    pub fn spatial_engine(&self) -> &SpatialEngine {
        &self.spatial_engine
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
pub enum SceneTransaction {
    ApplyLayout(ApplyLayoutTransaction),
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

    fn submit_layout(
        &self,
        spatial_engine: SpatialEngine,
    ) -> Result<LayoutTransactionReceipt, LayoutTransactionRejection> {
        let (acknowledgment, receipt) = oneshot::channel();
        self.push(SceneTransaction::ApplyLayout(ApplyLayoutTransaction {
            spatial_engine,
            acknowledgment,
        }))?;
        Ok(LayoutTransactionReceipt(receipt))
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
            if let SceneTransaction::ApplyLayout(transaction) = transaction {
                transaction.reject(LayoutTransactionRejection::RendererStopped);
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
    let retained_guard = guard.clone();
    tokio::spawn(async move {
        let retained_guard = retained_guard;
        let mut manager = scene_manager.write().await;
        let mut authoritative_spatial_engine = spatial_engine.write().await;
        let prepared_engine = prepared.into_spatial_engine();
        let receipt = scene_transactions.submit_layout(prepared_engine.clone())?;
        receipt.wait().await?;
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
