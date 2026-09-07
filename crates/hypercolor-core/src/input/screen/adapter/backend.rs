use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::super::{
    CaptureSourceId, RegisteredScreenBranchDemand, ResolvedScreenBranchDemand,
    ScreenPublicationHub, ScreenWorkerPreparation, ScreenWorkerPreparationTicket,
    ScreenWorkerRetirement,
};
use super::{
    CaptureActivity, CaptureExactState, CapturePublicationFence, CaptureSession,
    CaptureSessionAuthority, CaptureSessionAuthorityExhausted, CaptureSessionDeadline,
    CaptureSessionExit, CaptureSessionReadiness, CaptureSessionSet, CaptureSessionTransaction,
    PreparedCaptureSession, ReservedCaptureSessionAuthority, VersionedCaptureSettings,
    begin_capture_exact_preparation, begin_capture_exact_retirement,
};

type BackendActivity<B> =
    CaptureActivity<<B as CaptureBackend>::ActivityFence, <B as CaptureBackend>::ActivityEpoch>;

/// Shared fenced capture-epoch record handed to every worker the adapter
/// spawns.
pub(in crate::input::screen) struct CaptureActivityHandle<B: CaptureBackend> {
    activity: Arc<Mutex<BackendActivity<B>>>,
}

impl<B: CaptureBackend> Clone for CaptureActivityHandle<B> {
    fn clone(&self) -> Self {
        Self {
            activity: Arc::clone(&self.activity),
        }
    }
}

impl<B: CaptureBackend> CaptureActivityHandle<B> {
    pub(in crate::input::screen) fn new(activity: Arc<Mutex<BackendActivity<B>>>) -> Self {
        Self { activity }
    }

    #[cfg(feature = "macos-capture-fixtures")]
    pub(in crate::input::screen) fn is_current_epoch(&self, epoch: &B::ActivityEpoch) -> bool {
        self.activity
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_active(epoch)
    }

    fn activity(&self) -> &Mutex<BackendActivity<B>> {
        &self.activity
    }

    fn activity_handle(&self) -> Arc<Mutex<BackendActivity<B>>> {
        Arc::clone(&self.activity)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::input::screen) enum CaptureRetirementCause {
    ActiveStop,
    ObservedExit,
    /// The active worker was taken for exclusive replacement: the successor
    /// owns the publication slot, so only exact state is retired. Backends
    /// without an exclusive replacement path treat it like `ActiveStop`.
    ExclusiveSettlement,
}

pub(in crate::input::screen) struct CaptureBackendHandles<'a, B: CaptureBackend> {
    settings: &'a Arc<VersionedCaptureSettings<B::SettingsConfig>>,
    activity: &'a CaptureActivityHandle<B>,
    exact: &'a Arc<B::ExactState>,
}

impl<B: CaptureBackend> CaptureBackendHandles<'_, B> {
    pub(in crate::input::screen) fn settings_handle(
        &self,
    ) -> Arc<VersionedCaptureSettings<B::SettingsConfig>> {
        Arc::clone(self.settings)
    }

    pub(in crate::input::screen) fn activity(&self) -> &Mutex<BackendActivity<B>> {
        self.activity.activity()
    }

    pub(in crate::input::screen) fn activity_handle(&self) -> Arc<Mutex<BackendActivity<B>>> {
        self.activity.activity_handle()
    }

    pub(in crate::input::screen) fn activity_handle_ref(&self) -> CaptureActivityHandle<B> {
        self.activity.clone()
    }

    pub(in crate::input::screen) fn exact_state(&self) -> &B::ExactState {
        self.exact
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(in crate::input::screen) fn exact_state_handle(&self) -> Arc<B::ExactState> {
        Arc::clone(&self.exact)
    }
}

pub(in crate::input::screen) struct ScreenCaptureAdapterAssembly<B: CaptureBackend> {
    settings: Arc<VersionedCaptureSettings<B::SettingsConfig>>,
    activity: CaptureActivityHandle<B>,
    exact: Arc<B::ExactState>,
}

impl<B: CaptureBackend> ScreenCaptureAdapterAssembly<B> {
    pub(in crate::input::screen) fn new(
        exact: Arc<B::ExactState>,
        config: B::SettingsConfig,
    ) -> Self {
        Self {
            settings: Arc::new(VersionedCaptureSettings::new(
                config,
                super::super::ScreenCaptureDemand::Inactive,
            )),
            activity: CaptureActivityHandle::new(Arc::new(Mutex::new(CaptureActivity::default()))),
            exact,
        }
    }

    pub(in crate::input::screen) fn handles(&self) -> CaptureBackendHandles<'_, B> {
        CaptureBackendHandles {
            settings: &self.settings,
            activity: &self.activity,
            exact: &self.exact,
        }
    }

    pub(in crate::input::screen) fn finish(self, backend: B) -> ScreenCaptureAdapter<B> {
        ScreenCaptureAdapter {
            sessions: CaptureSessionSet::default(),
            backend,
            settings: self.settings,
            activity: self.activity,
            exact: self.exact,
        }
    }
}

pub(in crate::input::screen) trait CaptureBackend: Sized {
    type Worker: CaptureSession + Send + 'static;
    type Readiness: CaptureSessionReadiness + Send + 'static;
    type SpawnRequest;
    type SettingsConfig: Clone + Send + 'static;
    type ExactState: CaptureExactState;
    type ActivityFence: CapturePublicationFence<Self::ActivityEpoch> + Default + Send + 'static;
    type ActivityEpoch: PartialEq + Send + 'static;
    type AuthorityCommitCheckpoint<'a>
    where
        Self: 'a;

    /// Human-facing backend name used in adapter diagnostics.
    const NAME: &'static str;
    const READINESS_TIMEOUT: Duration;

    fn resolve_publication_branch(
        &self,
        settings: &VersionedCaptureSettings<Self::SettingsConfig>,
        source: &<Self::ExactState as CaptureExactState>::Source,
        demand: &RegisteredScreenBranchDemand,
    ) -> anyhow::Result<Option<ResolvedScreenBranchDemand>>;

    fn spawn_worker(
        &self,
        request: Self::SpawnRequest,
        handles: CaptureBackendHandles<'_, Self>,
        reservation: ReservedCaptureSessionAuthority,
    ) -> anyhow::Result<CaptureSessionTransaction<Self::Worker, Self::Readiness>>;

    fn prepare_authority_commit<'a>(
        &'a self,
        handles: CaptureBackendHandles<'a, Self>,
        reservation: &ReservedCaptureSessionAuthority,
    ) -> Option<Self::AuthorityCommitCheckpoint<'a>>;

    fn commit_authority(
        reservation: ReservedCaptureSessionAuthority,
        checkpoint: Self::AuthorityCommitCheckpoint<'_>,
    );

    fn retire_authority(
        &self,
        handles: CaptureBackendHandles<'_, Self>,
        authority: CaptureSessionAuthority,
        cause: CaptureRetirementCause,
    );
}

#[derive(Debug)]
pub(in crate::input::screen) enum CaptureSuccessorPreparationError {
    Unavailable,
    AuthorityExhausted(CaptureSessionAuthorityExhausted),
    Worker(anyhow::Error),
}

pub(in crate::input::screen) struct ScreenCaptureAdapter<B: CaptureBackend> {
    sessions: CaptureSessionSet<B::Worker>,
    backend: B,
    settings: Arc<VersionedCaptureSettings<B::SettingsConfig>>,
    activity: CaptureActivityHandle<B>,
    exact: Arc<B::ExactState>,
}

impl<B> Default for ScreenCaptureAdapter<B>
where
    B: CaptureBackend,
    B: Default,
    B::SettingsConfig: Default,
    B::ExactState: Default,
{
    fn default() -> Self {
        ScreenCaptureAdapterAssembly::new(
            Arc::new(B::ExactState::default()),
            B::SettingsConfig::default(),
        )
        .finish(B::default())
    }
}

impl<B: CaptureBackend> ScreenCaptureAdapter<B> {
    fn handles(&self) -> CaptureBackendHandles<'_, B> {
        CaptureBackendHandles {
            settings: &self.settings,
            activity: &self.activity,
            exact: &self.exact,
        }
    }

    #[cfg_attr(not(any(feature = "macos-capture-fixtures", test)), allow(dead_code))]
    pub(in crate::input::screen) fn backend(&self) -> &B {
        &self.backend
    }

    pub(in crate::input::screen) fn settings(
        &self,
    ) -> &VersionedCaptureSettings<B::SettingsConfig> {
        &self.settings
    }

    #[cfg(test)]
    pub(in crate::input::screen) fn settings_handle(
        &self,
    ) -> Arc<VersionedCaptureSettings<B::SettingsConfig>> {
        Arc::clone(&self.settings)
    }

    pub(in crate::input::screen) fn activity(&self) -> &Mutex<BackendActivity<B>> {
        self.activity.activity()
    }

    #[cfg(any(feature = "windows-capture-fixtures", test))]
    pub(in crate::input::screen) fn activity_handle(&self) -> Arc<Mutex<BackendActivity<B>>> {
        self.activity.activity_handle()
    }

    pub(in crate::input::screen) fn exact_state(&self) -> &B::ExactState {
        &self.exact
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(in crate::input::screen) fn exact_state_handle(&self) -> Arc<B::ExactState> {
        Arc::clone(&self.exact)
    }

    #[cfg(test)]
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

    pub(in crate::input::screen) fn resolve_exact_publication_branch(
        &self,
        demand: &RegisteredScreenBranchDemand,
    ) -> anyhow::Result<Option<ResolvedScreenBranchDemand>> {
        let Some(source) = self.exact.common().source() else {
            tracing::debug!(
                shared = ?std::ptr::from_ref(self.exact.as_ref()),
                "exact branch unresolvable: no publication source installed"
            );
            return Ok(None);
        };
        self.backend
            .resolve_publication_branch(self.settings.as_ref(), &source, demand)
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

    #[cfg(test)]
    pub(in crate::input::screen) fn prepare_worker(
        &self,
        request: B::SpawnRequest,
        reservation: ReservedCaptureSessionAuthority,
    ) -> anyhow::Result<PreparedCaptureSession<B::Worker>> {
        self.backend
            .spawn_worker(request, self.handles(), reservation)?
            .prepare(CaptureSessionDeadline::after(B::READINESS_TIMEOUT))
    }

    pub(in crate::input::screen) fn prepare_successor(
        &self,
        request: B::SpawnRequest,
    ) -> Result<PreparedCaptureSession<B::Worker>, CaptureSuccessorPreparationError> {
        if !self.sessions.can_prepare_successor() {
            return Err(CaptureSuccessorPreparationError::Unavailable);
        }
        let reservation = self
            .exact
            .common()
            .reserve_authority()
            .map_err(CaptureSuccessorPreparationError::AuthorityExhausted)?;
        self.backend
            .spawn_worker(request, self.handles(), reservation)
            .map_err(CaptureSuccessorPreparationError::Worker)?
            .prepare(CaptureSessionDeadline::after(B::READINESS_TIMEOUT))
            .map_err(CaptureSuccessorPreparationError::Worker)
    }

    /// Prepares, commits, and starts one successor worker.
    ///
    /// Returns the committed authority, which is the reservation the backend
    /// spawned under. Every preparation failure is mapped with the backend
    /// name so sources do not repeat that mapping.
    pub(in crate::input::screen) fn spawn_successor(
        &mut self,
        request: B::SpawnRequest,
    ) -> anyhow::Result<CaptureSessionAuthority> {
        let prepared = self.stage_successor(request)?;
        let reserved = prepared.authority();
        let committed = self.commit_worker(prepared).map_err(|_| {
            anyhow::anyhow!("{} successor admission changed before commit", B::NAME)
        })?;
        assert_eq!(committed, reserved);
        Ok(committed)
    }

    /// Prepares one successor without committing it, mapping every
    /// preparation failure with the backend name.
    pub(in crate::input::screen) fn stage_successor(
        &self,
        request: B::SpawnRequest,
    ) -> anyhow::Result<PreparedCaptureSession<B::Worker>> {
        self.prepare_successor(request)
            .map_err(|error| match error {
                CaptureSuccessorPreparationError::Unavailable => {
                    anyhow::anyhow!("previous {} worker is still stopping", B::NAME)
                }
                CaptureSuccessorPreparationError::AuthorityExhausted(error) => {
                    anyhow::anyhow!("{} session generation exhausted: {error}", B::NAME)
                }
                CaptureSuccessorPreparationError::Worker(error) => error,
            })
    }

    /// Retires the active worker (if any) and reaps every finished retiree.
    pub(in crate::input::screen) fn shutdown(&mut self) -> bool {
        let retired = self.retire_active_worker();
        self.reap_retired_workers();
        retired
    }

    /// Reaps finished retirees, logging any abnormal exit under the backend name.
    pub(in crate::input::screen) fn reap_retired_workers(&mut self) {
        self.reap_finished_workers(|authority, exit| {
            if let Some(failure) = exit.failure() {
                tracing::warn!(
                    backend = B::NAME,
                    generation = authority.generation(),
                    failure,
                    "retired capture worker failed"
                );
            }
        });
    }

    /// Observes an active worker that finished on its own.
    ///
    /// The source inspects the exit first (status publication happens
    /// before authority retirement), then the backend policy runs for the
    /// observed exit and finished retirees are reaped. Returns `None`, after
    /// reaping, when the active worker is still running.
    pub(in crate::input::screen) fn observe_exit<T>(
        &mut self,
        on_exit: impl FnOnce(CaptureSessionAuthority, <B::Worker as CaptureSession>::Exit) -> T,
    ) -> Option<T> {
        let Some((authority, exit)) = self.take_finished_active_worker() else {
            self.reap_retired_workers();
            return None;
        };
        let observed = on_exit(authority, exit);
        self.retire_finished_worker(authority);
        self.reap_retired_workers();
        Some(observed)
    }

    pub(in crate::input::screen) fn commit_worker(
        &mut self,
        prepared: PreparedCaptureSession<B::Worker>,
    ) -> Result<CaptureSessionAuthority, PreparedCaptureSession<B::Worker>> {
        let Self {
            sessions,
            backend,
            settings,
            activity,
            exact,
        } = self;
        let handles = CaptureBackendHandles {
            settings,
            activity,
            exact,
        };
        prepared
            .commit_into(
                sessions,
                |reservation| backend.prepare_authority_commit(handles, reservation),
                B::commit_authority,
            )
            .map(|commit| commit.authority())
    }

    pub(in crate::input::screen) const fn active_worker(&self) -> Option<&B::Worker> {
        self.sessions.active()
    }

    pub(in crate::input::screen) fn begin_exact_preparation(
        &mut self,
        ticket: ScreenWorkerPreparationTicket,
    ) -> anyhow::Result<ScreenWorkerPreparation> {
        let endpoint = self.sessions.exact_endpoint().ok_or_else(|| {
            anyhow::anyhow!(
                "{} worker is unavailable for exact publication preparation",
                B::NAME
            )
        })?;
        begin_capture_exact_preparation(&endpoint, ticket)
    }

    pub(in crate::input::screen) fn begin_exact_retirement(
        &mut self,
    ) -> Option<ScreenWorkerRetirement> {
        let endpoint = self.sessions.exact_endpoint()?;
        Some(begin_capture_exact_retirement(&endpoint))
    }

    #[cfg(test)]
    pub(in crate::input::screen) fn can_prepare_successor(&self) -> bool {
        self.sessions.can_prepare_successor()
    }

    pub(in crate::input::screen) fn can_install_successor(&self) -> bool {
        self.sessions.can_install_successor()
    }

    pub(in crate::input::screen) fn retire_active_worker(&mut self) -> bool {
        let Some(authority) = self.sessions.retire_active() else {
            return false;
        };
        self.backend.retire_authority(
            self.handles(),
            authority,
            CaptureRetirementCause::ActiveStop,
        );
        true
    }

    pub(in crate::input::screen) fn take_finished_active_worker(
        &mut self,
    ) -> Option<(CaptureSessionAuthority, <B::Worker as CaptureSession>::Exit)> {
        self.sessions.take_finished_active()
    }

    pub(in crate::input::screen) fn retire_finished_worker(
        &self,
        authority: CaptureSessionAuthority,
    ) {
        self.backend.retire_authority(
            self.handles(),
            authority,
            CaptureRetirementCause::ObservedExit,
        );
    }

    pub(in crate::input::screen) fn reap_finished_workers(
        &mut self,
        on_exit: impl FnMut(CaptureSessionAuthority, <B::Worker as CaptureSession>::Exit),
    ) {
        self.sessions.reap_finished(on_exit);
    }

    pub(in crate::input::screen) fn take_active_worker_for_settlement(
        &mut self,
    ) -> Option<B::Worker> {
        self.sessions.take_active_for_settlement()
    }

    pub(in crate::input::screen) fn retire_settled_worker(
        &self,
        authority: CaptureSessionAuthority,
    ) {
        self.backend.retire_authority(
            self.handles(),
            authority,
            CaptureRetirementCause::ExclusiveSettlement,
        );
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

impl<B: CaptureBackend> Drop for ScreenCaptureAdapter<B> {
    fn drop(&mut self) {
        self.retire_active_worker();
    }
}
