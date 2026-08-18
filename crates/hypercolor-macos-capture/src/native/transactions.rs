use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::io;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::thread;
use std::time::Instant;

use crate::{MacosCaptureError, MacosProtectedSourceState};

type TransactionHook = Arc<dyn Fn() + Send + Sync + 'static>;
/// Cancel hooks receive the generation the cell held when the cancel
/// claimed it. Hooks must target that value rather than a generation
/// captured at registration time: stage adoption rekeys the cell, and a
/// captured generation goes stale the moment it does.
type TransactionCancelHook = Arc<dyn Fn(u64) + Send + Sync + 'static>;
type DeadlineCallback = Box<dyn FnOnce() + Send + 'static>;

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

struct ScheduledDeadline {
    callback: Option<DeadlineCallback>,
}

#[derive(Default)]
struct DeadlineQueue {
    deadlines: BTreeMap<(Instant, u64), ScheduledDeadline>,
    deadline_by_id: HashMap<u64, Instant>,
}

struct DeadlineSchedulerInner {
    next_id: AtomicU64,
    queue: Mutex<DeadlineQueue>,
    ready: Condvar,
}

#[derive(Clone)]
pub(super) struct DeadlineScheduler {
    inner: Arc<DeadlineSchedulerInner>,
}

pub(super) struct DeadlineTicket {
    scheduler: Weak<DeadlineSchedulerInner>,
    id: u64,
    armed: bool,
}

impl DeadlineScheduler {
    pub(super) fn start(thread_name: &str) -> io::Result<Self> {
        let scheduler = Self::manual();
        let inner = Arc::clone(&scheduler.inner);
        thread::Builder::new()
            .name(thread_name.to_owned())
            .spawn(move || deadline_loop(&inner))?;
        Ok(scheduler)
    }

    pub(super) fn manual() -> Self {
        Self {
            inner: Arc::new(DeadlineSchedulerInner {
                next_id: AtomicU64::new(1),
                queue: Mutex::new(DeadlineQueue::default()),
                ready: Condvar::new(),
            }),
        }
    }

    pub(super) fn schedule(
        &self,
        deadline: Instant,
        callback: impl FnOnce() + Send + 'static,
    ) -> io::Result<DeadlineTicket> {
        let id = self
            .inner
            .next_id
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |id| id.checked_add(1))
            .map_err(|_| io::Error::other("macOS native deadline identity exhausted"))?;
        let mut queue = lock(&self.inner.queue);
        queue.deadline_by_id.insert(id, deadline);
        queue.deadlines.insert(
            (deadline, id),
            ScheduledDeadline {
                callback: Some(Box::new(callback)),
            },
        );
        drop(queue);
        self.inner.ready.notify_one();
        Ok(DeadlineTicket {
            scheduler: Arc::downgrade(&self.inner),
            id,
            armed: true,
        })
    }

    #[cfg(test)]
    pub(super) fn expire_through(&self, now: Instant) {
        run_due_callbacks(&self.inner, now);
    }

    #[cfg(test)]
    pub(super) fn pending(&self) -> usize {
        lock(&self.inner.queue).deadlines.len()
    }
}

impl DeadlineTicket {
    fn cancel(&mut self) {
        if !self.armed {
            return;
        }
        self.armed = false;
        let Some(scheduler) = self.scheduler.upgrade() else {
            return;
        };
        let mut queue = lock(&scheduler.queue);
        if let Some(deadline) = queue.deadline_by_id.remove(&self.id) {
            queue.deadlines.remove(&(deadline, self.id));
        }
        drop(queue);
        scheduler.ready.notify_one();
    }
}

impl Drop for DeadlineTicket {
    fn drop(&mut self) {
        self.cancel();
    }
}

fn deadline_loop(scheduler: &DeadlineSchedulerInner) {
    loop {
        let queue = lock(&scheduler.queue);
        let Some((deadline, _)) = queue.deadlines.first_key_value().map(|(key, _)| *key) else {
            drop(
                scheduler
                    .ready
                    .wait(queue)
                    .unwrap_or_else(std::sync::PoisonError::into_inner),
            );
            continue;
        };
        let now = Instant::now();
        if deadline > now {
            let (waiting, _) = scheduler
                .ready
                .wait_timeout(queue, deadline.duration_since(now))
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            drop(waiting);
            continue;
        }
        drop(queue);
        run_due_callbacks(scheduler, now);
    }
}

fn run_due_callbacks(scheduler: &DeadlineSchedulerInner, now: Instant) {
    let callbacks = {
        let mut queue = lock(&scheduler.queue);
        let due = queue
            .deadlines
            .range(..=(now, u64::MAX))
            .map(|(key, _)| *key)
            .collect::<Vec<_>>();
        due.into_iter()
            .filter_map(|key| {
                queue.deadline_by_id.remove(&key.1);
                queue
                    .deadlines
                    .remove(&key)
                    .and_then(|mut deadline| deadline.callback.take())
            })
            .collect::<Vec<_>>()
    };
    for callback in callbacks {
        callback();
    }
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
    pub(super) fn new(identity: TransactionIdentity) -> Self {
        let cell = Arc::new(TransactionCell {
            state: Mutex::new(TransactionState {
                generation: identity.generation,
                phase: identity.phase,
                claimed: false,
                outcome: None,
                deadline: None,
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
            state.deadline = None;
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
        let revision = {
            let mut state = lock(&self.cell.state);
            if !gate(&mut state) {
                return Ok(false);
            }
            state.deadline_revision = state
                .deadline_revision
                .checked_add(1)
                .ok_or_else(|| io::Error::other("macOS transaction deadline revision exhausted"))?;
            state.deadline_revision
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
            state = self
                .cell
                .ready
                .wait(state)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
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

pub struct MacosStreamRequestTransaction {
    generation: u64,
    waiter: Option<TransactionWaiter<()>>,
}

impl MacosStreamRequestTransaction {
    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    #[must_use]
    pub fn current_deadline(&self) -> Option<Instant> {
        self.waiter
            .as_ref()
            .and_then(TransactionWaiter::current_deadline)
    }

    pub fn wait(mut self) -> Result<(), MacosNativeTransactionError> {
        self.waiter
            .take()
            .expect("macOS stream request transaction waits once")
            .wait()
    }

    pub fn cancel(mut self) -> bool {
        self.waiter
            .take()
            .expect("macOS stream request transaction cancels once")
            .cancel()
    }
}

impl fmt::Debug for MacosStreamRequestTransaction {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MacosStreamRequestTransaction")
            .field("generation", &self.generation)
            .field("deadline", &self.current_deadline())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
impl MacosStreamRequestTransaction {
    pub(super) fn try_recv(
        &self,
    ) -> Result<Result<(), MacosCaptureError>, std::sync::mpsc::TryRecvError> {
        self.waiter
            .as_ref()
            .and_then(TransactionWaiter::try_outcome)
            .map(map_test_request_outcome)
            .ok_or(std::sync::mpsc::TryRecvError::Empty)
    }

    pub(super) fn recv(&self) -> Result<Result<(), MacosCaptureError>, std::sync::mpsc::RecvError> {
        Ok(map_test_request_outcome(
            self.waiter
                .as_ref()
                .expect("test request transaction retains its waiter")
                .wait_outcome(),
        ))
    }
}

#[cfg(test)]
fn map_test_request_outcome(
    outcome: Result<(), MacosNativeTransactionError>,
) -> Result<(), MacosCaptureError> {
    outcome.map_err(|error| match error {
        MacosNativeTransactionError::Capture(error) => error,
        error => MacosCaptureError::CaptureWorkerStartFailed(error.to_string()),
    })
}

pub struct MacosStreamDiagnosticTransaction {
    generation: u64,
    waiter: Option<TransactionWaiter<MacosProtectedSourceState>>,
}

impl MacosStreamDiagnosticTransaction {
    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    #[must_use]
    pub fn current_deadline(&self) -> Option<Instant> {
        self.waiter
            .as_ref()
            .and_then(TransactionWaiter::current_deadline)
    }

    pub fn wait(mut self) -> Result<MacosProtectedSourceState, MacosNativeTransactionError> {
        self.waiter
            .take()
            .expect("macOS stream diagnostic transaction waits once")
            .wait()
    }

    pub fn wait_until(
        mut self,
        deadline: Instant,
    ) -> Result<MacosProtectedSourceState, MacosNativeTransactionError> {
        self.waiter
            .take()
            .expect("macOS stream diagnostic transaction waits once")
            .wait_until(deadline)
    }

    pub fn cancel(mut self) -> bool {
        self.waiter
            .take()
            .expect("macOS stream diagnostic transaction cancels once")
            .cancel()
    }
}

impl fmt::Debug for MacosStreamDiagnosticTransaction {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MacosStreamDiagnosticTransaction")
            .field("generation", &self.generation)
            .field("deadline", &self.current_deadline())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
impl MacosStreamDiagnosticTransaction {
    pub(super) fn try_recv(
        &self,
    ) -> Result<MacosProtectedSourceState, std::sync::mpsc::TryRecvError> {
        self.waiter
            .as_ref()
            .and_then(TransactionWaiter::try_outcome)
            .map(|outcome| outcome.expect("fixture diagnostic transaction succeeds"))
            .ok_or(std::sync::mpsc::TryRecvError::Empty)
    }

    pub(super) fn recv(&self) -> Result<MacosProtectedSourceState, std::sync::mpsc::RecvError> {
        Ok(self
            .waiter
            .as_ref()
            .expect("test diagnostic transaction retains its waiter")
            .wait_outcome()
            .expect("fixture diagnostic transaction succeeds"))
    }
}

pub(super) fn stream_request_transaction(
    generation: u64,
) -> (MacosStreamRequestTransaction, TransactionCompleter<()>) {
    let completer = TransactionCompleter::new(TransactionIdentity {
        generation,
        phase: MacosNativeTransactionPhase::StreamStart,
    });
    let transaction = MacosStreamRequestTransaction {
        generation,
        waiter: Some(completer.waiter()),
    };
    (transaction, completer)
}

pub(super) fn stream_diagnostic_transaction(
    generation: u64,
) -> (
    MacosStreamDiagnosticTransaction,
    TransactionCompleter<MacosProtectedSourceState>,
) {
    let completer = TransactionCompleter::new(TransactionIdentity {
        generation,
        phase: MacosNativeTransactionPhase::SourceResolution,
    });
    let transaction = MacosStreamDiagnosticTransaction {
        generation,
        waiter: Some(completer.waiter()),
    };
    (transaction, completer)
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, Barrier};
    use std::thread;
    use std::time::{Duration, Instant};

    use super::{
        DeadlineScheduler, MacosNativeTransactionError, MacosNativeTransactionPhase,
        TransactionCompleter, TransactionIdentity,
    };

    fn fixture_transaction() -> (TransactionCompleter<u64>, super::TransactionWaiter<u64>) {
        let completer = TransactionCompleter::new(TransactionIdentity {
            generation: 7,
            phase: MacosNativeTransactionPhase::StreamStart,
        });
        let waiter = completer.waiter();
        (completer, waiter)
    }

    #[test]
    fn completed_deadline_is_physically_removed() {
        let scheduler = DeadlineScheduler::manual();
        let (completer, waiter) = fixture_transaction();
        let deadline = Instant::now() + Duration::from_secs(60);
        completer
            .arm(&scheduler, deadline, || panic!("cancelled deadline fired"))
            .expect("fixture deadline schedules");
        assert_eq!(scheduler.pending(), 1);

        assert!(completer.finish(Ok(11)));
        assert_eq!(scheduler.pending(), 0);
        assert_eq!(waiter.wait(), Ok(11));
    }

    #[test]
    fn consuming_the_result_does_not_reopen_the_transaction() {
        let (completer, waiter) = fixture_transaction();

        assert!(completer.finish(Ok(11)));
        assert_eq!(waiter.wait(), Ok(11));

        assert!(!completer.is_open());
        assert!(!completer.finish(Ok(12)));
    }

    #[test]
    fn claimed_result_does_not_wake_until_published() {
        let scheduler = DeadlineScheduler::manual();
        let (completer, waiter) = fixture_transaction();
        completer
            .arm(&scheduler, Instant::now() + Duration::from_secs(60), || {})
            .expect("fixture deadline schedules");
        let settlement = completer.claim(Ok(11)).expect("success claims open cell");

        assert!(!completer.is_open());
        assert_eq!(completer.current_deadline(), None);
        assert_eq!(scheduler.pending(), 0);
        assert_eq!(waiter.try_outcome(), None);

        settlement.publish();
        assert_eq!(waiter.wait(), Ok(11));
    }

    #[test]
    fn abandoned_success_claim_publishes_failure_during_unwind() {
        let (completer, waiter) = fixture_transaction();
        let unwind = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _settlement = completer.claim(Ok(11)).expect("success claims open cell");
            panic!("fixture aborts before side effects commit");
        }));

        assert!(unwind.is_err());

        assert_eq!(
            waiter.wait(),
            Err(MacosNativeTransactionError::Cancelled {
                phase: MacosNativeTransactionPhase::StreamStart,
                generation: 7,
            })
        );
    }

    #[test]
    fn manual_deadline_settles_without_sleep_or_polling() {
        let scheduler = DeadlineScheduler::manual();
        let (completer, waiter) = fixture_transaction();
        let deadline = Instant::now() + Duration::from_secs(5);
        completer
            .arm(&scheduler, deadline, || {})
            .expect("fixture deadline schedules");

        scheduler.expire_through(deadline);

        assert_eq!(
            waiter.wait(),
            Err(MacosNativeTransactionError::TimedOut {
                phase: MacosNativeTransactionPhase::StreamStart,
                generation: 7,
            })
        );
        assert_eq!(scheduler.pending(), 0);
    }

    #[test]
    fn earlier_wait_deadline_invokes_the_registered_timeout_transaction() {
        let scheduler = DeadlineScheduler::manual();
        let (completer, waiter) = fixture_transaction();
        let scheduled = Instant::now() + Duration::from_secs(60);
        completer
            .arm(&scheduler, scheduled, || {})
            .expect("fixture deadline schedules");

        assert_eq!(
            waiter.wait_until(Instant::now()),
            Err(MacosNativeTransactionError::TimedOut {
                phase: MacosNativeTransactionPhase::StreamStart,
                generation: 7,
            })
        );
        assert_eq!(scheduler.pending(), 0);
    }

    #[test]
    fn completion_and_timeout_commit_exactly_one_result() {
        let scheduler = DeadlineScheduler::manual();
        let (completer, waiter) = fixture_transaction();
        let deadline = Instant::now() + Duration::from_secs(5);
        let barrier = Arc::new(Barrier::new(3));
        let wins = Arc::new(AtomicU64::new(0));
        let timeout_wins = Arc::clone(&wins);
        let timeout_barrier = Arc::clone(&barrier);
        completer
            .arm(&scheduler, deadline, move || {
                timeout_barrier.wait();
                timeout_wins.fetch_add(1, Ordering::AcqRel);
            })
            .expect("fixture deadline schedules");
        let completion = completer.clone();
        let completion_wins = Arc::clone(&wins);
        let completion_barrier = Arc::clone(&barrier);
        let complete = thread::spawn(move || {
            completion_barrier.wait();
            if completion.finish(Ok(19)) {
                completion_wins.fetch_add(1, Ordering::AcqRel);
            }
        });
        let expirer = thread::spawn(move || scheduler.expire_through(deadline));
        barrier.wait();
        complete.join().expect("completion race exits");
        expirer.join().expect("timeout race exits");

        assert_eq!(wins.load(Ordering::Acquire), 1);
        assert!(matches!(
            waiter.wait(),
            Ok(19)
                | Err(MacosNativeTransactionError::TimedOut {
                    phase: MacosNativeTransactionPhase::StreamStart,
                    generation: 7,
                })
        ));
    }

    #[test]
    fn preselected_timeout_cannot_override_an_unpublished_success_claim() {
        let scheduler = DeadlineScheduler::manual();
        let (completer, waiter) = fixture_transaction();
        let deadline = Instant::now() + Duration::from_secs(5);
        completer
            .arm(&scheduler, deadline, || {})
            .expect("fixture deadline schedules");
        let timeout_selected = Arc::new(Barrier::new(2));
        let selected = Arc::clone(&timeout_selected);
        let resume_timeout = Arc::new(Barrier::new(2));
        let resume = Arc::clone(&resume_timeout);
        let timeout_cell = Arc::clone(&completer.cell);
        let timeout = thread::spawn(move || {
            selected.wait();
            resume.wait();
            super::claim_timeout(&timeout_cell, None, None)
        });
        timeout_selected.wait();

        let settlement = completer.claim(Ok(19)).expect("success claims open cell");
        assert_eq!(scheduler.pending(), 0);
        assert_eq!(waiter.try_outcome(), None);
        resume_timeout.wait();
        assert!(!timeout.join().expect("preselected timeout exits"));
        assert_eq!(waiter.try_outcome(), None);

        settlement.publish();
        assert_eq!(waiter.wait(), Ok(19));
    }

    #[test]
    fn dropping_waiter_invokes_cancellation_once() {
        let (completer, waiter) = fixture_transaction();
        let cancellations = Arc::new(AtomicU64::new(0));
        let cancellation_count = Arc::clone(&cancellations);
        completer.set_cancel(move |_| {
            cancellation_count.fetch_add(1, Ordering::AcqRel);
        });

        drop(waiter);
        drop(completer);

        assert_eq!(cancellations.load(Ordering::Acquire), 1);
    }

    #[test]
    fn cancel_hook_receives_the_rekeyed_generation() {
        let (completer, waiter) = fixture_transaction();
        let observed = Arc::new(AtomicU64::new(0));
        let observed_generation = Arc::clone(&observed);
        completer.set_cancel(move |generation| {
            observed_generation.store(generation, Ordering::Release);
        });

        assert!(completer.rekey_generation(43));
        assert_eq!(completer.identity().generation, 43);

        drop(waiter);
        assert_eq!(observed.load(Ordering::Acquire), 43);
        assert_eq!(
            completer.outcome(),
            Some(Err(MacosNativeTransactionError::Cancelled {
                phase: MacosNativeTransactionPhase::StreamStart,
                generation: 43,
            }))
        );
    }

    #[test]
    fn rekeying_retires_the_previous_generations_deadline() {
        let scheduler = DeadlineScheduler::manual();
        let (completer, _waiter) = fixture_transaction();
        let deadline = Instant::now() + Duration::from_secs(5);
        completer
            .arm(&scheduler, deadline, || panic!("retired deadline fired"))
            .expect("fixture deadline schedules");
        assert_eq!(scheduler.pending(), 1);

        assert!(completer.rekey_generation(43));

        assert_eq!(scheduler.pending(), 0);
        assert_eq!(completer.current_deadline(), None);
        scheduler.expire_through(deadline);
        assert!(completer.is_open());
    }

    #[test]
    fn arm_for_a_superseded_generation_is_refused() {
        let scheduler = DeadlineScheduler::manual();
        let (completer, _waiter) = fixture_transaction();
        assert!(completer.rekey_generation(43));

        let stale = completer
            .arm_for_generation(
                &scheduler,
                Instant::now() + Duration::from_secs(5),
                7,
                MacosNativeTransactionPhase::FirstCompleteFrame,
                || panic!("stale-generation deadline fired"),
            )
            .expect("stale arm should decline without error");
        assert!(
            !stale,
            "an arm validated before a rekey must die at the cell"
        );
        assert_eq!(scheduler.pending(), 0);
        assert_eq!(
            completer.identity().phase,
            MacosNativeTransactionPhase::StreamStart,
            "a refused arm must not mutate the phase"
        );

        let current = completer
            .arm_for_generation(
                &scheduler,
                Instant::now() + Duration::from_secs(5),
                43,
                MacosNativeTransactionPhase::FirstCompleteFrame,
                || {},
            )
            .expect("current-generation arm should schedule");
        assert!(current);
        assert_eq!(scheduler.pending(), 1);
        assert_eq!(
            completer.identity().phase,
            MacosNativeTransactionPhase::FirstCompleteFrame
        );
    }

    #[test]
    fn rekeying_a_claimed_transaction_is_refused() {
        let (completer, waiter) = fixture_transaction();
        assert!(completer.finish(Ok(11)));
        assert!(!completer.rekey_generation(43));
        assert_eq!(completer.identity().generation, 7);
        assert_eq!(waiter.wait(), Ok(11));
    }

    #[test]
    fn dropping_the_last_completer_cancels_instead_of_stranding_the_waiter() {
        let (completer, waiter) = fixture_transaction();
        let clone = completer.clone();

        drop(completer);
        assert_eq!(waiter.try_outcome(), None);

        drop(clone);
        assert_eq!(
            waiter.wait_outcome(),
            Err(MacosNativeTransactionError::Cancelled {
                phase: MacosNativeTransactionPhase::StreamStart,
                generation: 7,
            })
        );
    }

    #[test]
    fn completer_drop_does_not_run_the_cancel_hook() {
        let (completer, waiter) = fixture_transaction();
        let cancellations = Arc::new(AtomicU64::new(0));
        let cancellation_count = Arc::clone(&cancellations);
        completer.set_cancel(move |_| {
            cancellation_count.fetch_add(1, Ordering::AcqRel);
        });

        drop(completer);

        assert!(matches!(
            waiter.wait_outcome(),
            Err(MacosNativeTransactionError::Cancelled { .. })
        ));
        assert_eq!(cancellations.load(Ordering::Acquire), 0);
    }

    #[test]
    fn rearming_replaces_the_prior_deadline_without_a_tombstone() {
        let scheduler = DeadlineScheduler::manual();
        let (completer, _waiter) = fixture_transaction();
        let first = Instant::now() + Duration::from_secs(5);
        let second = first + Duration::from_secs(5);
        completer
            .arm(&scheduler, first, || panic!("superseded deadline fired"))
            .expect("first deadline schedules");
        completer
            .arm(&scheduler, second, || {})
            .expect("second deadline schedules");

        assert_eq!(completer.current_deadline(), Some(second));
        assert_eq!(scheduler.pending(), 1);
        scheduler.expire_through(first);
        assert!(completer.is_open());
    }
}
