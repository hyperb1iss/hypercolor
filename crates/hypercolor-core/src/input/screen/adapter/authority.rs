use std::num::NonZeroU64;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::input::screen) struct CaptureSessionAuthority(NonZeroU64);

impl CaptureSessionAuthority {
    pub(in crate::input::screen) fn new(generation: u64) -> Self {
        Self(NonZeroU64::new(generation).expect("capture session authority must be nonzero"))
    }

    pub(in crate::input::screen) const fn generation(self) -> u64 {
        self.0.get()
    }
}

#[derive(Debug, thiserror::Error)]
#[error("capture session authority generation exhausted")]
pub(in crate::input::screen) struct CaptureSessionAuthorityExhausted;

#[derive(Debug, thiserror::Error)]
#[error("capture session authority reservation is stale")]
pub(in crate::input::screen) struct StaleCaptureSessionReservation;

pub(in crate::input::screen) struct ReservedCaptureSessionAuthority {
    authority: CaptureSessionAuthority,
    issuer: Arc<CaptureSessionAuthorityState>,
}

impl ReservedCaptureSessionAuthority {
    pub(in crate::input::screen) const fn authority(&self) -> CaptureSessionAuthority {
        self.authority
    }
}

#[derive(Default)]
struct CaptureSessionAuthorityState {
    next: AtomicU64,
    current: AtomicU64,
}

#[derive(Default)]
pub(in crate::input::screen) struct CaptureSessionAuthoritySequencer {
    state: Arc<CaptureSessionAuthorityState>,
}

impl CaptureSessionAuthoritySequencer {
    pub(in crate::input::screen) fn reserve(
        &self,
    ) -> Result<ReservedCaptureSessionAuthority, CaptureSessionAuthorityExhausted> {
        let generation = self
            .state
            .next
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |generation| {
                generation.checked_add(1)
            })
            .map_err(|_| CaptureSessionAuthorityExhausted)?
            + 1;
        Ok(ReservedCaptureSessionAuthority {
            authority: CaptureSessionAuthority::new(generation),
            issuer: Arc::clone(&self.state),
        })
    }

    #[cfg(test)]
    pub(in crate::input::screen) fn current(&self) -> Option<CaptureSessionAuthority> {
        let generation = self.state.current.load(Ordering::Acquire);
        (generation != 0).then(|| CaptureSessionAuthority::new(generation))
    }

    pub(in crate::input::screen) fn is_current(&self, authority: CaptureSessionAuthority) -> bool {
        self.state.current.load(Ordering::Acquire) == authority.generation()
    }

    #[cfg(target_os = "linux")]
    pub(in crate::input::screen) fn can_commit(
        &self,
        reservation: &ReservedCaptureSessionAuthority,
    ) -> bool {
        Arc::ptr_eq(&self.state, &reservation.issuer)
            && self.state.current.load(Ordering::Acquire) < reservation.authority.generation()
    }

    pub(in crate::input::screen) fn commit(
        &self,
        reservation: ReservedCaptureSessionAuthority,
    ) -> Result<CaptureSessionAuthority, StaleCaptureSessionReservation> {
        let authority = reservation.authority;
        if !Arc::ptr_eq(&self.state, &reservation.issuer) {
            return Err(StaleCaptureSessionReservation);
        }
        self.state
            .current
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                (current < authority.generation()).then_some(authority.generation())
            })
            .map_err(|_| StaleCaptureSessionReservation)?;
        Ok(authority)
    }
}
