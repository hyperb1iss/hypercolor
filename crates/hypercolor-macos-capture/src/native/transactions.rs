use std::fmt;
use std::io;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Instant;

use crate::MacosCaptureError;

mod api;
mod deadline;

pub use api::{MacosStreamDiagnosticTransaction, MacosStreamRequestTransaction};
pub(super) use api::{stream_diagnostic_transaction, stream_request_transaction};
pub(super) use deadline::{DeadlineScheduler, DeadlineTicket};

#[cfg(test)]
mod tests;

type TransactionHook = Arc<dyn Fn() + Send + Sync + 'static>;
/// Cancel hooks receive the generation the cell held when the cancel
/// claimed it. Hooks must target that value rather than a generation
/// captured at registration time: stage adoption rekeys the cell, and a
/// captured generation goes stale the moment it does.
type TransactionCancelHook = Arc<dyn Fn(u64) + Send + Sync + 'static>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MacosNativeTransactionPhase {
    SourceResolution,
    StreamStart,
    FirstCompleteFrame,
}

impl fmt::Display for MacosNativeTransactionPhase {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::SourceResolution => "source resolution",
            Self::StreamStart => "stream start",
            Self::FirstCompleteFrame => "first complete frame",
        })
    }
}

#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum MacosNativeTransactionError {
    #[error("macOS {phase} transaction {generation} was cancelled")]
    Cancelled {
        phase: MacosNativeTransactionPhase,
        generation: u64,
    },
    #[error("macOS {phase} transaction {generation} timed out")]
    TimedOut {
        phase: MacosNativeTransactionPhase,
        generation: u64,
    },
    #[error(transparent)]
    Capture(#[from] MacosCaptureError),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct TransactionIdentity {
    pub(super) generation: u64,
    pub(super) phase: MacosNativeTransactionPhase,
}

struct TransactionState<T> {
    generation: u64,
    phase: MacosNativeTransactionPhase,
    claimed: bool,
    outcome: Option<Result<T, MacosNativeTransactionError>>,
    deadline: Option<Instant>,
    deadline_revision: u64,
    deadline_ticket: Option<DeadlineTicket>,
    timeout: Option<TransactionHook>,
    cancel: Option<TransactionCancelHook>,
}

struct TransactionCell<T> {
    state: Mutex<TransactionState<T>>,
    ready: Condvar,
}

/// Cancels the transaction when the last completer clone drops so an
/// abandoned cell can never strand its waiter. The registered cancel hook
/// is deliberately not run here: completer drop happens on the owning
/// (native) side, often while its state lock is held, and the hook exists
/// to actuate native-side cancellation that the dropping owner is already
/// performing.
struct CompleterGuard<T> {
    cell: Arc<TransactionCell<T>>,
}

impl<T> Drop for CompleterGuard<T> {
    fn drop(&mut self) {
        let Some((settlement, _)) = claim_with(&self.cell, |state| {
            Some((
                Err(MacosNativeTransactionError::Cancelled {
                    phase: state.phase,
                    generation: state.generation,
                }),
                None::<TransactionHook>,
            ))
        }) else {
            return;
        };
        settlement.publish();
    }
}

pub(super) struct TransactionCompleter<T> {
    cell: Arc<TransactionCell<T>>,
    guard: Arc<CompleterGuard<T>>,
}

pub(super) struct TransactionSettlement<T> {
    cell: Arc<TransactionCell<T>>,
    outcome: Option<Result<T, MacosNativeTransactionError>>,
}

struct TransactionWaiter<T> {
    cell: Arc<TransactionCell<T>>,
    cancel_on_drop: bool,
}

impl<T> Clone for TransactionCompleter<T> {
    fn clone(&self) -> Self {
        Self {
            cell: Arc::clone(&self.cell),
            guard: Arc::clone(&self.guard),
        }
    }
}

impl<T> fmt::Debug for TransactionCompleter<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TransactionCompleter")
            .field("identity", &self.identity())
            .field("open", &self.is_open())
            .finish()
    }
}

impl<T> TransactionCompleter<T> {
    pub(super) fn new(identity: TransactionIdentity, deadline: Option<Instant>) -> Self {
        let cell = Arc::new(TransactionCell {
            state: Mutex::new(TransactionState {
                generation: identity.generation,
                phase: identity.phase,
                claimed: false,
                outcome: None,
                deadline,
                deadline_revision: 0,
                deadline_ticket: None,
                timeout: None,
                cancel: None,
            }),
            ready: Condvar::new(),
        });
        Self {
            guard: Arc::new(CompleterGuard {
                cell: Arc::clone(&cell),
            }),
            cell,
        }
    }

    fn waiter(&self) -> TransactionWaiter<T> {
        TransactionWaiter {
            cell: Arc::clone(&self.cell),
            cancel_on_drop: true,
        }
    }

    pub(super) fn identity(&self) -> TransactionIdentity {
        let state = lock(&self.cell.state);
        TransactionIdentity {
            generation: state.generation,
            phase: state.phase,
        }
    }

    /// Rebinds the transaction to a new stage generation when an in-flight
    /// request is adopted by a fresh candidate stage (source pick,
    /// interrupted-stream recovery). Generation-filtered operations key on
    /// the live cell generation, so adoption must rekey the cell or every
    /// later arm/cancel/claim silently misses the adopted transaction. The
    /// previous generation's deadline is retired in the same breath: its
    /// timeout targets the stage the transaction just left.
    pub(super) fn rekey_generation(&self, generation: u64) -> bool {
        let retired = {
            let mut state = lock(&self.cell.state);
            if state.claimed {
                return false;
            }
            let Some(revision) = state.deadline_revision.checked_add(1) else {
                // Revision exhaustion means in-flight deadline callbacks can
                // no longer be invalidated, so the rekey must refuse rather
                // than adopt a cell it cannot quarantine.
                return false;
            };
            state.generation = generation;
            state.deadline_revision = revision;
            (state.timeout.take(), state.deadline_ticket.take())
        };
        drop(retired);
        true
    }

    pub(super) fn set_cancel(&self, cancel: impl Fn(u64) + Send + Sync + 'static) {
        let mut state = lock(&self.cell.state);
        if !state.claimed {
            state.cancel = Some(Arc::new(cancel));
        }
    }

    pub(super) fn arm(
        &self,
        scheduler: &DeadlineScheduler,
        deadline: Instant,
        timeout: impl Fn() + Send + Sync + 'static,
    ) -> io::Result<bool>
    where
        T: Send + 'static,
    {
        self.arm_gated(scheduler, deadline, |state| !state.claimed, timeout)
    }

    /// Set the phase and arm a deadline on behalf of a specific stage
    /// generation, atomically with the generation check.
    ///
    /// The check must live under the cell lock in the same critical section
    /// that mutates the phase and allocates the deadline revision: an arm
    /// that validated the generation under the stream-state lock and was
    /// then preempted across an adoption rekey would otherwise re-install a
    /// deadline whose timeout hook targets the superseded stage and no-ops,
    /// wedging the adopted candidate. A rekey landing between this section
    /// and the ticket commit bumps the deadline revision, so the commit
    /// check rejects that interleaving.
    pub(super) fn arm_for_generation(
        &self,
        scheduler: &DeadlineScheduler,
        deadline: Instant,
        expected_generation: u64,
        phase: MacosNativeTransactionPhase,
        timeout: impl Fn() + Send + Sync + 'static,
    ) -> io::Result<bool>
    where
        T: Send + 'static,
    {
        self.arm_gated(
            scheduler,
            deadline,
            move |state| {
                if state.claimed || state.generation != expected_generation {
                    return false;
                }
                state.phase = phase;
                true
            },
            timeout,
        )
    }

    fn arm_gated(
        &self,
        scheduler: &DeadlineScheduler,
        deadline: Instant,
        gate: impl FnOnce(&mut TransactionState<T>) -> bool,
        timeout: impl Fn() + Send + Sync + 'static,
    ) -> io::Result<bool>
    where
        T: Send + 'static,
    {
        let timeout: TransactionHook = Arc::new(timeout);
        let scheduled_timeout = Arc::clone(&timeout);
        let (revision, deadline) = {
            let mut state = lock(&self.cell.state);
            if !gate(&mut state) {
                return Ok(false);
            }
            state.deadline_revision = state
                .deadline_revision
                .checked_add(1)
                .ok_or_else(|| io::Error::other("macOS transaction deadline revision exhausted"))?;
            (
                state.deadline_revision,
                state
                    .deadline
                    .map_or(deadline, |current| current.min(deadline)),
            )
        };
        let timeout_cell = Arc::downgrade(&self.cell);
        let ticket = match scheduler.schedule(deadline, move || {
            if let Some(cell) = timeout_cell.upgrade() {
                let _ = claim_timeout(&cell, Some(revision), Some(scheduled_timeout));
            }
        }) {
            Ok(ticket) => ticket,
            Err(error) => {
                let previous = {
                    let mut state = lock(&self.cell.state);
                    if !state.claimed && state.deadline_revision == revision {
                        state.deadline = None;
                        state.timeout = None;
                        state.deadline_ticket.take()
                    } else {
                        None
                    }
                };
                drop(previous);
                return Err(error);
            }
        };
        let mut state = lock(&self.cell.state);
        if state.claimed || state.deadline_revision != revision {
            drop(state);
            drop(ticket);
            return Ok(false);
        }
        let previous = state.deadline_ticket.replace(ticket);
        state.deadline = Some(deadline);
        state.timeout = Some(timeout);
        drop(state);
        drop(previous);
        Ok(true)
    }

    pub(super) fn claim(
        &self,
        outcome: Result<T, MacosNativeTransactionError>,
    ) -> Option<TransactionSettlement<T>> {
        claim_with(&self.cell, move |_| {
            Some((outcome, None::<TransactionHook>))
        })
        .map(|(settlement, _)| settlement)
    }

    #[cfg(test)]
    pub(super) fn finish(&self, outcome: Result<T, MacosNativeTransactionError>) -> bool {
        self.claim(outcome).is_some_and(|settlement| {
            settlement.publish();
            true
        })
    }

    pub(super) fn is_open(&self) -> bool {
        !lock(&self.cell.state).claimed
    }

    pub(super) fn shares_cell(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.cell, &other.cell)
    }

    #[cfg(test)]
    pub(super) fn current_deadline(&self) -> Option<Instant> {
        lock(&self.cell.state).deadline
    }

    #[cfg(test)]
    pub(super) fn has_deadline_ticket(&self) -> bool {
        lock(&self.cell.state).deadline_ticket.is_some()
    }

    #[cfg(test)]
    pub(super) fn outcome(&self) -> Option<Result<T, MacosNativeTransactionError>>
    where
        T: Clone,
    {
        lock(&self.cell.state).outcome.clone()
    }

    #[cfg(test)]
    pub(super) fn cancel(&self) -> bool {
        claim_cancel(&self.cell)
    }
}

impl<T> TransactionSettlement<T> {
    pub(super) fn publish(mut self) {
        self.publish_inner(false);
    }

    fn publish_inner(&mut self, abandoned: bool) {
        let Some(mut outcome) = self.outcome.take() else {
            return;
        };
        let mut state = lock(&self.cell.state);
        debug_assert!(state.claimed, "published transaction was claimed");
        debug_assert!(state.outcome.is_none(), "transaction publishes once");
        if abandoned && outcome.is_ok() {
            outcome = Err(MacosNativeTransactionError::Cancelled {
                phase: state.phase,
                generation: state.generation,
            });
        }
        state.outcome = Some(outcome);
        drop(state);
        self.cell.ready.notify_all();
    }
}

impl<T> Drop for TransactionSettlement<T> {
    fn drop(&mut self) {
        self.publish_inner(true);
    }
}

impl<T> TransactionWaiter<T> {
    fn wait(mut self) -> Result<T, MacosNativeTransactionError> {
        let mut state = lock(&self.cell.state);
        while state.outcome.is_none() {
            if state.claimed {
                state = self
                    .cell
                    .ready
                    .wait(state)
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                continue;
            }
            let Some(deadline) = state.deadline else {
                state = self
                    .cell
                    .ready
                    .wait(state)
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                continue;
            };
            let now = Instant::now();
            if now >= deadline {
                drop(state);
                let _ = claim_timeout(&self.cell, None, None);
                state = lock(&self.cell.state);
                continue;
            }
            let (waiting, _) = self
                .cell
                .ready
                .wait_timeout(state, deadline.duration_since(now))
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state = waiting;
        }
        self.cancel_on_drop = false;
        state
            .outcome
            .take()
            .expect("settled macOS native transaction has an outcome")
    }

    fn cancel(mut self) -> bool {
        self.cancel_on_drop = false;
        claim_cancel(&self.cell)
    }

    fn current_deadline(&self) -> Option<Instant> {
        lock(&self.cell.state).deadline
    }

    fn wait_until(mut self, deadline: Instant) -> Result<T, MacosNativeTransactionError> {
        let mut state = lock(&self.cell.state);
        while state.outcome.is_none() {
            if state.claimed {
                state = self
                    .cell
                    .ready
                    .wait(state)
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                continue;
            }
            let effective_deadline = state
                .deadline
                .map_or(deadline, |current| current.min(deadline));
            let now = Instant::now();
            if now >= effective_deadline {
                drop(state);
                let _ = claim_timeout(&self.cell, None, None);
                state = lock(&self.cell.state);
                continue;
            }
            let (waiting, _) = self
                .cell
                .ready
                .wait_timeout(state, effective_deadline.duration_since(now))
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state = waiting;
        }
        self.cancel_on_drop = false;
        state
            .outcome
            .take()
            .expect("settled macOS native transaction has an outcome")
    }
}

#[cfg(test)]
impl<T: Clone> TransactionWaiter<T> {
    fn try_outcome(&self) -> Option<Result<T, MacosNativeTransactionError>> {
        lock(&self.cell.state).outcome.clone()
    }

    fn wait_outcome(&self) -> Result<T, MacosNativeTransactionError> {
        let mut state = lock(&self.cell.state);
        while state.outcome.is_none() {
            state = self
                .cell
                .ready
                .wait(state)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
        state
            .outcome
            .clone()
            .expect("settled macOS native transaction has an outcome")
    }
}

impl<T> Drop for TransactionWaiter<T> {
    fn drop(&mut self) {
        if self.cancel_on_drop {
            let _ = claim_cancel(&self.cell);
        }
    }
}

fn claim_with<T, H>(
    cell: &Arc<TransactionCell<T>>,
    decide: impl FnOnce(
        &mut TransactionState<T>,
    ) -> Option<(Result<T, MacosNativeTransactionError>, Option<H>)>,
) -> Option<(TransactionSettlement<T>, Option<H>)> {
    let (outcome, hook, retired_hooks, ticket) = {
        let mut state = lock(&cell.state);
        if state.claimed {
            return None;
        }
        let (outcome, hook) = decide(&mut state)?;
        state.claimed = true;
        state.deadline = None;
        // Retired hooks drop outside the state lock: a hook that captured a
        // completer clone would otherwise run the completer drop guard while
        // this cell's mutex is held.
        let retired_hooks = (state.timeout.take(), state.cancel.take());
        (outcome, hook, retired_hooks, state.deadline_ticket.take())
    };
    drop(ticket);
    drop(retired_hooks);
    Some((
        TransactionSettlement {
            cell: Arc::clone(cell),
            outcome: Some(outcome),
        },
        hook,
    ))
}

fn claim_cancel<T>(cell: &Arc<TransactionCell<T>>) -> bool {
    let Some((settlement, hook)) = claim_with(cell, move |state| {
        let generation = state.generation;
        Some((
            Err(MacosNativeTransactionError::Cancelled {
                phase: state.phase,
                generation,
            }),
            state.cancel.take().map(|hook| (hook, generation)),
        ))
    }) else {
        return false;
    };
    if let Some((hook, generation)) = hook {
        hook(generation);
    }
    settlement.publish();
    true
}

fn claim_timeout<T>(
    cell: &Arc<TransactionCell<T>>,
    deadline_revision: Option<u64>,
    timeout: Option<TransactionHook>,
) -> bool {
    let Some((settlement, hook)) = claim_with(cell, move |state| {
        if deadline_revision.is_some_and(|revision| state.deadline_revision != revision) {
            return None;
        }
        Some((
            Err(MacosNativeTransactionError::TimedOut {
                phase: state.phase,
                generation: state.generation,
            }),
            timeout.or_else(|| state.timeout.take()),
        ))
    }) else {
        return false;
    };
    if let Some(hook) = hook {
        hook();
    }
    settlement.publish();
    true
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
