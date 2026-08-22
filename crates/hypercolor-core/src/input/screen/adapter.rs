#[cfg(test)]
mod tests;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use super::{
    CaptureSourceId, ExactBoxList, ExactBoxNode, ScreenCommittedState, ScreenPublicationHub,
};

pub trait CapturePublicationSource: Clone + PartialEq {
    fn source_id(&self) -> &CaptureSourceId;
}

pub trait CaptureOwnedSource {
    fn source_id(&self) -> &CaptureSourceId;

    fn belongs_to_authority(&self, authority: &ScreenCommittedState) -> bool;
}

pub struct CaptureExactPublicationShared<S, O> {
    source: Mutex<Option<S>>,
    owned_sources: Mutex<ExactBoxList<O>>,
    hub: Mutex<Option<Arc<ScreenPublicationHub>>>,
    resolution_revision: AtomicU64,
}

impl<S, O> Default for CaptureExactPublicationShared<S, O> {
    fn default() -> Self {
        Self {
            source: Mutex::new(None),
            owned_sources: Mutex::new(ExactBoxList::default()),
            hub: Mutex::new(None),
            resolution_revision: AtomicU64::new(0),
        }
    }
}

impl<S, O> CaptureExactPublicationShared<S, O>
where
    S: CapturePublicationSource,
    O: CaptureOwnedSource,
{
    pub fn replace_source(&self, next: Option<S>) -> bool {
        let mut source = self
            .source
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if *source == next {
            return false;
        }
        *source = next;
        self.resolution_revision
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |revision| {
                revision.checked_add(1)
            })
            .expect("screen publication resolution revision exhausted");
        true
    }

    pub fn source(&self) -> Option<S> {
        self.source
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    pub fn install_hub(&self, hub: Arc<ScreenPublicationHub>) {
        *self
            .hub
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(hub);
    }

    pub fn hub(&self) -> Option<Arc<ScreenPublicationHub>> {
        self.hub
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    pub fn resolution_revision(&self) -> u64 {
        self.resolution_revision.load(Ordering::Acquire)
    }

    pub fn advance_resolution_revision(&self) {
        self.resolution_revision
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |revision| {
                revision.checked_add(1)
            })
            .expect("screen publication resolution revision exhausted");
    }

    pub fn owns_source(&self, source_id: &CaptureSourceId) -> bool {
        self.source()
            .is_some_and(|source| source.source_id() == source_id)
            || self
                .owned_sources
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .iter()
                .any(|owned| owned.source_id() == source_id)
    }

    pub fn register_owned_source(&self, source: Box<ExactBoxNode<O>>) {
        self.owned_sources
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push_boxed(source);
    }

    pub fn reap_owned_sources(&self) {
        let authority = self.hub().map(|hub| hub.committed_state());
        self.owned_sources
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .retain(|source| {
                authority
                    .as_ref()
                    .is_some_and(|authority| source.belongs_to_authority(authority))
            });
    }

    #[cfg(any(target_os = "linux", test))]
    pub fn retain_owned_sources(&self, retain: impl FnMut(&mut O) -> bool) {
        self.owned_sources
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .retain(retain);
    }

    pub fn clear_owned_sources(&self) {
        self.owned_sources
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
    }

    #[cfg(test)]
    pub fn owned_source_count(&self) -> usize {
        self.owned_sources
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .count()
    }
}

#[cfg(any(target_os = "windows", test))]
pub trait CapturePublicationEpoch: PartialEq {
    fn source_generation(&self) -> u64;

    fn activity_generation(&self) -> u64;
}

#[cfg(any(target_os = "windows", test))]
pub struct CapturePublication<E, T> {
    source_generation: u64,
    activity_generation: u64,
    pub(super) active: Option<E>,
    pub(super) latest: Option<T>,
}

#[cfg(any(target_os = "windows", test))]
impl<E, T> Default for CapturePublication<E, T> {
    fn default() -> Self {
        Self {
            source_generation: 0,
            activity_generation: 0,
            active: None,
            latest: None,
        }
    }
}

#[cfg(any(target_os = "windows", test))]
impl<E, T> CapturePublication<E, T>
where
    E: CapturePublicationEpoch,
{
    pub fn activate(&mut self, active: E) -> bool {
        if active.source_generation() != self.source_generation
            || active.activity_generation() != self.activity_generation
        {
            return false;
        }
        if self.active.as_ref() != Some(&active) {
            self.latest = None;
            self.active = Some(active);
        }
        true
    }

    pub fn fence_source(&mut self, source_generation: u64) {
        self.source_generation = source_generation;
        self.clear();
    }

    pub fn fence_activity(&mut self, activity_generation: u64) {
        self.activity_generation = activity_generation;
        self.clear();
    }

    pub fn clear(&mut self) {
        self.active = None;
        self.latest = None;
    }

    pub fn publish(&mut self, active: &E, value: T) -> bool {
        if self.active.as_ref() != Some(active) {
            return false;
        }
        self.latest = Some(value);
        true
    }
}
