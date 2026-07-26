//! Process-wide ownership of the Raw Input registration.
//!
//! Raw Input registration is process-global per `(usUsagePage, usUsage)`, and
//! that turns a detached pump into a live hazard: the old pump eventually
//! unwedges, runs its teardown, calls `RegisterRawInputDevices` with
//! `RIDEV_REMOVE`, and deregisters the *replacement* session. Nothing looks
//! broken — every stale batch is already epoch-rejected — input simply stops
//! arriving, from a thread nobody is watching.
//!
//! So a worker must prove it still owns the registration before removing it.
//! The proof and the action have to be taken *together*: an atomic
//! compare-and-swap orders the claim against itself but not against
//! `RegisterRawInputDevices`, which is the process-global state actually
//! contended, and neither ordering of the two survives.
//!
//! - Claim first, then register: if the replacement's registration fails, the
//!   claim has already rotated, so the old worker's teardown declines to
//!   remove and the previous registration is orphaned — still pointing at a
//!   window about to be destroyed, with nobody willing to remove it.
//! - Register first, then claim: between the two, a stale worker can observe
//!   itself as owner, pass the check, and remove the replacement's
//!   registration that landed microseconds earlier.
//!
//! Both windows are microseconds wide and both are reachable, because
//! toggling capture is exactly the workload that creates a replacement while
//! the old pump is still unwinding. The fix is to make the *pair* atomic
//! rather than each half: one lock spans registration-plus-publication and
//! owner-check-plus-removal, so those interleavings are unrepresentable
//! rather than merely unlikely.

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

/// Allocates the generation each session identifies itself by.
static NEXT_GENERATION: AtomicU64 = AtomicU64::new(1);

/// Take the next registration generation. Never returns 0, so a zero can
/// never be mistaken for a live owner.
pub fn next_generation() -> u64 {
    NEXT_GENERATION.fetch_add(1, Ordering::Relaxed)
}

/// Which generation currently owns the process's Raw Input registration.
///
/// Generic over the registration calls so the interleavings above can be
/// driven directly in tests, on any platform, rather than raced against a
/// real Win32 call.
#[derive(Debug)]
pub struct RegistrationClaim {
    owner: Mutex<Option<u64>>,
}

impl RegistrationClaim {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            owner: Mutex::new(None),
        }
    }

    /// Register and publish the claim as one critical section.
    ///
    /// On registration failure the claim is left untouched, so the previous
    /// owner keeps both the registration and the responsibility for removing
    /// it, and the caller's `start()` fails with no process-global state
    /// mutated.
    ///
    /// # Errors
    ///
    /// Returns whatever `register` returned.
    pub fn acquire<E>(
        &self,
        generation: u64,
        register: impl FnOnce() -> Result<(), E>,
    ) -> Result<(), E> {
        let mut owner = self.lock();
        register()?;
        *owner = Some(generation);
        Ok(())
    }

    /// Check ownership and deregister as one critical section.
    ///
    /// Returns whether removal actually ran. A worker that no longer holds the
    /// claim releases the lock immediately and touches nothing — it destroys
    /// only its own window, which is thread-affine to it and harmless.
    ///
    /// # Errors
    ///
    /// Returns whatever `remove` returned. The claim is cleared either way:
    /// a removal that failed still leaves no live registration this session
    /// could meaningfully retry against.
    pub fn release<E>(
        &self,
        generation: u64,
        remove: impl FnOnce() -> Result<(), E>,
    ) -> Result<bool, E> {
        let mut owner = self.lock();
        if *owner != Some(generation) {
            return Ok(false);
        }
        let outcome = remove();
        *owner = None;
        outcome.map(|()| true)
    }

    /// Generation currently holding the registration, if any.
    #[must_use]
    pub fn owner(&self) -> Option<u64> {
        *self.lock()
    }

    /// A poisoned claim is recoverable: the data is a single `Option<u64>`
    /// that every writer leaves in a consistent state, and refusing to
    /// register for the rest of the process lifetime because an unrelated
    /// panic crossed this lock would be strictly worse.
    fn lock(&self) -> std::sync::MutexGuard<'_, Option<u64>> {
        self.owner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl Default for RegistrationClaim {
    fn default() -> Self {
        Self::new()
    }
}

/// The one claim guarding this process's registration.
pub static PROCESS_CLAIM: RegistrationClaim = RegistrationClaim::new();
