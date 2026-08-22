use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Barrier, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use super::{
    CompletionDisposition, CompletionFence, CompletionWitness, DeadlineScheduler, NativeLifecycle,
    RetirementExecutor, lock,
};

#[derive(Default)]
struct ControlledExecutor {
    jobs: Mutex<VecDeque<Box<dyn FnOnce() + Send + 'static>>>,
}

impl ControlledExecutor {
    fn take(&self) -> Box<dyn FnOnce() + Send + 'static> {
        lock(&self.jobs)
            .pop_front()
            .expect("controlled retirement job is pending")
    }
}

impl RetirementExecutor for ControlledExecutor {
    fn execute(&self, job: Box<dyn FnOnce() + Send + 'static>) {
        lock(&self.jobs).push_back(job);
    }
}

struct DropProbe(Arc<AtomicU64>);

impl Drop for DropProbe {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::AcqRel);
    }
}

struct FenceOwner {
    _completion: CompletionFence,
    _drop: DropProbe,
}

fn lifecycle() -> (NativeLifecycle, Arc<ControlledExecutor>) {
    let executor = Arc::new(ControlledExecutor::default());
    (
        NativeLifecycle::with_parts(
            DeadlineScheduler::manual(),
            Arc::clone(&executor) as Arc<dyn RetirementExecutor>,
        ),
        executor,
    )
}

fn completed_fence() -> CompletionFence {
    let fence = CompletionFence::new();
    assert!(fence.witness().complete());
    fence
}

#[test]
fn completion_witness_invokes_observers_exactly_once() {
    let fence = CompletionFence::new();
    let witness = fence.witness();
    let calls = Arc::new(AtomicU64::new(0));
    let observed = Arc::clone(&calls);
    fence.observe(move |disposition| {
        assert_eq!(disposition, CompletionDisposition::Invoked);
        observed.fetch_add(1, Ordering::AcqRel);
    });

    assert!(witness.complete());
    assert!(!witness.complete());
    drop(witness);

    assert_eq!(calls.load(Ordering::Acquire), 1);
}

#[test]
fn destroying_completion_witness_settles_the_fence() {
    let fence = CompletionFence::new();
    let witness = fence.witness();
    let disposition = Arc::new(Mutex::new(None));
    let observed = Arc::clone(&disposition);
    fence.observe(move |value| *lock(&observed) = Some(value));

    drop(witness);

    assert_eq!(*lock(&disposition), Some(CompletionDisposition::Destroyed));
}

#[test]
fn stop_timeout_keeps_owner_quarantined_until_late_completion() {
    let (lifecycle, executor) = lifecycle();
    let drops = Arc::new(AtomicU64::new(0));
    let stop_witness = Arc::new(Mutex::new(None::<CompletionWitness>));
    let captured_witness = Arc::clone(&stop_witness);
    let timeouts = Arc::new(AtomicU64::new(0));
    let timeout_count = Arc::clone(&timeouts);
    let deadline = Instant::now() + Duration::from_secs(5);
    lifecycle.retire(
        DropProbe(Arc::clone(&drops)),
        completed_fence(),
        deadline,
        move |_, witness| *lock(&captured_witness) = Some(witness),
        move || {
            timeout_count.fetch_add(1, Ordering::AcqRel);
        },
    );
    executor.take()();
    lifecycle.inner.deadlines.expire_through(deadline);

    assert_eq!(timeouts.load(Ordering::Acquire), 1);
    assert_eq!(drops.load(Ordering::Acquire), 0);
    assert_eq!(lifecycle.pending_retirements(), 1);

    assert!(
        lock(&stop_witness)
            .take()
            .expect("late stop completion remains owned")
            .complete()
    );

    assert_eq!(lifecycle.pending_retirements(), 0);
    assert_eq!(drops.load(Ordering::Acquire), 1);
}

#[test]
fn completed_stop_beats_a_dequeued_timeout_before_its_claim() {
    let (lifecycle, executor) = lifecycle();
    let drops = Arc::new(AtomicU64::new(0));
    let stop_witness = Arc::new(Mutex::new(None::<CompletionWitness>));
    let captured_witness = Arc::clone(&stop_witness);
    let timeouts = Arc::new(AtomicU64::new(0));
    let timeout_count = Arc::clone(&timeouts);
    let timeout_dequeued = Arc::new(Barrier::new(2));
    let timeout_observed = Arc::clone(&timeout_dequeued);
    let resume_timeout = Arc::new(Barrier::new(2));
    let timeout_resume = Arc::clone(&resume_timeout);
    let deadline = Instant::now() + Duration::from_secs(5);
    lifecycle.retire_with_timeout_dequeued(
        DropProbe(Arc::clone(&drops)),
        completed_fence(),
        deadline,
        move |_, witness| *lock(&captured_witness) = Some(witness),
        move || {
            timeout_count.fetch_add(1, Ordering::AcqRel);
        },
        move || {
            timeout_observed.wait();
            timeout_resume.wait();
        },
    );
    executor.take()();

    let deadlines = lifecycle.inner.deadlines.clone();
    let timeout = thread::spawn(move || deadlines.expire_through(deadline));
    timeout_dequeued.wait();

    let witness = lock(&stop_witness)
        .take()
        .expect("native stop completion remains owned");
    assert!(witness.complete());
    assert!(!witness.complete());
    assert_eq!(lifecycle.pending_retirements(), 0);
    assert_eq!(drops.load(Ordering::Acquire), 0);

    resume_timeout.wait();
    timeout.join().expect("dequeued timeout exits");

    assert_eq!(timeouts.load(Ordering::Acquire), 0);
    assert_eq!(drops.load(Ordering::Acquire), 1);
}

#[test]
fn completion_destruction_releases_timed_out_owner() {
    let (lifecycle, executor) = lifecycle();
    let drops = Arc::new(AtomicU64::new(0));
    let stop_witness = Arc::new(Mutex::new(None::<CompletionWitness>));
    let captured_witness = Arc::clone(&stop_witness);
    let deadline = Instant::now() + Duration::from_secs(5);
    lifecycle.retire(
        DropProbe(Arc::clone(&drops)),
        completed_fence(),
        deadline,
        move |_, witness| *lock(&captured_witness) = Some(witness),
        || {},
    );
    executor.take()();
    lifecycle.inner.deadlines.expire_through(deadline);

    drop(lock(&stop_witness).take());

    assert_eq!(lifecycle.pending_retirements(), 0);
    assert_eq!(drops.load(Ordering::Acquire), 1);
}

#[test]
fn dropping_lifecycle_tears_down_a_missing_completion_quarantine() {
    let (lifecycle, executor) = lifecycle();
    let drops = Arc::new(AtomicU64::new(0));
    let start_completion = CompletionFence::new();
    let stop_witness = Arc::new(Mutex::new(None::<CompletionWitness>));
    let captured_witness = Arc::clone(&stop_witness);
    lifecycle.retire(
        FenceOwner {
            _completion: start_completion.clone(),
            _drop: DropProbe(Arc::clone(&drops)),
        },
        start_completion,
        Instant::now() + Duration::from_secs(5),
        move |_, witness| *lock(&captured_witness) = Some(witness),
        || {},
    );
    executor.take()();
    assert_eq!(lifecycle.pending_retirements(), 1);

    drop(lifecycle);

    assert_eq!(drops.load(Ordering::Acquire), 1);
    drop(lock(&stop_witness).take());
}

#[test]
fn independent_retirements_have_no_serial_head_of_line_blocking() {
    let (lifecycle, executor) = lifecycle();
    let blocked = Arc::new(Barrier::new(2));
    let release = Arc::new(Barrier::new(2));
    let first_blocked = Arc::clone(&blocked);
    let first_release = Arc::clone(&release);
    let first_done = Arc::new(AtomicU64::new(0));
    let first_result = Arc::clone(&first_done);
    lifecycle.retire_without_native_stop((), completed_fence(), move |_| {
        first_blocked.wait();
        first_release.wait();
        first_result.fetch_add(1, Ordering::AcqRel);
    });
    let second_done = Arc::new(AtomicU64::new(0));
    let second_result = Arc::clone(&second_done);
    lifecycle.retire_without_native_stop((), completed_fence(), move |_| {
        second_result.fetch_add(1, Ordering::AcqRel);
    });
    let first = executor.take();
    let second = executor.take();
    let first_thread = thread::spawn(first);
    blocked.wait();

    second();

    assert_eq!(second_done.load(Ordering::Acquire), 1);
    assert_eq!(first_done.load(Ordering::Acquire), 0);
    release.wait();
    first_thread.join().expect("blocked retirement exits");
    assert_eq!(lifecycle.pending_retirements(), 0);
}
