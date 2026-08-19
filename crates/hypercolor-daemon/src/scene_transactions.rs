use std::collections::VecDeque;
use std::future::Future;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
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
    #[error("layout activation was superseded by a newer control-plane state")]
    Superseded,
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
    #[error("layout persistence was superseded before renderer activation")]
    PersistenceSuperseded,
    #[error("layout rollback persistence failed: {0}")]
    PersistenceRollback(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LayoutTransactionToken(u64);

#[derive(Debug, Clone)]
pub struct PreparedLayoutUpdate {
    spatial_engine: SpatialEngine,
}

#[derive(Debug, Clone)]
pub(crate) struct LayoutPersistenceState {
    pub(crate) layout: SpatialLayout,
    pub(crate) active_scene_id: Option<SceneId>,
    pub(crate) active_render_groups: Arc<[Zone]>,
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
    expected_layout: SpatialLayout,
    active_scene_id: Option<SceneId>,
    active_render_groups: Arc<[Zone]>,
    source_active_render_groups_revision: u64,
    active_render_groups_revision: u64,
    unassigned_behavior: UnassignedBehavior,
    activation: LayoutActivationControl,
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
    pub fn expected_layout(&self) -> &SpatialLayout {
        &self.expected_layout
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
    pub const fn source_active_render_groups_revision(&self) -> u64 {
        self.source_active_render_groups_revision
    }

    #[must_use]
    pub const fn active_render_groups_revision(&self) -> u64 {
        self.active_render_groups_revision
    }

    #[must_use]
    pub fn unassigned_behavior(&self) -> &UnassignedBehavior {
        &self.unassigned_behavior
    }

    pub(crate) fn activation(&self) -> LayoutActivationControl {
        self.activation.clone()
    }

    pub(crate) fn is_cancelled(&self) -> bool {
        self.acknowledgment.is_closed()
    }

    #[doc(hidden)]
    pub fn accept(self) {
        let _ = self.acknowledgment.send(Ok(()));
    }

    #[doc(hidden)]
    pub fn accept_and_commit_for_test(self) {
        let activation = self.activation();
        self.accept();
        activation.commit();
        activation.complete(Ok(()));
    }

    #[doc(hidden)]
    pub async fn accept_and_publish_for_test<F, Fut>(
        self,
        spatial_engine: &Arc<RwLock<SpatialEngine>>,
        scene_manager: &Arc<RwLock<SceneManager>>,
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
        let expected_active_render_groups_revision = self.source_active_render_groups_revision;
        self.accept();
        while activation.decision() == LayoutActivationDecision::Pending {
            tokio::task::yield_now().await;
        }
        if activation.decision() == LayoutActivationDecision::Abort {
            activation.complete(Ok(()));
            return Ok(());
        }

        before_publication().await;
        let result = publish_prepared_layout_activation(
            spatial_engine,
            scene_manager,
            candidate_spatial_engine,
            &expected_layout,
            expected_active_scene_id,
            expected_active_render_groups_revision,
            |_| {},
        )
        .await;
        activation.complete(result.clone());
        result
    }

    #[doc(hidden)]
    pub fn reject(self, rejection: LayoutTransactionRejection) {
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
pub enum SceneTransaction {
    PrepareLayout(PrepareLayoutTransaction),
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
    scene_activation_lock: Arc<Mutex<()>>,
    layout_update_lock: Arc<Mutex<()>>,
    next_layout_token: Arc<AtomicU64>,
}

pub struct SceneActivationGuard {
    _guard: OwnedMutexGuard<()>,
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
        expected_layout: SpatialLayout,
        active_scene_id: Option<SceneId>,
        active_render_groups: Arc<[Zone]>,
        source_active_render_groups_revision: u64,
        active_render_groups_revision: u64,
        unassigned_behavior: UnassignedBehavior,
    ) -> Result<PreparedLayoutSubmission, LayoutTransactionRejection> {
        let (acknowledgment, receipt) = oneshot::channel();
        let (activation, completion) = LayoutActivationControl::new();
        self.push(SceneTransaction::PrepareLayout(PrepareLayoutTransaction {
            token,
            spatial_engine,
            expected_layout,
            active_scene_id,
            active_render_groups,
            source_active_render_groups_revision,
            active_render_groups_revision,
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

    /// Whether any transaction is waiting to be retired.
    ///
    /// Lets an idle render loop skip the servicing path entirely on the ticks
    /// where there is nothing queued.
    #[must_use]
    pub fn has_pending(&self) -> bool {
        !self
            .inner
            .lock()
            .expect("scene transaction queue should lock")
            .pending
            .is_empty()
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

    pub async fn acquire_scene_activation_guard(&self) -> SceneActivationGuard {
        SceneActivationGuard {
            _guard: Arc::clone(&self.scene_activation_lock).lock_owned().await,
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

pub(crate) async fn publish_prepared_layout_activation<F>(
    spatial_engine: &Arc<RwLock<SpatialEngine>>,
    scene_manager: &Arc<RwLock<SceneManager>>,
    candidate_spatial_engine: SpatialEngine,
    expected_layout: &SpatialLayout,
    expected_active_scene_id: Option<SceneId>,
    expected_active_render_groups_revision: u64,
    publish_renderer_state: F,
) -> Result<(), LayoutTransactionRejection>
where
    F: FnOnce(SpatialEngine),
{
    // FRAME-BOUNDARY WRITER (Spec 76 §2.3, §6.1). This runs on the
    // render thread and must hold the scene lock and the spatial lock
    // together, so the renderer never observes a layout and a zone set
    // that disagree. `commit_scene` takes neither the spatial lock nor
    // an `AppState` the render thread could reach, so it cannot serve
    // this swap; §6.1 re-points commit at this transaction instead. The
    // source-is-current check below is this writer's own
    // compare-and-swap, and it refuses whenever a commit has moved the
    // active scene, its resolved zones, or the authoritative layout.
    let mut manager = scene_manager.write().await;
    let mut authoritative_spatial_engine = spatial_engine.write().await;
    let source_is_current = manager.active_scene_id().copied() == expected_active_scene_id
        && manager.active_render_groups_revision() == expected_active_render_groups_revision
        && authoritative_spatial_engine.layout().as_ref() == expected_layout;
    if !source_is_current {
        return Err(LayoutTransactionRejection::Superseded);
    }

    manager.sync_primary_group_layout(candidate_spatial_engine.layout().as_ref());
    *authoritative_spatial_engine = candidate_spatial_engine.clone();
    publish_renderer_state(candidate_spatial_engine);
    Ok(())
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
        |_| async { LayoutPersistenceOutcome::Written },
    )
    .await
}

pub(crate) async fn apply_prepared_layout_update_under_guard_with_persistence<F, Fut>(
    spatial_engine: Arc<RwLock<SpatialEngine>>,
    scene_manager: Arc<RwLock<SceneManager>>,
    scene_transactions: SceneTransactionQueue,
    guard: &LayoutUpdateGuard,
    prepared: PreparedLayoutUpdate,
    mut persist: F,
) -> Result<(), LayoutUpdateError>
where
    F: FnMut(LayoutPersistencePhase) -> Fut + Send + 'static,
    Fut: Future<Output = LayoutPersistenceOutcome> + Send + 'static,
{
    let retained_guard = guard.clone();
    tokio::spawn(async move {
        let retained_guard = retained_guard;
        let prepared_engine = prepared.into_spatial_engine();
        let (
            expected_layout,
            active_scene_id,
            active_render_groups,
            source_active_render_groups_revision,
            active_render_groups_revision,
            unassigned_behavior,
        ) = {
            let manager = scene_manager.read().await;
            let authoritative_spatial_engine = spatial_engine.read().await;
            let (active_render_groups, active_render_groups_revision) =
                manager.active_render_groups_for_primary_layout(prepared_engine.layout().as_ref());
            (
                authoritative_spatial_engine.layout().as_ref().clone(),
                manager.active_scene_id().copied(),
                active_render_groups,
                manager.active_render_groups_revision(),
                active_render_groups_revision,
                manager
                    .active_scene()
                    .map(|scene| scene.unassigned_behavior.clone())
                    .unwrap_or_default(),
            )
        };
        let token = scene_transactions.next_layout_token();
        let submission = scene_transactions.submit_layout_preparation(
            token,
            prepared_engine.clone(),
            expected_layout,
            active_scene_id,
            Arc::clone(&active_render_groups),
            source_active_render_groups_revision,
            active_render_groups_revision,
            unassigned_behavior,
        )?;
        submission.preparation.wait().await?;
        let commit_state = LayoutPersistenceState {
            layout: prepared_engine.layout().as_ref().clone(),
            active_scene_id,
            active_render_groups: Arc::clone(&active_render_groups),
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
