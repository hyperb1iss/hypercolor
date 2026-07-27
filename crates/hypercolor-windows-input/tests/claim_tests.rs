//! Deterministic tests for process-wide Raw Input registration ownership.

use std::cell::Cell;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Barrier};

use hypercolor_windows_input::claim::{ClaimStage, RegistrationClaim, next_generation};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ClaimFailure {
    Cancelled,
    Registration,
    Verification,
    Rollback,
}

fn acquire(claim: &RegistrationClaim, generation: u64) -> Result<(), ClaimFailure> {
    claim.acquire(generation, |_| Ok(()), || Ok(()), || Ok(()), || Ok(()))
}

#[test]
fn a_fresh_claim_has_no_owner() {
    let claim = RegistrationClaim::new();
    assert_eq!(claim.owner(), None);
}

#[test]
fn generations_are_unique_and_never_zero() {
    let first = next_generation();
    let second = next_generation();
    assert_ne!(first, second);
    assert_ne!(first, 0);
    assert_ne!(second, 0);
}

#[test]
fn a_successful_transaction_publishes_the_claim() {
    let claim = RegistrationClaim::new();
    assert_eq!(acquire(&claim, 7), Ok(()));
    assert_eq!(claim.owner(), Some(7));
}

#[test]
fn a_failed_registration_invalidates_uncertain_previous_ownership() {
    let claim = RegistrationClaim::new();
    acquire(&claim, 1).expect("first registration succeeds");
    let rollbacks = Cell::new(0usize);

    let result = claim.acquire(
        2,
        |_| Ok(()),
        || Err(ClaimFailure::Registration),
        || Ok(()),
        || {
            rollbacks.set(rollbacks.get() + 1);
            Ok(())
        },
    );

    assert_eq!(result, Err(ClaimFailure::Registration));
    assert_eq!(rollbacks.get(), 1);
    assert_eq!(claim.owner(), None);
}

#[test]
fn rollback_failure_preserves_the_registration_diagnostic() {
    let claim = RegistrationClaim::new();
    let result = claim.acquire(
        3,
        |_| Ok(()),
        || Err(ClaimFailure::Registration),
        || Ok(()),
        || Err(ClaimFailure::Rollback),
    );

    assert_eq!(result, Err(ClaimFailure::Registration));
    assert_eq!(claim.owner(), None);
}

#[test]
fn cancellation_at_each_stage_has_transactional_effects() {
    for cancelled_stage in [
        ClaimStage::Registration,
        ClaimStage::Verification,
        ClaimStage::Publication,
    ] {
        let claim = RegistrationClaim::new();
        acquire(&claim, 1).expect("previous registration succeeds");
        let registrations = Cell::new(0usize);
        let verifications = Cell::new(0usize);
        let rollbacks = Cell::new(0usize);

        let result = claim.acquire(
            2,
            |stage| {
                if stage == cancelled_stage {
                    Err(ClaimFailure::Cancelled)
                } else {
                    Ok(())
                }
            },
            || {
                registrations.set(registrations.get() + 1);
                Ok(())
            },
            || {
                verifications.set(verifications.get() + 1);
                Ok(())
            },
            || {
                rollbacks.set(rollbacks.get() + 1);
                Ok(())
            },
        );

        assert_eq!(result, Err(ClaimFailure::Cancelled));
        if cancelled_stage == ClaimStage::Registration {
            assert_eq!(registrations.get(), 0);
            assert_eq!(verifications.get(), 0);
            assert_eq!(rollbacks.get(), 0);
            assert_eq!(claim.owner(), Some(1));
        } else {
            assert_eq!(registrations.get(), 1);
            assert_eq!(rollbacks.get(), 1);
            assert_eq!(claim.owner(), None);
            let expected_verifications = usize::from(cancelled_stage == ClaimStage::Publication);
            assert_eq!(verifications.get(), expected_verifications);
        }
    }
}

#[test]
fn failed_verification_requests_an_ownership_checked_rollback() {
    let claim = RegistrationClaim::new();
    let rollbacks = Cell::new(0usize);

    let result = claim.acquire(
        3,
        |_| Ok(()),
        || Ok(()),
        || Err(ClaimFailure::Verification),
        || {
            rollbacks.set(rollbacks.get() + 1);
            Ok(())
        },
    );

    assert_eq!(result, Err(ClaimFailure::Verification));
    assert_eq!(rollbacks.get(), 1);
    assert_eq!(claim.owner(), None);
}

#[test]
fn rollback_failure_preserves_the_cancellation_diagnostic() {
    let claim = RegistrationClaim::new();
    let result = claim.acquire(
        3,
        |stage| {
            if stage == ClaimStage::Publication {
                Err(ClaimFailure::Cancelled)
            } else {
                Ok(())
            }
        },
        || Ok(()),
        || Ok(()),
        || Err(ClaimFailure::Rollback),
    );

    assert_eq!(result, Err(ClaimFailure::Cancelled));
    assert_eq!(claim.owner(), None);
}

#[test]
fn the_owner_removes_and_clears_the_claim() {
    let claim = RegistrationClaim::new();
    acquire(&claim, 1).expect("registration succeeds");
    let removals = Cell::new(0usize);

    let result = claim.release(1, || {
        removals.set(removals.get() + 1);
        Ok::<(), ClaimFailure>(())
    });

    assert_eq!(result, Ok(true));
    assert_eq!(removals.get(), 1);
    assert_eq!(claim.owner(), None);
}

#[test]
fn stale_release_cannot_deregister_its_successor() {
    let claim = RegistrationClaim::new();
    acquire(&claim, 1).expect("first registration succeeds");
    acquire(&claim, 2).expect("replacement registration succeeds");
    let removals = Cell::new(0usize);

    let result = claim.release(1, || {
        removals.set(removals.get() + 1);
        Ok::<(), ClaimFailure>(())
    });

    assert_eq!(result, Ok(false));
    assert_eq!(removals.get(), 0);
    assert_eq!(claim.owner(), Some(2));
}

#[test]
fn stale_owned_cleanup_runs_without_clearing_the_successor() {
    let claim = RegistrationClaim::new();
    acquire(&claim, 1).expect("first registration succeeds");
    acquire(&claim, 2).expect("replacement registration succeeds");
    let cleanups = Cell::new(0usize);

    let result = claim.release_owned(1, || {
        cleanups.set(cleanups.get() + 1);
        Ok::<(), ClaimFailure>(())
    });

    assert_eq!(result, Ok(false));
    assert_eq!(cleanups.get(), 1);
    assert_eq!(claim.owner(), Some(2));
}

#[test]
fn releasing_twice_removes_once() {
    let claim = RegistrationClaim::new();
    acquire(&claim, 1).expect("registration succeeds");
    let removals = Cell::new(0usize);

    for expected in [true, false] {
        let result = claim.release(1, || {
            removals.set(removals.get() + 1);
            Ok::<(), ClaimFailure>(())
        });
        assert_eq!(result, Ok(expected));
    }

    assert_eq!(removals.get(), 1);
}

#[test]
fn failed_removal_still_clears_the_claim() {
    let claim = RegistrationClaim::new();
    acquire(&claim, 1).expect("registration succeeds");

    assert_eq!(
        claim.release(1, || Err(ClaimFailure::Rollback)),
        Err(ClaimFailure::Rollback)
    );
    assert_eq!(claim.owner(), None);
}

#[test]
fn cancelled_claimant_cannot_register_late_or_displace_successor() {
    let claim = Arc::new(RegistrationClaim::new());
    let registered = Arc::new(Barrier::new(2));
    let continue_claim = Arc::new(Barrier::new(2));
    let cancelled = Arc::new(AtomicBool::new(false));
    let rollbacks = Arc::new(AtomicUsize::new(0));

    let claimant = {
        let claim = Arc::clone(&claim);
        let registered = Arc::clone(&registered);
        let continue_claim = Arc::clone(&continue_claim);
        let cancelled = Arc::clone(&cancelled);
        let rollbacks = Arc::clone(&rollbacks);
        std::thread::spawn(move || {
            claim.acquire(
                1,
                |_| {
                    if cancelled.load(Ordering::Acquire) {
                        Err(ClaimFailure::Cancelled)
                    } else {
                        Ok(())
                    }
                },
                || {
                    registered.wait();
                    continue_claim.wait();
                    Ok(())
                },
                || Ok(()),
                || {
                    rollbacks.fetch_add(1, Ordering::Relaxed);
                    Ok(())
                },
            )
        })
    };

    registered.wait();
    cancelled.store(true, Ordering::Release);
    continue_claim.wait();
    assert_eq!(
        claimant.join().expect("claimant joins"),
        Err(ClaimFailure::Cancelled)
    );
    assert_eq!(acquire(&claim, 2), Ok(()));
    assert_eq!(claim.owner(), Some(2));
    assert_eq!(rollbacks.load(Ordering::Relaxed), 1);

    let stale_removals = Cell::new(0usize);
    assert_eq!(
        claim.release(1, || {
            stale_removals.set(stale_removals.get() + 1);
            Ok::<(), ClaimFailure>(())
        }),
        Ok(false)
    );
    assert_eq!(stale_removals.get(), 0);
    assert_eq!(claim.owner(), Some(2));
}

#[test]
fn concurrent_claimants_publish_exactly_one_owner_at_a_time() {
    let claim = Arc::new(RegistrationClaim::new());
    let start = Arc::new(Barrier::new(3));
    let active_registrations = Arc::new(AtomicUsize::new(0));
    let peak_registrations = Arc::new(AtomicUsize::new(0));

    let workers: Vec<_> = (1..=2)
        .map(|generation| {
            let claim = Arc::clone(&claim);
            let start = Arc::clone(&start);
            let active_registrations = Arc::clone(&active_registrations);
            let peak_registrations = Arc::clone(&peak_registrations);
            std::thread::spawn(move || {
                start.wait();
                claim
                    .acquire(
                        generation,
                        |_| Ok::<(), ClaimFailure>(()),
                        || {
                            let active = active_registrations.fetch_add(1, Ordering::SeqCst) + 1;
                            peak_registrations.fetch_max(active, Ordering::SeqCst);
                            std::thread::yield_now();
                            active_registrations.fetch_sub(1, Ordering::SeqCst);
                            Ok(())
                        },
                        || Ok(()),
                        || Ok(()),
                    )
                    .expect("registration succeeds");
            })
        })
        .collect();

    start.wait();
    for worker in workers {
        worker.join().expect("worker joins");
    }

    assert_eq!(active_registrations.load(Ordering::SeqCst), 0);
    assert_eq!(peak_registrations.load(Ordering::SeqCst), 1);
    assert!(matches!(claim.owner(), Some(1 | 2)));
}
