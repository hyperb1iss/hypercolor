//! Registration ownership, driven directly rather than raced.
//!
//! The interleavings these cover are microseconds wide in production and
//! effectively impossible to reproduce by racing threads, which is why the
//! claim takes an injectable registration call: the orderings that a
//! compare-and-swap could not survive are forced here, deterministically.

use std::cell::Cell;

use hypercolor_windows_input::claim::{RegistrationClaim, next_generation};

#[derive(Debug, PartialEq, Eq)]
struct RegistrationFailed;

#[test]
fn a_fresh_claim_has_no_owner() {
    let claim = RegistrationClaim::new();
    assert_eq!(claim.owner(), None);
}

#[test]
fn generations_are_unique_and_never_zero() {
    // Zero is reserved so it can never be mistaken for a live owner.
    let first = next_generation();
    let second = next_generation();
    assert_ne!(first, second);
    assert_ne!(first, 0);
    assert_ne!(second, 0);
}

#[test]
fn a_successful_registration_publishes_the_claim() {
    let claim = RegistrationClaim::new();
    let result: Result<(), RegistrationFailed> = claim.acquire(7, || Ok(()));
    assert!(result.is_ok());
    assert_eq!(claim.owner(), Some(7));
}

#[test]
fn a_failed_registration_leaves_the_previous_owner_installed() {
    // This is the ordering a claim-then-register CAS gets wrong: rotating the
    // claim first would orphan the old registration, leaving it pointing at a
    // window about to be destroyed with nobody willing to remove it.
    let claim = RegistrationClaim::new();
    claim
        .acquire(1, || Ok::<(), RegistrationFailed>(()))
        .expect("first registration succeeds");

    let result = claim.acquire(2, || Err(RegistrationFailed));
    assert_eq!(result, Err(RegistrationFailed));
    assert_eq!(
        claim.owner(),
        Some(1),
        "the previous owner keeps both the registration and the duty to remove it"
    );
}

#[test]
fn the_owner_removes_and_clears_the_claim() {
    let claim = RegistrationClaim::new();
    claim
        .acquire(1, || Ok::<(), RegistrationFailed>(()))
        .expect("registration succeeds");

    let removed = Cell::new(false);
    let outcome = claim.release(1, || {
        removed.set(true);
        Ok::<(), RegistrationFailed>(())
    });

    assert_eq!(outcome, Ok(true));
    assert!(removed.get(), "the owner actually deregisters");
    assert_eq!(claim.owner(), None);
}

#[test]
fn a_stale_worker_does_not_deregister_its_replacement() {
    // The hazard the whole mechanism exists for: registration is process-global,
    // so a detached pump running its teardown late would otherwise tear down
    // whichever session is now live. Nothing would look broken — every stale
    // batch is already epoch-rejected — input would simply stop arriving.
    let claim = RegistrationClaim::new();
    claim
        .acquire(1, || Ok::<(), RegistrationFailed>(()))
        .expect("the first session registers");
    claim
        .acquire(2, || Ok::<(), RegistrationFailed>(()))
        .expect("the replacement registers");

    let removed = Cell::new(false);
    let outcome = claim.release(1, || {
        removed.set(true);
        Ok::<(), RegistrationFailed>(())
    });

    assert_eq!(outcome, Ok(false));
    assert!(
        !removed.get(),
        "the stale worker must not call RIDEV_REMOVE at all"
    );
    assert_eq!(
        claim.owner(),
        Some(2),
        "the live session keeps its registration"
    );
}

#[test]
fn releasing_twice_removes_once() {
    let claim = RegistrationClaim::new();
    claim
        .acquire(1, || Ok::<(), RegistrationFailed>(()))
        .expect("registration succeeds");

    let removals = Cell::new(0u32);
    let remove_once = || {
        claim.release(1, || {
            removals.set(removals.get() + 1);
            Ok::<(), RegistrationFailed>(())
        })
    };
    assert_eq!(remove_once(), Ok(true));
    assert_eq!(remove_once(), Ok(false));
    assert_eq!(removals.get(), 1, "teardown is idempotent");
}

#[test]
fn a_failed_removal_still_clears_the_claim() {
    // A removal that failed leaves no live registration this session could
    // meaningfully retry against, and pinning the slot to a dead generation
    // would block every future session from ever registering.
    let claim = RegistrationClaim::new();
    claim
        .acquire(1, || Ok::<(), RegistrationFailed>(()))
        .expect("registration succeeds");

    let outcome = claim.release(1, || Err(RegistrationFailed));
    assert_eq!(outcome, Err(RegistrationFailed));
    assert_eq!(claim.owner(), None);
}

#[test]
fn a_start_stop_cycle_leaves_the_slot_empty() {
    let claim = RegistrationClaim::new();
    for generation in 1..=3 {
        claim
            .acquire(generation, || Ok::<(), RegistrationFailed>(()))
            .expect("registration succeeds");
        assert_eq!(claim.owner(), Some(generation));
        assert_eq!(
            claim.release(generation, || Ok::<(), RegistrationFailed>(())),
            Ok(true)
        );
        assert_eq!(claim.owner(), None);
    }
}

#[test]
fn a_replacement_registering_during_a_stale_teardown_is_serialized() {
    // The register-then-claim ordering a CAS gets wrong in the other direction:
    // a stale worker observing itself as owner between the two calls could
    // remove a replacement that landed microseconds earlier. Because the owner
    // check and the removal share the critical section with any competing
    // registration, the only two orderings possible are both safe, and this
    // asserts the one that matters.
    let claim = RegistrationClaim::new();
    claim
        .acquire(1, || Ok::<(), RegistrationFailed>(()))
        .expect("the first session registers");

    // The replacement's registration completes inside its own critical
    // section, so by the time the stale release runs, ownership has moved.
    let registrations = Cell::new(0u32);
    claim
        .acquire(2, || {
            registrations.set(registrations.get() + 1);
            Ok::<(), RegistrationFailed>(())
        })
        .expect("the replacement registers");

    let removals = Cell::new(0u32);
    let stale = claim.release(1, || {
        removals.set(removals.get() + 1);
        Ok::<(), RegistrationFailed>(())
    });

    assert_eq!(stale, Ok(false));
    assert_eq!(registrations.get(), 1);
    assert_eq!(removals.get(), 0);
    assert_eq!(claim.owner(), Some(2));
}
