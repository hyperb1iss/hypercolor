use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::{Duration, Instant};

use super::{
    DeadlineScheduler, MacosNativeTransactionError, MacosNativeTransactionPhase,
    TransactionCompleter, TransactionIdentity,
};

fn fixture_transaction() -> (TransactionCompleter<u64>, super::TransactionWaiter<u64>) {
    let completer = TransactionCompleter::new(
        TransactionIdentity {
            generation: 7,
            phase: MacosNativeTransactionPhase::StreamStart,
        },
        None,
    );
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
    assert_eq!(completer.current_deadline(), Some(deadline));
    scheduler.expire_through(deadline);
    assert!(completer.is_open());
}

#[test]
fn phase_rearm_preserves_the_transactions_absolute_deadline() {
    let scheduler = DeadlineScheduler::manual();
    let deadline = Instant::now() + Duration::from_secs(5);
    let completer = TransactionCompleter::<u64>::new(
        TransactionIdentity {
            generation: 7,
            phase: MacosNativeTransactionPhase::StreamStart,
        },
        Some(deadline),
    );
    let _waiter = completer.waiter();
    completer
        .arm_for_generation(
            &scheduler,
            deadline,
            7,
            MacosNativeTransactionPhase::StreamStart,
            || {},
        )
        .expect("start phase arms");

    let proposed_reset = deadline + Duration::from_secs(5);
    completer
        .arm_for_generation(
            &scheduler,
            proposed_reset,
            7,
            MacosNativeTransactionPhase::FirstCompleteFrame,
            || {},
        )
        .expect("first-frame phase rearms");

    assert_eq!(completer.current_deadline(), Some(deadline));
    assert_eq!(scheduler.pending(), 1);
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
fn rearming_replaces_the_ticket_without_extending_the_absolute_deadline() {
    let scheduler = DeadlineScheduler::manual();
    let (completer, waiter) = fixture_transaction();
    let first = Instant::now() + Duration::from_secs(5);
    let second = first + Duration::from_secs(5);
    completer
        .arm(&scheduler, first, || panic!("superseded deadline fired"))
        .expect("first deadline schedules");
    completer
        .arm(&scheduler, second, || {})
        .expect("second deadline schedules");

    assert_eq!(completer.current_deadline(), Some(first));
    assert_eq!(scheduler.pending(), 1);
    scheduler.expire_through(first);
    assert!(matches!(
        waiter.wait(),
        Err(MacosNativeTransactionError::TimedOut { .. })
    ));
}
