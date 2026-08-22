use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::super::{CaptureSourceId, ScreenPublicationHub};
use super::{
    CaptureExactState, CapturePublication, CapturePublicationFence, CaptureSession,
    CaptureSessionAuthority, CaptureSessionAuthorityExhausted, CaptureSessionDeadline,
    CaptureSessionReadiness, CaptureSessionSet, CaptureSessionTransaction, PreparedCaptureSession,
    ReservedCaptureSessionAuthority,
};

type BackendCompatibilityPublication<B> = CapturePublication<
    <B as CaptureBackend>::CompatibilityFence,
    <B as CaptureBackend>::CompatibilityEpoch,
    <B as CaptureBackend>::CompatibilityValue,
>;

pub(in crate::input::screen) struct CaptureBackendHandles<'a, B: CaptureBackend> {
    compatibility: &'a Arc<Mutex<BackendCompatibilityPublication<B>>>,
    exact: &'a Arc<B::ExactState>,
}

impl<B: CaptureBackend> CaptureBackendHandles<'_, B> {
    pub(in crate::input::screen) fn compatibility_publication_handle(
        &self,
    ) -> Arc<Mutex<BackendCompatibilityPublication<B>>> {
        Arc::clone(&self.compatibility)
    }

    pub(in crate::input::screen) fn exact_state_handle(&self) -> Arc<B::ExactState> {
        Arc::clone(&self.exact)
    }
}

pub(in crate::input::screen) struct ScreenCaptureAdapterAssembly<B: CaptureBackend> {
    compatibility: Arc<Mutex<BackendCompatibilityPublication<B>>>,
    exact: Arc<B::ExactState>,
}

impl<B: CaptureBackend> ScreenCaptureAdapterAssembly<B> {
    pub(in crate::input::screen) fn new(exact: Arc<B::ExactState>) -> Self {
        Self {
            compatibility: Arc::new(Mutex::new(CapturePublication::default())),
            exact,
        }
    }

    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub(in crate::input::screen) fn handles(&self) -> CaptureBackendHandles<'_, B> {
        CaptureBackendHandles {
            compatibility: &self.compatibility,
            exact: &self.exact,
        }
    }

    pub(in crate::input::screen) fn finish(self, backend: B) -> ScreenCaptureAdapter<B> {
        ScreenCaptureAdapter {
            sessions: CaptureSessionSet::default(),
            backend,
            compatibility: self.compatibility,
            exact: self.exact,
        }
    }
}

pub(in crate::input::screen) trait CaptureBackend: Sized {
    type Worker: CaptureSession + Send + 'static;
    type Readiness: CaptureSessionReadiness + Send + 'static;
    type SpawnRequest;
    type ExactState: CaptureExactState;
    type CompatibilityFence: CapturePublicationFence<Self::CompatibilityEpoch>
        + Default
        + Send
        + 'static;
    type CompatibilityEpoch: PartialEq + Send + 'static;
    type CompatibilityValue: Send + 'static;

    const READINESS_TIMEOUT: Duration;

    fn spawn_worker(
        &self,
        request: Self::SpawnRequest,
        handles: CaptureBackendHandles<'_, Self>,
        reservation: ReservedCaptureSessionAuthority,
    ) -> anyhow::Result<CaptureSessionTransaction<Self::Worker, Self::Readiness>>;
}

pub(in crate::input::screen) struct ScreenCaptureAdapter<B: CaptureBackend> {
    sessions: CaptureSessionSet<B::Worker>,
    backend: B,
    compatibility: Arc<Mutex<BackendCompatibilityPublication<B>>>,
    exact: Arc<B::ExactState>,
}

impl<B> Default for ScreenCaptureAdapter<B>
where
    B: CaptureBackend,
    B: Default,
    B::ExactState: Default,
{
    fn default() -> Self {
        ScreenCaptureAdapterAssembly::new(Arc::new(B::ExactState::default())).finish(B::default())
    }
}

impl<B: CaptureBackend> ScreenCaptureAdapter<B> {
    fn handles(&self) -> CaptureBackendHandles<'_, B> {
        CaptureBackendHandles {
            compatibility: &self.compatibility,
            exact: &self.exact,
        }
    }

    pub(in crate::input::screen) fn compatibility_publication(
        &self,
    ) -> &Mutex<BackendCompatibilityPublication<B>> {
        &self.compatibility
    }

    pub(in crate::input::screen) fn compatibility_publication_handle(
        &self,
    ) -> Arc<Mutex<BackendCompatibilityPublication<B>>> {
        Arc::clone(&self.compatibility)
    }

    pub(in crate::input::screen) fn exact_state(&self) -> &B::ExactState {
        &self.exact
    }

    pub(in crate::input::screen) fn exact_state_handle(&self) -> Arc<B::ExactState> {
        Arc::clone(&self.exact)
    }

    pub(in crate::input::screen) fn reserve_exact_authority(
        &self,
    ) -> Result<ReservedCaptureSessionAuthority, CaptureSessionAuthorityExhausted> {
        self.exact.common().reserve_authority()
    }

    pub(in crate::input::screen) fn install_publication_hub(&self, hub: Arc<ScreenPublicationHub>) {
        self.exact.common().install_hub(hub);
    }

    pub(in crate::input::screen) fn exact_source(
        &self,
    ) -> Option<<B::ExactState as CaptureExactState>::Source> {
        self.exact.common().source()
    }

    pub(in crate::input::screen) fn owns_exact_source(&self, source_id: &CaptureSourceId) -> bool {
        self.exact.common().owns_source(source_id)
    }

    pub(in crate::input::screen) fn exact_resolution_revision(&self) -> u64 {
        self.exact.common().resolution_revision()
    }

    pub(in crate::input::screen) fn advance_exact_resolution_revision(&self) {
        self.exact.common().advance_resolution_revision();
    }

    pub(in crate::input::screen) fn prepare_worker(
        &self,
        request: B::SpawnRequest,
        reservation: ReservedCaptureSessionAuthority,
    ) -> anyhow::Result<PreparedCaptureSession<B::Worker>> {
        self.backend
            .spawn_worker(request, self.handles(), reservation)?
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
