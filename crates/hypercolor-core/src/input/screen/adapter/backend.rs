use std::time::Duration;

use super::{
    CaptureSession, CaptureSessionAuthority, CaptureSessionDeadline, CaptureSessionReadiness,
    CaptureSessionSet, CaptureSessionTransaction, PreparedCaptureSession,
    ReservedCaptureSessionAuthority,
};

pub(in crate::input::screen) trait CaptureBackend: Sized {
    type Worker: CaptureSession + Send + 'static;
    type Readiness: CaptureSessionReadiness + Send + 'static;
    type SpawnRequest;

    const READINESS_TIMEOUT: Duration;

    fn spawn_worker(
        request: Self::SpawnRequest,
        reservation: ReservedCaptureSessionAuthority,
    ) -> anyhow::Result<CaptureSessionTransaction<Self::Worker, Self::Readiness>>;
}

pub(in crate::input::screen) struct ScreenCaptureAdapter<B: CaptureBackend> {
    sessions: CaptureSessionSet<B::Worker>,
}

impl<B: CaptureBackend> Default for ScreenCaptureAdapter<B> {
    fn default() -> Self {
        Self {
            sessions: CaptureSessionSet::default(),
        }
    }
}

impl<B: CaptureBackend> ScreenCaptureAdapter<B> {
    pub(in crate::input::screen) fn prepare_worker(
        &self,
        request: B::SpawnRequest,
        reservation: ReservedCaptureSessionAuthority,
    ) -> anyhow::Result<PreparedCaptureSession<B::Worker>> {
        B::spawn_worker(request, reservation)?
            .prepare(CaptureSessionDeadline::after(B::READINESS_TIMEOUT))
    }

    pub(in crate::input::screen) fn commit_worker<P, D>(
        &mut self,
        prepared: PreparedCaptureSession<B::Worker>,
        checkpoint: impl FnOnce(&ReservedCaptureSessionAuthority) -> Option<P>,
        commit_authority: impl FnOnce(ReservedCaptureSessionAuthority, P) -> D,
    ) -> Result<CaptureSessionAuthority, PreparedCaptureSession<B::Worker>> {
        prepared
            .commit_into(&mut self.sessions, checkpoint, commit_authority)
            .map(|commit| commit.authority())
    }

    #[cfg_attr(
        not(any(
            target_os = "linux",
            target_os = "windows",
            feature = "macos-capture-fixtures",
            test
        )),
        allow(dead_code)
    )]
    pub(in crate::input::screen) const fn active_worker(&self) -> Option<&B::Worker> {
        self.sessions.active()
    }

    pub(in crate::input::screen) fn active_exact_endpoint(
        &self,
    ) -> Option<<B::Worker as CaptureSession>::ExactEndpoint> {
        self.sessions.exact_endpoint()
    }

    #[cfg_attr(
        not(any(target_os = "linux", target_os = "windows", test)),
        allow(dead_code)
    )]
    pub(in crate::input::screen) fn can_prepare_successor(&self) -> bool {
        self.sessions.can_prepare_successor()
    }

    #[cfg_attr(
        not(any(target_os = "linux", target_os = "windows", test)),
        allow(dead_code)
    )]
    pub(in crate::input::screen) fn can_install_successor(&self) -> bool {
        self.sessions.can_install_successor()
    }

    pub(in crate::input::screen) fn retire_active_worker(
        &mut self,
    ) -> Option<CaptureSessionAuthority> {
        self.sessions.retire_active()
    }

    pub(in crate::input::screen) fn take_finished_active_worker(
        &mut self,
    ) -> Option<(CaptureSessionAuthority, <B::Worker as CaptureSession>::Exit)> {
        self.sessions.take_finished_active()
    }

    pub(in crate::input::screen) fn reap_finished_workers(
        &mut self,
        on_exit: impl FnMut(CaptureSessionAuthority, <B::Worker as CaptureSession>::Exit),
    ) {
        self.sessions.reap_finished(on_exit);
    }

    #[cfg(target_os = "windows")]
    pub(in crate::input::screen) fn take_active_worker_for_settlement(
        &mut self,
    ) -> Option<B::Worker> {
        self.sessions.take_active_for_settlement()
    }

    #[cfg(test)]
    pub(in crate::input::screen) fn install_worker_for_test(
        &mut self,
        worker: B::Worker,
    ) -> Result<(), B::Worker> {
        self.sessions.install(worker)
    }

    #[cfg(test)]
    pub(in crate::input::screen) fn retiring_worker_count(&self) -> usize {
        self.sessions.retiring_len()
    }

    #[cfg(test)]
    pub(in crate::input::screen) fn retiring_worker_capacity(&self) -> usize {
        self.sessions.retiring_capacity()
    }
}
