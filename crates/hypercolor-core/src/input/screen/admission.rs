//! Shared lock-free byte admission for screen capture resources.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use arc_swap::ArcSwap;
use thiserror::Error;

use super::{
    ScreenAdmissionCapacity, ScreenAnalysisComputeCapacity, ScreenAnalysisResourcePlan,
    ScreenAnalysisWorkPlan, ScreenCaptureDemand,
};

/// Immutable manager-owned screen capacity policy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScreenCapacityPolicySnapshot {
    capacity_enforced: bool,
    total_capacity: ScreenAdmissionCapacity,
    publication_capacity: ScreenAdmissionCapacity,
    capture_demand: ScreenCaptureDemand,
    analysis_resource_plan: Option<ScreenAnalysisResourcePlan>,
    analysis_work_plan: Option<ScreenAnalysisWorkPlan>,
    analysis_compute_capacity: Option<ScreenAnalysisComputeCapacity>,
}

impl ScreenCapacityPolicySnapshot {
    pub(crate) const fn new(
        capacity_enforced: bool,
        total_capacity: ScreenAdmissionCapacity,
        publication_capacity: ScreenAdmissionCapacity,
        capture_demand: ScreenCaptureDemand,
        analysis_resource_plan: Option<ScreenAnalysisResourcePlan>,
        analysis_work_plan: Option<ScreenAnalysisWorkPlan>,
        analysis_compute_capacity: Option<ScreenAnalysisComputeCapacity>,
    ) -> Self {
        Self {
            capacity_enforced,
            total_capacity,
            publication_capacity,
            capture_demand,
            analysis_resource_plan,
            analysis_work_plan,
            analysis_compute_capacity,
        }
    }

    /// Whether exact screen capacity admission is installed.
    #[must_use]
    pub const fn capacity_enforced(self) -> bool {
        self.capacity_enforced
    }

    /// Configured steady-state capacity shared by analysis and publication.
    #[must_use]
    pub const fn total_capacity(self) -> ScreenAdmissionCapacity {
        self.total_capacity
    }

    /// Remaining steady-state capacity available to publication.
    #[must_use]
    pub const fn publication_capacity(self) -> ScreenAdmissionCapacity {
        self.publication_capacity
    }

    /// Authoritative screen-capture demand used by the analysis plans.
    #[must_use]
    pub const fn capture_demand(self) -> ScreenCaptureDemand {
        self.capture_demand
    }

    /// Exact retained and peak analysis resources for current demand.
    #[must_use]
    pub const fn analysis_resource_plan(self) -> Option<ScreenAnalysisResourcePlan> {
        self.analysis_resource_plan
    }

    /// Exact compatibility-analysis work for current demand.
    #[must_use]
    pub const fn analysis_work_plan(self) -> Option<ScreenAnalysisWorkPlan> {
        self.analysis_work_plan
    }

    /// Calibrated analysis compute capacity, when configured.
    #[must_use]
    pub const fn analysis_compute_capacity(self) -> Option<ScreenAnalysisComputeCapacity> {
        self.analysis_compute_capacity
    }
}

/// One coherent lock-free screen capacity observation.
#[derive(Clone, Debug)]
pub struct ScreenCapacityStatusSnapshot {
    policy: Arc<ScreenCapacityPolicySnapshot>,
    physical: ScreenByteAdmissionSnapshot,
}

impl ScreenCapacityStatusSnapshot {
    /// Manager-owned capacity policy and analysis plans.
    #[must_use]
    pub fn policy(&self) -> ScreenCapacityPolicySnapshot {
        *self.policy
    }

    /// Live physical capacity and source-lifetime reservations.
    #[must_use]
    pub const fn physical(&self) -> ScreenByteAdmissionSnapshot {
        self.physical
    }
}

/// Cloneable lock-free access to exact screen capacity status.
#[derive(Clone, Debug)]
pub struct ScreenCapacityStatusHandle {
    policy: Arc<ArcSwap<ScreenCapacityPolicySnapshot>>,
    coordinator: ScreenByteAdmissionCoordinator,
    transition_revision: Arc<AtomicU64>,
}

pub(crate) struct ScreenCapacityStatusTransition {
    handle: ScreenCapacityStatusHandle,
    locked_revision: u64,
    finished: bool,
}

impl ScreenCapacityStatusHandle {
    pub(crate) fn new(
        policy: &ScreenCapacityPolicySnapshot,
        coordinator: ScreenByteAdmissionCoordinator,
    ) -> Self {
        Self {
            policy: Arc::new(ArcSwap::from_pointee(*policy)),
            coordinator,
            transition_revision: Arc::new(AtomicU64::new(0)),
        }
    }

    pub(crate) fn publish(&self, policy: &ScreenCapacityPolicySnapshot) {
        self.policy.store(Arc::new(*policy));
    }

    pub(crate) fn begin_transition(&self) -> ScreenCapacityStatusTransition {
        loop {
            let revision = self.transition_revision.load(Ordering::Acquire);
            if revision & 1 != 0 {
                std::hint::spin_loop();
                continue;
            }
            let locked_revision = revision
                .checked_add(1)
                .filter(|locked| locked.checked_add(1).is_some())
                .expect("screen capacity status revision exhausted");
            if self
                .transition_revision
                .compare_exchange_weak(
                    revision,
                    locked_revision,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                )
                .is_ok()
            {
                return ScreenCapacityStatusTransition {
                    handle: self.clone(),
                    locked_revision,
                    finished: false,
                };
            }
        }
    }

    /// Snapshot policy and physical usage from one installed capacity revision.
    #[must_use]
    pub fn snapshot(&self) -> ScreenCapacityStatusSnapshot {
        loop {
            let revision = self.transition_revision.load(Ordering::Acquire);
            if revision & 1 != 0 {
                std::hint::spin_loop();
                continue;
            }
            let policy = self.policy.load_full();
            let physical = self.coordinator.snapshot();
            if self.transition_revision.load(Ordering::Acquire) == revision {
                return ScreenCapacityStatusSnapshot { policy, physical };
            }
            std::hint::spin_loop();
        }
    }
}

impl ScreenCapacityStatusTransition {
    pub(crate) fn publish(mut self, policy: &ScreenCapacityPolicySnapshot) {
        self.handle.policy.store(Arc::new(*policy));
        self.finish();
    }

    fn finish(&mut self) {
        self.handle
            .transition_revision
            .store(self.locked_revision + 1, Ordering::Release);
        self.finished = true;
    }
}

impl Drop for ScreenCapacityStatusTransition {
    fn drop(&mut self) {
        if !self.finished {
            self.finish();
        }
    }
}

/// Lock-free process-wide byte fence shared by analysis and publication.
#[derive(Clone, Debug)]
pub struct ScreenByteAdmissionCoordinator {
    inner: Arc<ScreenByteAdmissionInner>,
}

#[derive(Debug)]
struct ScreenByteAdmissionInner {
    byte_budget: AtomicU64,
    backend_capacity: AtomicU64,
    reserved_bytes: AtomicU64,
    capacity_revision: AtomicU64,
}

impl ScreenByteAdmissionInner {
    fn release(&self, bytes: u64) {
        if bytes == 0 {
            return;
        }
        let released =
            self.reserved_bytes
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |reserved| {
                    reserved.checked_sub(bytes)
                });
        debug_assert!(released.is_ok(), "screen byte admission underflow");
    }
}

/// One source-lifetime resource reservation.
#[derive(Clone, Debug)]
pub struct ScreenByteLease {
    inner: Arc<ScreenByteLeaseInner>,
}

/// Unique mutable quote held while fallible resources are prepared.
#[derive(Debug)]
pub struct ScreenByteReservation {
    lease: Option<ScreenByteLease>,
}

#[derive(Debug)]
struct ScreenByteLeaseInner {
    coordinator: Arc<ScreenByteAdmissionInner>,
    bytes: AtomicU64,
}

impl Drop for ScreenByteLeaseInner {
    fn drop(&mut self) {
        let bytes = self.bytes.swap(0, Ordering::AcqRel);
        self.coordinator.release(bytes);
    }
}

/// Observable state of the shared screen resource fence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScreenByteAdmissionSnapshot {
    capacity: ScreenAdmissionCapacity,
    reserved_bytes: u64,
    capacity_revision: u64,
}

impl ScreenByteAdmissionSnapshot {
    /// Installed process and backend limits.
    #[must_use]
    pub const fn capacity(self) -> ScreenAdmissionCapacity {
        self.capacity
    }

    /// Bytes held by active, staged, published, or retiring resources.
    #[must_use]
    pub const fn reserved_bytes(self) -> u64 {
        self.reserved_bytes
    }

    /// Stable even revision of the installed capacity.
    #[must_use]
    pub const fn capacity_revision(self) -> u64 {
        self.capacity_revision
    }

    /// Remaining bytes admitted by both independent fences.
    #[must_use]
    pub fn available_bytes(self) -> u64 {
        self.capacity
            .byte_budget()
            .min(self.capacity.backend_capacity())
            .saturating_sub(self.reserved_bytes)
    }
}

/// Typed rejection from the shared screen byte fence.
#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
pub enum ScreenByteAdmissionError {
    /// A source quote would exceed one of the installed capacity fences.
    #[error(
        "screen resources need {requested_bytes} bytes; shared capacity has {available_bytes} bytes available"
    )]
    CapacityExceeded {
        requested_bytes: u64,
        available_bytes: u64,
    },
    /// Capacity cannot shrink below resources that can still be observed.
    #[error(
        "screen capacity cannot shrink to {requested_capacity} bytes while {reserved_bytes} bytes remain reserved"
    )]
    CapacityShrinkRejected {
        requested_capacity: u64,
        reserved_bytes: u64,
    },
    /// The monotonic capacity revision cannot advance safely.
    #[error("screen capacity revision exhausted")]
    RevisionExhausted,
}

impl Default for ScreenByteAdmissionCoordinator {
    fn default() -> Self {
        Self::new(ScreenAdmissionCapacity::new(u64::MAX, u64::MAX))
    }
}

impl ScreenByteAdmissionCoordinator {
    /// Construct an empty coordinator with independent process/backend fences.
    #[must_use]
    pub fn new(capacity: ScreenAdmissionCapacity) -> Self {
        Self {
            inner: Arc::new(ScreenByteAdmissionInner {
                byte_budget: AtomicU64::new(capacity.byte_budget()),
                backend_capacity: AtomicU64::new(capacity.backend_capacity()),
                reserved_bytes: AtomicU64::new(0),
                capacity_revision: AtomicU64::new(0),
            }),
        }
    }

    /// Reserve a worst-case quote before any corresponding allocation.
    pub fn try_acquire(
        &self,
        bytes: u64,
    ) -> Result<ScreenByteReservation, ScreenByteAdmissionError> {
        loop {
            let revision = self.inner.capacity_revision.load(Ordering::Acquire);
            if revision & 1 != 0 {
                std::hint::spin_loop();
                continue;
            }
            let capacity = self
                .inner
                .byte_budget
                .load(Ordering::Acquire)
                .min(self.inner.backend_capacity.load(Ordering::Acquire));
            let reserved = self.inner.reserved_bytes.load(Ordering::Acquire);
            let Some(next) = reserved.checked_add(bytes) else {
                return Err(ScreenByteAdmissionError::CapacityExceeded {
                    requested_bytes: bytes,
                    available_bytes: capacity.saturating_sub(reserved),
                });
            };
            if next > capacity {
                return Err(ScreenByteAdmissionError::CapacityExceeded {
                    requested_bytes: bytes,
                    available_bytes: capacity.saturating_sub(reserved),
                });
            }
            if self
                .inner
                .reserved_bytes
                .compare_exchange_weak(reserved, next, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
            {
                continue;
            }
            if self.inner.capacity_revision.load(Ordering::Acquire) == revision {
                return Ok(ScreenByteReservation {
                    lease: Some(ScreenByteLease {
                        inner: Arc::new(ScreenByteLeaseInner {
                            coordinator: Arc::clone(&self.inner),
                            bytes: AtomicU64::new(bytes),
                        }),
                    }),
                });
            }
            self.inner.release(bytes);
        }
    }

    /// Install new fences atomically unless live reservations make them invalid.
    pub fn try_set_capacity(
        &self,
        capacity: ScreenAdmissionCapacity,
    ) -> Result<(), ScreenByteAdmissionError> {
        loop {
            let revision = self.inner.capacity_revision.load(Ordering::Acquire);
            if revision & 1 != 0 {
                std::hint::spin_loop();
                continue;
            }
            let Some(locked_revision) = revision.checked_add(1) else {
                return Err(ScreenByteAdmissionError::RevisionExhausted);
            };
            if self
                .inner
                .capacity_revision
                .compare_exchange_weak(
                    revision,
                    locked_revision,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                )
                .is_err()
            {
                continue;
            }
            let reserved = self.inner.reserved_bytes.load(Ordering::Acquire);
            let requested_capacity = capacity.byte_budget().min(capacity.backend_capacity());
            if reserved > requested_capacity {
                self.inner
                    .capacity_revision
                    .store(revision, Ordering::Release);
                return Err(ScreenByteAdmissionError::CapacityShrinkRejected {
                    requested_capacity,
                    reserved_bytes: reserved,
                });
            }
            let Some(committed_revision) = locked_revision.checked_add(1) else {
                self.inner
                    .capacity_revision
                    .store(revision, Ordering::Release);
                return Err(ScreenByteAdmissionError::RevisionExhausted);
            };
            self.inner
                .byte_budget
                .store(capacity.byte_budget(), Ordering::Release);
            self.inner
                .backend_capacity
                .store(capacity.backend_capacity(), Ordering::Release);
            self.inner
                .capacity_revision
                .store(committed_revision, Ordering::Release);
            return Ok(());
        }
    }

    /// Snapshot one stable capacity revision and its current usage.
    #[must_use]
    pub fn snapshot(&self) -> ScreenByteAdmissionSnapshot {
        loop {
            let revision = self.inner.capacity_revision.load(Ordering::Acquire);
            if revision & 1 != 0 {
                std::hint::spin_loop();
                continue;
            }
            let capacity = ScreenAdmissionCapacity::new(
                self.inner.byte_budget.load(Ordering::Acquire),
                self.inner.backend_capacity.load(Ordering::Acquire),
            );
            let reserved_bytes = self.inner.reserved_bytes.load(Ordering::Acquire);
            if self.inner.capacity_revision.load(Ordering::Acquire) == revision {
                return ScreenByteAdmissionSnapshot {
                    capacity,
                    reserved_bytes,
                    capacity_revision: revision,
                };
            }
        }
    }
}

impl ScreenByteReservation {
    /// Bytes represented by this unique preparation quote.
    #[must_use]
    pub fn bytes(&self) -> u64 {
        self.lease
            .as_ref()
            .map_or(0, |lease| lease.inner.bytes.load(Ordering::Acquire))
    }

    /// Release quote slack after all fallible preparation has succeeded.
    ///
    /// # Errors
    ///
    /// Rejects an attempted increase; additional storage needs a fresh quote.
    pub fn reconcile_down(&mut self, actual_bytes: u64) -> Result<(), ScreenByteAdmissionError> {
        let lease = self
            .lease
            .as_ref()
            .expect("unfrozen reservation retains its lease");
        loop {
            let quoted = lease.inner.bytes.load(Ordering::Acquire);
            if actual_bytes > quoted {
                return Err(ScreenByteAdmissionError::CapacityExceeded {
                    requested_bytes: actual_bytes - quoted,
                    available_bytes: 0,
                });
            }
            if actual_bytes == quoted {
                return Ok(());
            }
            if lease
                .inner
                .bytes
                .compare_exchange_weak(quoted, actual_bytes, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                let released = quoted - actual_bytes;
                lease.inner.coordinator.release(released);
                return Ok(());
            }
        }
    }

    /// Split one independently owned portion without changing total admission.
    pub fn split_off(
        &mut self,
        bytes: u64,
    ) -> Result<ScreenByteReservation, ScreenByteAdmissionError> {
        let lease = self
            .lease
            .as_ref()
            .expect("unfrozen reservation retains its lease");
        loop {
            let current = lease.inner.bytes.load(Ordering::Acquire);
            if bytes > current {
                return Err(ScreenByteAdmissionError::CapacityExceeded {
                    requested_bytes: bytes - current,
                    available_bytes: 0,
                });
            }
            if lease
                .inner
                .bytes
                .compare_exchange_weak(
                    current,
                    current - bytes,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                )
                .is_ok()
            {
                return Ok(ScreenByteReservation {
                    lease: Some(ScreenByteLease {
                        inner: Arc::new(ScreenByteLeaseInner {
                            coordinator: Arc::clone(&lease.inner.coordinator),
                            bytes: AtomicU64::new(bytes),
                        }),
                    }),
                });
            }
        }
    }

    /// Merge another unique quote from the same coordinator.
    pub fn absorb(
        &mut self,
        mut other: ScreenByteReservation,
    ) -> Result<(), ScreenByteAdmissionError> {
        let lease = self
            .lease
            .as_ref()
            .expect("unfrozen reservation retains its lease");
        let other_lease = other
            .lease
            .as_ref()
            .expect("unfrozen reservation retains its lease");
        if !Arc::ptr_eq(&lease.inner.coordinator, &other_lease.inner.coordinator) {
            return Err(ScreenByteAdmissionError::CapacityExceeded {
                requested_bytes: other.bytes(),
                available_bytes: 0,
            });
        }
        let current = lease.inner.bytes.load(Ordering::Acquire);
        let additional = other_lease.inner.bytes.load(Ordering::Acquire);
        let Some(combined) = current.checked_add(additional) else {
            return Err(ScreenByteAdmissionError::CapacityExceeded {
                requested_bytes: additional,
                available_bytes: 0,
            });
        };
        lease.inner.bytes.store(combined, Ordering::Release);
        other_lease.inner.bytes.store(0, Ordering::Release);
        drop(other.lease.take());
        Ok(())
    }

    /// Freeze this quote into cloneable ownership after preparation succeeds.
    #[must_use]
    pub fn freeze(mut self) -> ScreenByteLease {
        self.lease
            .take()
            .expect("reservation can be frozen exactly once")
    }
}

impl ScreenByteLease {
    /// Bytes represented by this immutable shared ownership generation.
    #[must_use]
    pub fn bytes(&self) -> u64 {
        self.inner.bytes.load(Ordering::Acquire)
    }

    /// Atomically rebase a live reservation to an exact backing size.
    ///
    /// An increase is admitted before this lease exposes the larger size. A
    /// rejected increase preserves both the lease and process-wide totals.
    ///
    /// # Errors
    ///
    /// Returns [`ScreenByteAdmissionError::CapacityExceeded`] when an increase
    /// cannot fit inside the installed process and backend fences.
    pub fn try_reconcile_exact(&self, exact_bytes: u64) -> Result<(), ScreenByteAdmissionError> {
        loop {
            let current = self.inner.bytes.load(Ordering::Acquire);
            if exact_bytes == current {
                return Ok(());
            }
            if exact_bytes < current {
                if self
                    .inner
                    .bytes
                    .compare_exchange_weak(
                        current,
                        exact_bytes,
                        Ordering::AcqRel,
                        Ordering::Acquire,
                    )
                    .is_ok()
                {
                    self.inner.coordinator.release(current - exact_bytes);
                    return Ok(());
                }
                continue;
            }

            let additional = exact_bytes - current;
            let top_up = ScreenByteAdmissionCoordinator {
                inner: Arc::clone(&self.inner.coordinator),
            }
            .try_acquire(additional)?;
            if self
                .inner
                .bytes
                .compare_exchange(current, exact_bytes, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                let top_up = top_up.freeze();
                top_up.inner.bytes.store(0, Ordering::Release);
                return Ok(());
            }
            drop(top_up);
        }
    }

    pub(crate) fn is_same(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }
}
