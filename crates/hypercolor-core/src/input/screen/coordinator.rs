use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use futures_util::stream::{FuturesUnordered, StreamExt as _};
use thiserror::Error;

use super::{
    CaptureSourceId, CommittedScreenPlan, PreparingScreenPlan, ScreenPlanAbort, ScreenPlanError,
    ScreenPreparedWorkerToken, ScreenPublicationDemandSnapshot,
};

type WorkerPreparationFuture =
    Pin<Box<dyn Future<Output = anyhow::Result<ScreenPreparedWorkerToken>> + Send + 'static>>;

/// Detached completion handle for resources prepared by one source worker.
///
/// Constructing this handle must only enqueue or snapshot source-owned work.
/// The returned future performs all blocking or asynchronous preparation after
/// the input-manager lock has been released.
pub struct ScreenWorkerPreparation {
    completion: Option<WorkerPreparationFuture>,
    abort: Option<Box<dyn FnOnce() + Send + 'static>>,
}

impl ScreenWorkerPreparation {
    /// Wrap one source-owned asynchronous preparation.
    #[must_use]
    pub fn new(
        completion: impl Future<Output = anyhow::Result<ScreenPreparedWorkerToken>> + Send + 'static,
    ) -> Self {
        Self {
            completion: Some(Box::pin(completion)),
            abort: None,
        }
    }

    /// Wrap preparation with an enqueue-only source cleanup callback.
    #[must_use]
    pub fn with_abort(
        completion: impl Future<Output = anyhow::Result<ScreenPreparedWorkerToken>> + Send + 'static,
        abort: impl FnOnce() + Send + 'static,
    ) -> Self {
        Self {
            completion: Some(Box::pin(completion)),
            abort: Some(Box::new(abort)),
        }
    }

    pub(crate) async fn complete(
        mut self,
    ) -> (anyhow::Result<ScreenPreparedWorkerToken>, ScreenWorkerAbort) {
        let result = self
            .completion
            .take()
            .expect("screen worker preparation completes once")
            .await;
        let abort = ScreenWorkerAbort {
            callback: self.abort.take(),
        };
        (result, abort)
    }
}

impl Drop for ScreenWorkerPreparation {
    fn drop(&mut self) {
        if let Some(abort) = self.abort.take() {
            abort();
        }
    }
}

impl std::fmt::Debug for ScreenWorkerPreparation {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ScreenWorkerPreparation")
            .finish_non_exhaustive()
    }
}

pub(crate) struct ScreenWorkerAbort {
    callback: Option<Box<dyn FnOnce() + Send + 'static>>,
}

impl ScreenWorkerAbort {
    fn disarm(mut self) {
        self.callback = None;
    }
}

impl Drop for ScreenWorkerAbort {
    fn drop(&mut self) {
        if let Some(callback) = self.callback.take() {
            callback();
        }
    }
}

impl std::fmt::Debug for ScreenWorkerAbort {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ScreenWorkerAbort")
            .finish_non_exhaustive()
    }
}

type WorkerRetirementFuture = Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + 'static>>;

/// Detached source-worker cleanup started after authority commits.
pub struct ScreenWorkerRetirement {
    completion: WorkerRetirementFuture,
}

impl ScreenWorkerRetirement {
    /// Wrap one source-owned retirement operation.
    #[must_use]
    pub fn new(completion: impl Future<Output = anyhow::Result<()>> + Send + 'static) -> Self {
        Self {
            completion: Box::pin(completion),
        }
    }

    /// Wait until the source worker releases every retired runtime owner.
    ///
    /// # Errors
    ///
    /// Returns the source-owned cleanup failure after committed authority has
    /// already advanced.
    pub async fn complete(self) -> anyhow::Result<()> {
        self.completion.await
    }
}

impl std::fmt::Debug for ScreenWorkerRetirement {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ScreenWorkerRetirement")
            .finish_non_exhaustive()
    }
}

/// Committed authority plus source-owned cleanup work to finish out of lock.
#[must_use = "committed screen publication retirement must run out of lock"]
#[derive(Debug)]
pub struct CommittedScreenPublicationTransition {
    committed: CommittedScreenPlan,
    worker_retirements: Vec<(Arc<str>, ScreenWorkerRetirement)>,
}

impl CommittedScreenPublicationTransition {
    pub(crate) fn new(
        committed: CommittedScreenPlan,
        worker_retirements: Vec<(Arc<str>, ScreenWorkerRetirement)>,
    ) -> Self {
        Self {
            committed,
            worker_retirements,
        }
    }

    /// Split committed core retirement from source-worker cleanup handles.
    pub fn into_parts(self) -> (CommittedScreenPlan, Vec<(Arc<str>, ScreenWorkerRetirement)>) {
        (self.committed, self.worker_retirements)
    }
}

#[derive(Debug)]
pub(crate) struct PendingScreenWorkerPreparation {
    pub(crate) source_id: CaptureSourceId,
    pub(crate) preparation: ScreenWorkerPreparation,
}

type WorkerCompletionFuture = Pin<
    Box<
        dyn Future<
                Output = (
                    CaptureSourceId,
                    (anyhow::Result<ScreenPreparedWorkerToken>, ScreenWorkerAbort),
                ),
            > + Send
            + 'static,
    >,
>;

struct ScreenPublicationAwaitGuard {
    preparing: Option<PreparingScreenPlan>,
    completions: FuturesUnordered<WorkerCompletionFuture>,
    worker_aborts: Vec<ScreenWorkerAbort>,
}

impl ScreenPublicationAwaitGuard {
    fn new(preparing: PreparingScreenPlan, workers: Vec<PendingScreenWorkerPreparation>) -> Self {
        let guard = Self {
            preparing: Some(preparing),
            completions: FuturesUnordered::new(),
            worker_aborts: Vec::new(),
        };
        for worker in workers {
            guard.completions.push(Box::pin(async move {
                (worker.source_id, worker.preparation.complete().await)
            }));
        }
        guard
    }

    fn preparing_mut(&mut self) -> &mut PreparingScreenPlan {
        self.preparing
            .as_mut()
            .expect("awaiting screen publication retains its candidate")
    }

    fn abort(&mut self) -> ScreenPlanAbort {
        self.preparing
            .take()
            .expect("awaiting screen publication aborts once")
            .abort()
    }

    fn into_prepared(
        mut self,
        demand: ScreenPublicationDemandSnapshot,
    ) -> PreparedScreenPublicationPlan {
        debug_assert!(self.completions.is_empty());
        PreparedScreenPublicationPlan {
            preparing: self.preparing.take(),
            demand,
            worker_aborts: std::mem::take(&mut self.worker_aborts),
        }
    }
}

impl Drop for ScreenPublicationAwaitGuard {
    fn drop(&mut self) {
        if let Some(preparing) = self.preparing.take() {
            drop(preparing.abort());
        }
    }
}

/// Candidate plan whose real worker resources are preparing out of lock.
#[must_use = "screen publication preparation must be awaited or aborted"]
pub struct ScreenPublicationPreparation {
    preparing: Option<PreparingScreenPlan>,
    workers: Vec<PendingScreenWorkerPreparation>,
    demand: ScreenPublicationDemandSnapshot,
}

impl ScreenPublicationPreparation {
    pub(crate) fn new(
        preparing: PreparingScreenPlan,
        workers: Vec<PendingScreenWorkerPreparation>,
        demand: ScreenPublicationDemandSnapshot,
    ) -> Self {
        Self {
            preparing: Some(preparing),
            workers,
            demand,
        }
    }

    /// Await every already-started source worker without holding the manager.
    ///
    /// A failed worker aborts the shared activation latch and returns the
    /// byte-identical active-plan receipt.
    ///
    /// # Errors
    ///
    /// Returns the source failure together with an explicit abort receipt.
    pub async fn await_workers(
        mut self,
    ) -> Result<PreparedScreenPublicationPlan, ScreenPublicationTransitionFailure> {
        let preparing = self
            .preparing
            .take()
            .expect("screen publication preparation is consumed once");
        let workers = std::mem::take(&mut self.workers);
        let mut awaiting = ScreenPublicationAwaitGuard::new(preparing, workers);
        while let Some((source_id, (token, abort))) = awaiting.completions.next().await {
            awaiting.worker_aborts.push(abort);
            let token = match token {
                Ok(token) => token,
                Err(error) => {
                    let abort = awaiting.abort();
                    return Err(ScreenPublicationTransitionFailure::new(
                        ScreenPublicationTransitionError::WorkerPreparationFailed {
                            source_id,
                            message: Arc::from(error.to_string()),
                        },
                        abort,
                    ));
                }
            };
            if let Err(error) = awaiting.preparing_mut().acknowledge(token) {
                let abort = awaiting.abort();
                return Err(ScreenPublicationTransitionFailure::new(
                    ScreenPublicationTransitionError::Plan(error),
                    abort,
                ));
            }
        }
        Ok(awaiting.into_prepared(self.demand.clone()))
    }

    /// Explicitly abandon all started worker preparations.
    #[must_use]
    pub fn abort(mut self) -> ScreenPlanAbort {
        self.preparing
            .take()
            .expect("screen publication preparation is consumed once")
            .abort()
    }
}

impl Drop for ScreenPublicationPreparation {
    fn drop(&mut self) {
        if let Some(preparing) = self.preparing.take() {
            drop(preparing.abort());
        }
    }
}

impl std::fmt::Debug for ScreenPublicationPreparation {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ScreenPublicationPreparation")
            .field("preparing", &self.preparing)
            .field("worker_count", &self.workers.len())
            .finish_non_exhaustive()
    }
}

/// Fully acknowledged candidate waiting for one fenced manager commit.
#[must_use = "prepared screen publication plans must be committed or aborted"]
pub struct PreparedScreenPublicationPlan {
    preparing: Option<PreparingScreenPlan>,
    demand: ScreenPublicationDemandSnapshot,
    worker_aborts: Vec<ScreenWorkerAbort>,
}

impl PreparedScreenPublicationPlan {
    pub(crate) fn take_preparing(&mut self) -> PreparingScreenPlan {
        self.preparing
            .take()
            .expect("prepared screen publication plan is consumed once")
    }

    pub(crate) fn demand(&self) -> &ScreenPublicationDemandSnapshot {
        &self.demand
    }

    pub(crate) fn disarm_worker_aborts(&mut self) {
        for abort in std::mem::take(&mut self.worker_aborts) {
            abort.disarm();
        }
    }

    /// Explicitly abandon the acknowledged candidate.
    #[must_use]
    pub fn abort(mut self) -> ScreenPlanAbort {
        self.take_preparing().abort()
    }
}

impl Drop for PreparedScreenPublicationPlan {
    fn drop(&mut self) {
        if let Some(preparing) = self.preparing.take() {
            drop(preparing.abort());
        }
    }
}

impl std::fmt::Debug for PreparedScreenPublicationPlan {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PreparedScreenPublicationPlan")
            .field("preparing", &self.preparing)
            .finish_non_exhaustive()
    }
}

/// Transaction failure paired with proof that active authority was preserved.
#[derive(Debug)]
pub struct ScreenPublicationTransitionFailure {
    error: ScreenPublicationTransitionError,
    abort: ScreenPlanAbort,
}

impl ScreenPublicationTransitionFailure {
    pub(crate) const fn new(
        error: ScreenPublicationTransitionError,
        abort: ScreenPlanAbort,
    ) -> Self {
        Self { error, abort }
    }

    /// Exact transition failure.
    #[must_use]
    pub const fn error(&self) -> &ScreenPublicationTransitionError {
        &self.error
    }

    /// Abort receipt naming the unchanged active plan.
    #[must_use]
    pub const fn abort(&self) -> &ScreenPlanAbort {
        &self.abort
    }
}

impl std::fmt::Display for ScreenPublicationTransitionFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.error.fmt(formatter)
    }
}

impl std::error::Error for ScreenPublicationTransitionFailure {}

/// Failure to resolve, prepare, fence, or commit an exact publication plan.
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum ScreenPublicationTransitionError {
    /// Demand was prepared against a different structural input graph.
    #[error(
        "screen publication graph generation changed: expected {expected:?}, observed {observed:?}"
    )]
    GraphGenerationConflict {
        /// Generation carried by the immutable demand snapshot.
        expected: super::ScreenInputGraphGeneration,
        /// Current manager-owned graph generation.
        observed: super::ScreenInputGraphGeneration,
    },
    /// No registered source could resolve one exact logical branch.
    #[error("no screen source resolved exact publication branch {branch_index}")]
    UnresolvedBranch {
        /// Stable index in the immutable demand snapshot.
        branch_index: usize,
    },
    /// More than one registered source claimed one logical branch.
    #[error("multiple screen sources resolved exact publication branch {branch_index}")]
    AmbiguousBranch {
        /// Stable index in the immutable demand snapshot.
        branch_index: usize,
    },
    /// Two manager slots claimed the same resolved capture-source identity.
    #[error("multiple screen sources claim resolved capture id {source_id:?}")]
    SourceOwnershipConflict {
        /// Capture source with conflicting worker ownership.
        source_id: CaptureSourceId,
    },
    /// A source failed while resolving an exact branch.
    #[error("screen source {source_name} failed to resolve branch {branch_index}: {message}")]
    SourceResolutionFailed {
        /// Registered source name.
        source_name: Arc<str>,
        /// Stable index in the immutable demand snapshot.
        branch_index: usize,
        /// Source-owned diagnostic.
        message: Arc<str>,
    },
    /// No live source owns a ticket that introduces candidate resources.
    #[error("no live screen source owns preparation ticket for {source_id:?}")]
    WorkerOwnerMissing {
        /// Resolved capture source requiring preparation.
        source_id: CaptureSourceId,
    },
    /// A source could not enqueue its worker preparation.
    #[error("screen source {source_id:?} could not start worker preparation: {message}")]
    WorkerPreparationStartFailed {
        /// Resolved capture source requiring preparation.
        source_id: CaptureSourceId,
        /// Source-owned diagnostic.
        message: Arc<str>,
    },
    /// A source worker failed after preparation was detached from the manager.
    #[error("screen source {source_id:?} failed worker preparation: {message}")]
    WorkerPreparationFailed {
        /// Resolved capture source whose worker failed.
        source_id: CaptureSourceId,
        /// Source-owned diagnostic.
        message: Arc<str>,
    },
    /// Canonical plan construction or a transaction fence failed.
    #[error(transparent)]
    Plan(#[from] ScreenPlanError),
}
