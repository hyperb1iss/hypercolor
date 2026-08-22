use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};

use super::super::ScreenCaptureDemand;

pub(in crate::input::screen) struct VersionedCaptureSettings<C> {
    config: Mutex<C>,
    demand: Mutex<ScreenCaptureDemand>,
    revision: AtomicU64,
}

pub(in crate::input::screen) struct CaptureSettingsSnapshot<C> {
    pub(in crate::input::screen) config: C,
    pub(in crate::input::screen) demand: ScreenCaptureDemand,
}

pub(in crate::input::screen) struct CaptureSettingsGuard<'a, C> {
    config: MutexGuard<'a, C>,
    demand: MutexGuard<'a, ScreenCaptureDemand>,
    revision: &'a AtomicU64,
}

impl<C> VersionedCaptureSettings<C> {
    pub(in crate::input::screen) fn new(config: C, demand: ScreenCaptureDemand) -> Self {
        Self {
            config: Mutex::new(config),
            demand: Mutex::new(demand),
            revision: AtomicU64::new(0),
        }
    }

    pub(in crate::input::screen) fn snapshot(&self) -> CaptureSettingsSnapshot<C>
    where
        C: Clone,
    {
        let values = self.lock();
        CaptureSettingsSnapshot {
            config: values.config.clone(),
            demand: *values.demand,
        }
    }

    pub(in crate::input::screen) fn lock(&self) -> CaptureSettingsGuard<'_, C> {
        CaptureSettingsGuard {
            config: self
                .config
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            demand: self
                .demand
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            revision: &self.revision,
        }
    }

    pub(in crate::input::screen) fn lock_config(&self) -> MutexGuard<'_, C> {
        self.config
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub(in crate::input::screen) fn lock_demand(&self) -> MutexGuard<'_, ScreenCaptureDemand> {
        self.demand
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub(in crate::input::screen) fn try_lock_demand(
        &self,
    ) -> std::sync::LockResult<MutexGuard<'_, ScreenCaptureDemand>> {
        self.demand.lock()
    }

    pub(in crate::input::screen) fn revision(&self) -> u64 {
        self.revision.load(Ordering::Acquire)
    }

    pub(in crate::input::screen) fn bump_revision(&self) -> u64 {
        self.revision
            .fetch_add(1, Ordering::Release)
            .wrapping_add(1)
    }

    pub(in crate::input::screen) fn commit_revision(&self) -> u64 {
        self.revision.fetch_add(1, Ordering::AcqRel).wrapping_add(1)
    }
}

impl<C> CaptureSettingsGuard<'_, C> {
    pub(in crate::input::screen) fn config(&self) -> &C {
        &self.config
    }

    pub(in crate::input::screen) fn config_mut(&mut self) -> &mut C {
        &mut self.config
    }

    pub(in crate::input::screen) fn demand_mut(&mut self) -> &mut ScreenCaptureDemand {
        &mut self.demand
    }

    pub(in crate::input::screen) fn commit(self) -> u64 {
        self.revision.fetch_add(1, Ordering::AcqRel).wrapping_add(1)
    }
}
