use std::collections::VecDeque;
use std::future::Future;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex as StdMutex};

use thiserror::Error;
use tokio::sync::{Mutex, OwnedMutexGuard, oneshot};

use hypercolor_core::spatial::{SpatialEngine, SpatialPlanError};
use hypercolor_types::scene::{SceneId, UnassignedBehavior, Zone};
use hypercolor_types::spatial::SpatialLayout;

use crate::domain::scene::SceneService;
use crate::domain::spatial::SpatialService;

#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum LayoutTransactionRejection {
    #[error("render pipeline stopped before applying the layout")]
    RendererStopped,
    #[error("layout activation was superseded by a newer control-plane state")]
    Superseded,
    #[error("render pipeline rejected the layout: {message}")]
    PreparationFailed { message: String },
}

#[derive(Debug, Error)]
pub(crate) enum LayoutUpdateError {
    #[error(transparent)]
    SpatialPlan(#[from] SpatialPlanError),
    #[error(transparent)]
    Transaction(#[from] LayoutTransactionRejection),
    #[error("layout update coordinator failed: {0}")]
    Coordinator(String),
    #[error("layout persistence failed: {0}")]
    Persistence(String),
    #[error("layout persistence was superseded before renderer activation")]
    PersistenceSuperseded,
    #[error("layout rollback persistence failed: {0}")]
    PersistenceRollback(String),
}

#[derive(Debug, Clone)]
struct PreparedLayoutUpdate {
    spatial_engine: SpatialEngine,
}

#[derive(Debug, Clone)]
pub(crate) struct LayoutPersistenceState {
    pub(crate) layout: SpatialLayout,
    pub(crate) active_scene_id: Option<SceneId>,
    pub(crate) resolved_zones: Arc<[Zone]>,
}

#[derive(Debug, Clone)]
pub(crate) enum LayoutPersistencePhase {
    Precommit(LayoutPersistenceState),
    Rollback,
    Converge,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum LayoutPersistenceOutcome {
    Written,
    Superseded,
    BeforeAdmission(String),
    RetryArmed(String),
}

impl PreparedLayoutUpdate {
    pub(crate) fn try_new(layout: SpatialLayout) -> Result<Self, SpatialPlanError> {
        Ok(Self {
            spatial_engine: SpatialEngine::try_new(layout)?,
        })
    }

    fn into_spatial_engine(self) -> SpatialEngine {
        self.spatial_engine
    }
}

#[derive(Debug)]
pub(crate) struct PrepareLayoutTransaction {
    spatial_engine: SpatialEngine,
    expected_layout: SpatialLayout,
    active_scene_id: Option<SceneId>,
    resolved_zones: Arc<[Zone]>,
    source_resolved_zones_revision: u64,
    resolved_zones_revision: u64,
    unassigned_behavior: UnassignedBehavior,
    activation: LayoutActivationControl,
    acknowledgment: oneshot::Sender<Result<(), LayoutTransactionRejection>>,
}

impl PrepareLayoutTransaction {
    #[must_use]
    pub(crate) fn spatial_engine(&self) -> &SpatialEngine {
        &self.spatial_engine
    }

    #[must_use]
    pub(crate) fn expected_layout(&self) -> &SpatialLayout {
        &self.expected_layout
    }

    #[must_use]
    pub(crate) fn active_scene_id(&self) -> Option<SceneId> {
        self.active_scene_id
    }

    #[must_use]
    pub(crate) fn resolved_zones(&self) -> Arc<[Zone]> {
        Arc::clone(&self.resolved_zones)
    }

    #[must_use]
    pub(crate) const fn source_resolved_zones_revision(&self) -> u64 {
        self.source_resolved_zones_revision
    }

    #[must_use]
    pub(crate) const fn resolved_zones_revision(&self) -> u64 {
        self.resolved_zones_revision
    }

    #[must_use]
    pub(crate) fn unassigned_behavior(&self) -> &UnassignedBehavior {
        &self.unassigned_behavior
    }

    pub(crate) fn activation(&self) -> LayoutActivationControl {
        self.activation.clone()
    }

    pub(crate) fn is_cancelled(&self) -> bool {
        self.acknowledgment.is_closed()
    }

    pub(crate) fn accept(self) {
        let _ = self.acknowledgment.send(Ok(()));
    }

    #[cfg(feature = "persistence-test-hooks")]
    async fn accept_and_publish<F, Fut>(
        self,
        spatial_engine: &SpatialService,
        scene_manager: &SceneService,
        before_publication: F,
    ) -> Result<(), LayoutTransactionRejection>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = ()>,
    {
        let activation = self.activation();
        let candidate_spatial_engine = self.spatial_engine.clone();
        let expected_layout = self.expected_layout.clone();
        let expected_active_scene_id = self.active_scene_id;
        let expected_resolved_zones_revision = self.source_resolved_zones_revision;
        self.accept();
        while activation.decision() == LayoutActivationDecision::Pending {
            tokio::task::yield_now().await;
        }
        if activation.decision() == LayoutActivationDecision::Abort {
            activation.complete(Ok(()));
            return Ok(());
        }

        before_publication().await;
        let result = scene_manager
            .publish_layout_activation(
                spatial_engine,
                candidate_spatial_engine,
                &expected_layout,
                expected_active_scene_id,
                expected_resolved_zones_revision,
                |_| Ok(()),
            )
            .await;
        activation.complete(result.clone());
        result
    }

    pub(crate) fn reject(self, rejection: LayoutTransactionRejection) {
        let _ = self.acknowledgment.send(Err(rejection));
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LayoutActivationDecision {
    Pending,
    Commit,
    Abort,
}

const ACTIVATION_PENDING: u8 = 0;
const ACTIVATION_COMMIT: u8 = 1;
const ACTIVATION_ABORT: u8 = 2;

#[derive(Debug)]
struct LayoutActivationState {
    decision: AtomicU8,
    completion: StdMutex<Option<oneshot::Sender<Result<(), LayoutTransactionRejection>>>>,
}

#[derive(Debug, Clone)]
pub(crate) struct LayoutActivationControl {
    state: Arc<LayoutActivationState>,
}

impl LayoutActivationControl {
    fn new() -> (Self, LayoutActivationReceipt) {
        let (completion, receipt) = oneshot::channel();
        (
            Self {
                state: Arc::new(LayoutActivationState {
                    decision: AtomicU8::new(ACTIVATION_PENDING),
                    completion: StdMutex::new(Some(completion)),
                }),
            },
            LayoutActivationReceipt(receipt),
        )
    }

    pub(crate) fn decision(&self) -> LayoutActivationDecision {
        match self.state.decision.load(Ordering::Acquire) {
            ACTIVATION_COMMIT => LayoutActivationDecision::Commit,
            ACTIVATION_ABORT => LayoutActivationDecision::Abort,
            _ => LayoutActivationDecision::Pending,
        }
    }

    fn commit(&self) {
        let _ = self.state.decision.compare_exchange(
            ACTIVATION_PENDING,
            ACTIVATION_COMMIT,
            Ordering::Release,
            Ordering::Relaxed,
        );
    }

    fn abort(&self) {
        let _ = self.state.decision.compare_exchange(
            ACTIVATION_PENDING,
            ACTIVATION_ABORT,
            Ordering::Release,
            Ordering::Relaxed,
        );
    }

    pub(crate) fn complete(&self, result: Result<(), LayoutTransactionRejection>) {
        if let Some(completion) = self
            .state
            .completion
            .lock()
            .expect("layout activation completion should lock")
            .take()
        {
            let _ = completion.send(result);
        }
    }
}

struct LayoutActivationReceipt(oneshot::Receiver<Result<(), LayoutTransactionRejection>>);

impl LayoutActivationReceipt {
    async fn wait(self) -> Result<(), LayoutTransactionRejection> {
        self.0
            .await
            .unwrap_or(Err(LayoutTransactionRejection::RendererStopped))
    }
}

#[derive(Debug)]
pub(crate) enum SceneTransaction {
    PrepareLayout(PrepareLayoutTransaction),
    SetScreenCaptureConfigured(bool),
}

#[derive(Default)]
struct SceneTransactionQueueState {
    pending: VecDeque<SceneTransaction>,
    closed: bool,
}

#[derive(Clone)]
pub struct SceneTransactionQueue {
    inner: Arc<StdMutex<SceneTransactionQueueState>>,
    scene_activation_lock: Arc<Mutex<()>>,
    layout_update_lock: Arc<Mutex<()>>,
}

impl Default for SceneTransactionQueue {
    fn default() -> Self {
        Self::new()
    }
}

pub(crate) struct SceneActivationGuard {
    _guard: OwnedMutexGuard<()>,
}

pub(crate) struct LayoutUpdateGuard {
    guard: Arc<OwnedMutexGuard<()>>,
}

impl Clone for LayoutUpdateGuard {
    fn clone(&self) -> Self {
        Self {
            guard: Arc::clone(&self.guard),
        }
    }
}

#[derive(Clone)]
pub(crate) struct LayoutTransactionAuthority {
    spatial_engine: SpatialService,
    scene_manager: SceneService,
    scene_transactions: SceneTransactionQueue,
}

impl LayoutTransactionAuthority {
    pub(crate) fn new(
        spatial_engine: SpatialService,
        scene_manager: SceneService,
        scene_transactions: SceneTransactionQueue,
    ) -> Self {
        Self {
            spatial_engine,
            scene_manager,
            scene_transactions,
        }
    }

    pub(crate) async fn acquire_layout_update_guard(&self) -> LayoutUpdateGuard {
        self.scene_transactions.acquire_layout_update_guard().await
    }

    pub(crate) async fn acquire_scene_activation_guard(&self) -> SceneActivationGuard {
        self.scene_transactions
            .acquire_scene_activation_guard()
            .await
    }

    #[cfg(feature = "persistence-test-hooks")]
    pub(crate) fn test_executor(&self) -> LayoutPublicationTestExecutor {
        LayoutPublicationTestExecutor::new(
            self.scene_transactions.clone(),
            self.spatial_engine.clone(),
            self.scene_manager.clone(),
        )
    }

    pub(crate) async fn apply_under_guard(
        &self,
        guard: &LayoutUpdateGuard,
        layout: SpatialLayout,
    ) -> Result<(), LayoutUpdateError> {
        self.apply_under_guard_with_persistence(guard, layout, |_| async {
            LayoutPersistenceOutcome::Written
        })
        .await
    }

    pub(crate) async fn apply_under_guard_with_persistence<F, Fut>(
        &self,
        guard: &LayoutUpdateGuard,
        layout: SpatialLayout,
        mut persist: F,
    ) -> Result<(), LayoutUpdateError>
    where
        F: FnMut(LayoutPersistencePhase) -> Fut + Send + 'static,
        Fut: Future<Output = LayoutPersistenceOutcome> + Send + 'static,
    {
        let prepared = PreparedLayoutUpdate::try_new(layout)?;
        let spatial_engine = self.spatial_engine.clone();
        let scene_manager = self.scene_manager.clone();
        let scene_transactions = self.scene_transactions.clone();
        let retained_guard = guard.clone();
        tokio::spawn(async move {
            let retained_guard = retained_guard;
            let prepared_engine = prepared.into_spatial_engine();
            let (
                expected_layout,
                active_scene_id,
                resolved_zones,
                source_resolved_zones_revision,
                resolved_zones_revision,
                unassigned_behavior,
            ) = {
                let manager = scene_manager.snapshot().await;
                let authoritative_spatial_engine = spatial_engine.snapshot();
                let (resolved_zones, resolved_zones_revision) =
                    manager.resolved_zones_for_primary_layout(prepared_engine.layout().as_ref());
                (
                    authoritative_spatial_engine.layout().as_ref().clone(),
                    manager.active_scene_id().copied(),
                    resolved_zones,
                    manager.resolved_zones_revision(),
                    resolved_zones_revision,
                    manager
                        .active_scene()
                        .map(|scene| scene.unassigned_behavior.clone())
                        .unwrap_or_default(),
                )
            };
            let submission = scene_transactions.submit_layout_preparation(
                prepared_engine.clone(),
                expected_layout,
                active_scene_id,
                Arc::clone(&resolved_zones),
                source_resolved_zones_revision,
                resolved_zones_revision,
                unassigned_behavior,
            )?;
            submission.preparation.wait().await?;
            let commit_state = LayoutPersistenceState {
                layout: prepared_engine.layout().as_ref().clone(),
                active_scene_id,
                resolved_zones: Arc::clone(&resolved_zones),
            };
            match persist(LayoutPersistencePhase::Precommit(commit_state)).await {
                LayoutPersistenceOutcome::Written => {}
                LayoutPersistenceOutcome::Superseded => {
                    submission.activation.abort();
                    let _ = submission.completion.wait().await;
                    return Err(LayoutUpdateError::PersistenceSuperseded);
                }
                LayoutPersistenceOutcome::BeforeAdmission(error) => {
                    submission.activation.abort();
                    let _ = submission.completion.wait().await;
                    return Err(LayoutUpdateError::Persistence(error));
                }
                LayoutPersistenceOutcome::RetryArmed(error) => {
                    submission.activation.abort();
                    let _ = submission.completion.wait().await;
                    let rollback = persist(LayoutPersistencePhase::Rollback).await;
                    if rollback != LayoutPersistenceOutcome::Written {
                        return Err(LayoutUpdateError::PersistenceRollback(format!(
                            "{error}; rollback outcome: {rollback:?}"
                        )));
                    }
                    return Err(LayoutUpdateError::Persistence(error));
                }
            }
            submission.activation.commit();
            let renderer_result = submission.completion.wait().await;
            if let Err(error) = renderer_result {
                let rollback = persist(LayoutPersistencePhase::Rollback).await;
                if rollback != LayoutPersistenceOutcome::Written {
                    return Err(LayoutUpdateError::PersistenceRollback(format!(
                        "{error}; rollback outcome: {rollback:?}"
                    )));
                }
                return Err(error.into());
            }
            drop(retained_guard);
            Ok::<(), LayoutUpdateError>(())
        })
        .await
        .map_err(|error| LayoutUpdateError::Coordinator(error.to_string()))?
    }
}

pub(crate) struct SceneTransactionConsumer {
    queue: SceneTransactionQueue,
}

impl Drop for SceneTransactionConsumer {
    fn drop(&mut self) {
        self.queue.close();
    }
}

impl SceneTransactionQueue {
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Arc::default(),
            scene_activation_lock: Arc::default(),
            layout_update_lock: Arc::default(),
        }
    }

    pub(crate) fn push(
        &self,
        transaction: SceneTransaction,
    ) -> Result<(), LayoutTransactionRejection> {
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
        spatial_engine: SpatialEngine,
        expected_layout: SpatialLayout,
        active_scene_id: Option<SceneId>,
        resolved_zones: Arc<[Zone]>,
        source_resolved_zones_revision: u64,
        resolved_zones_revision: u64,
        unassigned_behavior: UnassignedBehavior,
    ) -> Result<PreparedLayoutSubmission, LayoutTransactionRejection> {
        let (acknowledgment, receipt) = oneshot::channel();
        let (activation, completion) = LayoutActivationControl::new();
        self.push(SceneTransaction::PrepareLayout(PrepareLayoutTransaction {
            spatial_engine,
            expected_layout,
            active_scene_id,
            resolved_zones,
            source_resolved_zones_revision,
            resolved_zones_revision,
            unassigned_behavior,
            activation: activation.clone(),
            acknowledgment,
        }))?;
        Ok(PreparedLayoutSubmission {
            preparation: LayoutTransactionReceipt(receipt),
            activation,
            completion,
        })
    }

    #[must_use]
    pub(crate) fn drain(&self) -> Vec<SceneTransaction> {
        self.inner
            .lock()
            .expect("scene transaction queue should lock")
            .pending
            .drain(..)
            .collect()
    }

    #[cfg(feature = "persistence-test-hooks")]
    fn take_next_layout_preparation(&self) -> Option<PrepareLayoutTransaction> {
        let mut state = self
            .inner
            .lock()
            .expect("scene transaction queue should lock");
        let index = state
            .pending
            .iter()
            .position(|transaction| matches!(transaction, SceneTransaction::PrepareLayout(_)))?;
        match state.pending.remove(index) {
            Some(SceneTransaction::PrepareLayout(transaction)) => Some(transaction),
            Some(SceneTransaction::SetScreenCaptureConfigured(_)) | None => {
                unreachable!("selected queue entry should be a layout preparation")
            }
        }
    }

    #[cfg(feature = "persistence-test-hooks")]
    fn pending_layout_preparation_count(&self) -> usize {
        self.inner
            .lock()
            .expect("scene transaction queue should lock")
            .pending
            .iter()
            .filter(|transaction| matches!(transaction, SceneTransaction::PrepareLayout(_)))
            .count()
    }

    /// Whether any transaction is waiting to be retired.
    ///
    /// Lets an idle render loop skip the servicing path entirely on the ticks
    /// where there is nothing queued.
    #[must_use]
    pub(crate) fn has_pending(&self) -> bool {
        !self
            .inner
            .lock()
            .expect("scene transaction queue should lock")
            .pending
            .is_empty()
    }

    #[must_use]
    pub(crate) fn consumer(&self) -> SceneTransactionConsumer {
        SceneTransactionConsumer {
            queue: self.clone(),
        }
    }

    pub(crate) async fn acquire_layout_update_guard(&self) -> LayoutUpdateGuard {
        LayoutUpdateGuard {
            guard: Arc::new(Arc::clone(&self.layout_update_lock).lock_owned().await),
        }
    }

    pub(crate) async fn acquire_scene_activation_guard(&self) -> SceneActivationGuard {
        SceneActivationGuard {
            _guard: Arc::clone(&self.scene_activation_lock).lock_owned().await,
        }
    }

    pub(crate) fn close(&self) {
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
                SceneTransaction::SetScreenCaptureConfigured(_) => {}
            }
        }
    }

    #[must_use]
    #[cfg(test)]
    pub(crate) fn is_closed(&self) -> bool {
        self.inner
            .lock()
            .expect("scene transaction queue should lock")
            .closed
    }

    #[cfg(test)]
    fn pending_len(&self) -> usize {
        self.inner
            .lock()
            .expect("scene transaction queue should lock")
            .pending
            .len()
    }
}

#[derive(Clone)]
#[cfg(feature = "persistence-test-hooks")]
#[doc(hidden)]
pub struct LayoutPublicationTestExecutor {
    queue: SceneTransactionQueue,
    spatial_engine: SpatialService,
    scene_manager: SceneService,
}

#[cfg(feature = "persistence-test-hooks")]
impl LayoutPublicationTestExecutor {
    #[must_use]
    pub(crate) fn new(
        queue: SceneTransactionQueue,
        spatial_engine: SpatialService,
        scene_manager: SceneService,
    ) -> Self {
        Self {
            queue,
            spatial_engine,
            scene_manager,
        }
    }

    pub async fn execute_next_layout_publication(
        &self,
    ) -> Result<Option<SpatialLayout>, LayoutTransactionRejection> {
        self.execute_next_layout_publication_before(|| async {})
            .await
    }

    #[cfg(feature = "persistence-test-hooks")]
    pub async fn execute_next_layout_publication_with_hook<F, Fut>(
        &self,
        before_publication: F,
    ) -> Result<Option<SpatialLayout>, LayoutTransactionRejection>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = ()>,
    {
        self.execute_next_layout_publication_before(before_publication)
            .await
    }

    #[cfg(feature = "persistence-test-hooks")]
    pub fn reject_next_layout_publication(&self, rejection: LayoutTransactionRejection) -> bool {
        let Some(transaction) = self.queue.take_next_layout_preparation() else {
            return false;
        };
        transaction.reject(rejection);
        true
    }

    #[must_use]
    pub fn pending_layout_publications(&self) -> usize {
        self.queue.pending_layout_preparation_count()
    }

    async fn execute_next_layout_publication_before<F, Fut>(
        &self,
        before_publication: F,
    ) -> Result<Option<SpatialLayout>, LayoutTransactionRejection>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = ()>,
    {
        let Some(transaction) = self.queue.take_next_layout_preparation() else {
            return Ok(None);
        };
        let layout = transaction.spatial_engine().layout().as_ref().clone();
        transaction
            .accept_and_publish(
                &self.spatial_engine,
                &self.scene_manager,
                before_publication,
            )
            .await?;
        Ok(Some(layout))
    }
}

struct LayoutTransactionReceipt(oneshot::Receiver<Result<(), LayoutTransactionRejection>>);

struct PreparedLayoutSubmission {
    preparation: LayoutTransactionReceipt,
    activation: LayoutActivationControl,
    completion: LayoutActivationReceipt,
}

impl LayoutTransactionReceipt {
    async fn wait(self) -> Result<(), LayoutTransactionRejection> {
        self.0
            .await
            .unwrap_or(Err(LayoutTransactionRejection::RendererStopped))
    }
}

#[cfg(test)]
mod tests;
