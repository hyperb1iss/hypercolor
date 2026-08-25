use std::num::{NonZeroU32, NonZeroUsize};
use std::ops::Deref;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

#[cfg(test)]
use super::super::ExactBoxNode;
use super::super::{
    CaptureSourceId, CpuReductionExecutor, ExactBoxList, ScreenCommittedState,
    ScreenPreparedWorkerToken, ScreenPublicationHub, ScreenResourceLifetime, ScreenWorkerBinding,
    ScreenWorkerExactLedgerBuilder, ScreenWorkerLedgerBuildError, ScreenWorkerPreparationTicket,
};
use super::{
    CaptureSessionAuthority, CaptureSessionAuthorityExhausted, CaptureSessionAuthoritySequencer,
    ReservedCaptureSessionAuthority, StaleCaptureSessionReservation,
};

pub(in crate::input::screen) fn preflight_capture_exact_scope_bytes(
    ledger: &mut ScreenWorkerExactLedgerBuilder,
    minimum_remaining: &mut u64,
    retained_bytes: u64,
) -> Result<(), ScreenWorkerLedgerBuildError> {
    let modeled = retained_bytes.min(*minimum_remaining);
    *minimum_remaining -= modeled;
    let additional = retained_bytes - modeled;
    if additional > 0 {
        ledger.preflight_additional_bytes(additional)?;
    }
    Ok(())
}

pub(in crate::input::screen) fn finish_removed_capture_exact_source(
    ticket: ScreenWorkerPreparationTicket,
) -> anyhow::Result<ScreenPreparedWorkerToken> {
    let source_id = ticket.source_id();
    let candidate = ticket.candidate_plan();
    let delta = ticket.source_delta();
    let candidate_has_work = candidate
        .branches()
        .iter()
        .any(|branch| branch.descriptor().source_epoch().source_id == *source_id)
        || candidate
            .physical_reductions()
            .iter()
            .any(|reduction| reduction.descriptor().source_epoch().source_id == *source_id);
    let delta_has_candidate_work = !delta.added_branches().is_empty()
        || !delta.retained_branches().is_empty()
        || !delta.added_physical_reductions().is_empty()
        || !delta.retained_physical_reductions().is_empty();
    anyhow::ensure!(
        !candidate_has_work
            && !delta_has_candidate_work
            && !delta.removed_branches().is_empty()
            && ticket.required_minimums().is_empty(),
        "exact source {source_id} is not a removal-only preparation"
    );
    let exact = ScreenWorkerExactLedgerBuilder::new(ticket)?.finish()?;
    let (token, lifetimes) = exact.into_parts();
    debug_assert!(lifetimes.is_empty());
    Ok(token)
}

pub(in crate::input::screen) trait CapturePublicationSource:
    Clone + PartialEq
{
    fn source_id(&self) -> &CaptureSourceId;
}

pub(in crate::input::screen) trait CaptureOwnedSource {
    fn source_id(&self) -> &CaptureSourceId;

    fn belongs_to_authority(&self, authority: &ScreenCommittedState) -> bool;
}

/// One exact source a worker owns on behalf of a committed runtime binding.
///
/// The record outlives nothing: it is reaped as soon as the committed plan
/// no longer owns its binding, releasing the runtime lifetime with it.
pub(in crate::input::screen) struct CaptureOwnedSourceRecord {
    source_id: CaptureSourceId,
    binding: ScreenWorkerBinding,
    _runtime_lifetime: ScreenResourceLifetime,
}

impl CaptureOwnedSourceRecord {
    pub(in crate::input::screen) const fn new(
        source_id: CaptureSourceId,
        binding: ScreenWorkerBinding,
        runtime_lifetime: ScreenResourceLifetime,
    ) -> Self {
        Self {
            source_id,
            binding,
            _runtime_lifetime: runtime_lifetime,
        }
    }
}

impl CaptureOwnedSource for CaptureOwnedSourceRecord {
    fn source_id(&self) -> &CaptureSourceId {
        &self.source_id
    }

    fn belongs_to_authority(&self, authority: &ScreenCommittedState) -> bool {
        authority.owns_runtime_binding(&self.binding)
    }
}

/// Lazily constructed CPU reduction executor shared by a backend's workers.
///
/// Construction waits for the first exact CPU runtime so backends that never
/// fall back to CPU reduction never spin up the thread pool.
#[derive(Default)]
pub(in crate::input::screen) struct CpuExecutorSlot {
    executor: Mutex<Option<Arc<CpuReductionExecutor>>>,
}

impl CpuExecutorSlot {
    const TILE_HEIGHT: NonZeroU32 = NonZeroU32::new(16).expect("CPU reduction tile height");

    fn lock(&self) -> std::sync::MutexGuard<'_, Option<Arc<CpuReductionExecutor>>> {
        self.executor
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Returns the executor, constructing it on first use.
    pub(in crate::input::screen) fn executor(&self) -> anyhow::Result<Arc<CpuReductionExecutor>> {
        let mut executor = self.lock();
        if let Some(executor) = executor.as_ref() {
            return Ok(Arc::clone(executor));
        }
        let prepared = Arc::new(CpuReductionExecutor::new(
            thread::available_parallelism().unwrap_or(NonZeroUsize::MIN),
            Self::TILE_HEIGHT,
        )?);
        *executor = Some(Arc::clone(&prepared));
        Ok(prepared)
    }

    /// Worker count the executor has, or would have once constructed.
    pub(in crate::input::screen) fn worker_count(&self) -> NonZeroUsize {
        self.lock().as_ref().map_or_else(
            || thread::available_parallelism().unwrap_or(NonZeroUsize::MIN),
            |executor| executor.worker_count(),
        )
    }
}

pub(in crate::input::screen) trait CaptureExactState:
    Send + Sync + 'static
{
    type Source: CapturePublicationSource + Send + 'static;
    type OwnedSource: CaptureOwnedSource + Send + 'static;

    fn common(&self) -> &CaptureExactPublicationShared<Self::Source, Self::OwnedSource>;
}

pub(in crate::input::screen) struct CaptureAuthorityDisplacement<S, O> {
    _source: Option<S>,
    _owned_sources: ExactBoxList<O>,
}

pub(in crate::input::screen) struct CaptureAuthorityRetirement<S, O> {
    replacement: CaptureSessionAuthority,
    displaced: CaptureAuthorityDisplacement<S, O>,
}

impl<S, O> CaptureAuthorityRetirement<S, O> {
    pub(in crate::input::screen) const fn replacement(&self) -> CaptureSessionAuthority {
        self.replacement
    }

    pub(in crate::input::screen) fn into_displaced(self) -> CaptureAuthorityDisplacement<S, O> {
        self.displaced
    }
}

pub(in crate::input::screen) struct CaptureExactPublicationShared<S, O> {
    authority: CaptureSessionAuthoritySequencer,
    source: Mutex<Option<S>>,
    owned_sources: Mutex<ExactBoxList<O>>,
    hub: Mutex<Option<Arc<ScreenPublicationHub>>>,
    resolution_revision: AtomicU64,
}

impl<S, O> Default for CaptureExactPublicationShared<S, O> {
    fn default() -> Self {
        Self {
            authority: CaptureSessionAuthoritySequencer::default(),
            source: Mutex::new(None),
            owned_sources: Mutex::new(ExactBoxList::default()),
            hub: Mutex::new(None),
            resolution_revision: AtomicU64::new(0),
        }
    }
}

impl<S, O> CaptureExactState for CaptureExactPublicationShared<S, O>
where
    S: CapturePublicationSource + Send + 'static,
    O: CaptureOwnedSource + Send + 'static,
{
    type Source = S;
    type OwnedSource = O;

    fn common(&self) -> &Self {
        self
    }
}

/// Backend exact state that wraps the shared core with extra native fields
/// is reachable through `Deref`, so the core impl covers it.
impl<T, S, O> CaptureExactState for T
where
    T: Deref<Target = CaptureExactPublicationShared<S, O>> + Send + Sync + 'static,
    S: CapturePublicationSource + Send + 'static,
    O: CaptureOwnedSource + Send + 'static,
{
    type Source = S;
    type OwnedSource = O;

    fn common(&self) -> &CaptureExactPublicationShared<S, O> {
        self
    }
}

impl<S, O> CaptureExactPublicationShared<S, O>
where
    S: CapturePublicationSource,
    O: CaptureOwnedSource,
{
    pub(in crate::input::screen) fn reserve_authority(
        &self,
    ) -> Result<ReservedCaptureSessionAuthority, CaptureSessionAuthorityExhausted> {
        self.authority.reserve()
    }

    pub(in crate::input::screen) fn can_activate_reserved_authority(
        &self,
        reservation: &ReservedCaptureSessionAuthority,
    ) -> bool {
        self.authority.can_commit(reservation)
    }

    pub(in crate::input::screen) fn activate_reserved_authority(
        &self,
        reservation: ReservedCaptureSessionAuthority,
    ) -> Result<CaptureAuthorityDisplacement<S, O>, StaleCaptureSessionReservation> {
        let mut source = self
            .source
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut owned_sources = self
            .owned_sources
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.authority.commit(reservation)?;
        let displaced_source = source.take();
        let displaced_owned_sources = std::mem::take(&mut *owned_sources);
        if displaced_source.is_some() {
            self.advance_resolution_revision();
        }
        drop(owned_sources);
        drop(source);
        Ok(CaptureAuthorityDisplacement {
            _source: displaced_source,
            _owned_sources: displaced_owned_sources,
        })
    }

    pub(in crate::input::screen) fn retire_authority_if_current(
        &self,
        expected: CaptureSessionAuthority,
    ) -> Result<Option<CaptureAuthorityRetirement<S, O>>, CaptureSessionAuthorityExhausted> {
        let mut source = self
            .source
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut owned_sources = self
            .owned_sources
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !self.authority.is_current(expected) {
            return Ok(None);
        }
        let reservation = self.authority.reserve()?;
        let replacement = self
            .authority
            .commit(reservation)
            .expect("reserved retirement authority remains newer than current");
        let displaced_source = source.take();
        let displaced_owned_sources = std::mem::take(&mut *owned_sources);
        if displaced_source.is_some() {
            self.advance_resolution_revision();
        }
        drop(owned_sources);
        drop(source);
        Ok(Some(CaptureAuthorityRetirement {
            replacement,
            displaced: CaptureAuthorityDisplacement {
                _source: displaced_source,
                _owned_sources: displaced_owned_sources,
            },
        }))
    }

    pub(in crate::input::screen) fn is_current_authority(
        &self,
        authority: CaptureSessionAuthority,
    ) -> bool {
        self.authority.is_current(authority)
    }

    pub(in crate::input::screen) fn current_authority(&self) -> Option<CaptureSessionAuthority> {
        self.authority.current()
    }

    /// The current authority's generation, or zero before any session commits.
    #[cfg(test)]
    pub(in crate::input::screen) fn current_generation(&self) -> u64 {
        self.current_authority()
            .map_or(0, CaptureSessionAuthority::generation)
    }

    pub(in crate::input::screen) fn replace_source_if_current(
        &self,
        authority: CaptureSessionAuthority,
        next: Option<S>,
    ) -> bool {
        let mut source = self
            .source
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !self.is_current_authority(authority) || *source == next {
            drop(source);
            drop(next);
            return false;
        }
        let displaced = std::mem::replace(&mut *source, next);
        self.advance_resolution_revision();
        drop(source);
        drop(displaced);
        true
    }

    pub(in crate::input::screen) fn with_current_source<T>(
        &self,
        authority: CaptureSessionAuthority,
        operation: impl FnOnce(Option<S>) -> T,
    ) -> Option<T> {
        let source = self
            .source
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !self.is_current_authority(authority) {
            drop(source);
            drop(operation);
            return None;
        }
        let current = source.clone();
        drop(source);
        Some(operation(current))
    }

    pub(in crate::input::screen) fn source(&self) -> Option<S> {
        self.source
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    pub(in crate::input::screen) fn install_hub(&self, hub: Arc<ScreenPublicationHub>) {
        *self
            .hub
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(hub);
    }

    pub(in crate::input::screen) fn hub(&self) -> Option<Arc<ScreenPublicationHub>> {
        self.hub
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    pub(in crate::input::screen) fn resolution_revision(&self) -> u64 {
        self.resolution_revision.load(Ordering::Acquire)
    }

    pub(in crate::input::screen) fn advance_resolution_revision(&self) {
        self.resolution_revision
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |revision| {
                revision.checked_add(1)
            })
            .expect("screen publication resolution revision exhausted");
    }

    pub(in crate::input::screen) fn owns_source(&self, source_id: &CaptureSourceId) -> bool {
        let source = self
            .source
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let owned_sources = self
            .owned_sources
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        source
            .as_ref()
            .is_some_and(|source| source.source_id() == source_id)
            || owned_sources
                .iter()
                .any(|owned| owned.source_id() == source_id)
    }

    pub(in crate::input::screen) fn with_current_owned_sources<T>(
        &self,
        authority: CaptureSessionAuthority,
        operation: impl FnOnce(&mut ExactBoxList<O>) -> T,
    ) -> Option<T> {
        let mut owned_sources = self
            .owned_sources
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !self.is_current_authority(authority) {
            drop(owned_sources);
            drop(operation);
            return None;
        }
        let result = operation(&mut owned_sources);
        drop(owned_sources);
        Some(result)
    }

    #[cfg(test)]
    pub(in crate::input::screen) fn register_owned_source_if_current(
        &self,
        authority: CaptureSessionAuthority,
        source: Box<ExactBoxNode<O>>,
    ) -> bool {
        self.with_current_owned_sources(authority, |owned_sources| {
            owned_sources.push_boxed(source);
        })
        .is_some()
    }

    pub(in crate::input::screen) fn reap_owned_sources_if_current(
        &self,
        authority: CaptureSessionAuthority,
    ) -> bool {
        let committed = self.hub().map(|hub| hub.committed_state());
        let mut removed = None;
        let current = self
            .with_current_owned_sources(authority, |owned_sources| {
                removed = Some(owned_sources.extract_if(|source| {
                    !committed
                        .as_ref()
                        .is_some_and(|committed| source.belongs_to_authority(committed))
                }));
            })
            .is_some();
        drop(removed);
        current
    }

    #[cfg(test)]
    pub(in crate::input::screen) fn retain_owned_sources_if_current(
        &self,
        authority: CaptureSessionAuthority,
        mut retain: impl FnMut(&mut O) -> bool,
    ) -> bool {
        let mut removed = None;
        let current = self
            .with_current_owned_sources(authority, |owned_sources| {
                removed = Some(owned_sources.extract_if(|source| !retain(source)));
            })
            .is_some();
        drop(removed);
        current
    }

    pub(in crate::input::screen) fn clear_owned_sources_if_current(
        &self,
        authority: CaptureSessionAuthority,
    ) -> bool {
        let mut displaced = None;
        let current = self
            .with_current_owned_sources(authority, |owned_sources| {
                displaced = Some(std::mem::take(owned_sources));
            })
            .is_some();
        drop(displaced);
        current
    }

    #[cfg(test)]
    pub(in crate::input::screen) fn owned_source_count(&self) -> usize {
        self.owned_sources
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .count()
    }
}
