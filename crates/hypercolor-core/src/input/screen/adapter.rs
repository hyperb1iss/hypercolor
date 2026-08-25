mod authority;
mod backend;
pub(in crate::input::screen) mod exact;
mod session;
mod settings;
mod shell;
#[cfg(test)]
mod tests;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tokio::sync::oneshot;

use super::{
    ExactBoxList, ExactBoxNode, ScreenCommittedState, ScreenPreparedWorkerToken,
    ScreenPublicationHub, ScreenWorkerBinding, ScreenWorkerBindingState, ScreenWorkerPreparation,
    ScreenWorkerPreparationTicket, ScreenWorkerRetirement,
};

pub(in crate::input::screen) use authority::{
    CaptureSessionAuthority, CaptureSessionAuthorityExhausted, CaptureSessionAuthoritySequencer,
    ReservedCaptureSessionAuthority, StaleCaptureSessionReservation,
};
#[cfg(test)]
pub(in crate::input::screen) use backend::CaptureSuccessorPreparationError;
pub(in crate::input::screen) use backend::{
    CaptureActivityHandle, CaptureBackend, CaptureBackendHandles, CaptureRetirementCause,
    ScreenCaptureAdapter, ScreenCaptureAdapterAssembly,
};
pub(in crate::input::screen) use exact::{
    CaptureExactPublicationShared, CaptureExactState, CaptureOwnedSource, CaptureOwnedSourceRecord,
    CapturePublicationSource, CpuExecutorSlot, finish_removed_capture_exact_source,
    preflight_capture_exact_scope_bytes,
};
pub(in crate::input::screen) use session::{
    CaptureCommandEndpoint, CaptureCommandSender, CaptureSession, CaptureSessionDeadline,
    CaptureSessionExit, CaptureSessionReadiness, CaptureSessionSet, CaptureSessionTransaction,
    CaptureSuccessorPolicy, PreparedCaptureSession,
};
pub(in crate::input::screen) use settings::{
    CaptureSettingsAdopter, CaptureSettingsAdoption, VersionedCaptureSettings,
    begin_capture_settings_adoption,
};
pub(in crate::input::screen) use shell::CaptureSourceShell;

pub enum CaptureExactCommand {
    Prepare {
        authority: CaptureSessionAuthority,
        ticket: ScreenWorkerPreparationTicket,
        cancelled: Arc<AtomicBool>,
        completion: oneshot::Sender<anyhow::Result<ScreenPreparedWorkerToken>>,
    },
    Reap {
        authority: CaptureSessionAuthority,
        completion: Option<oneshot::Sender<anyhow::Result<()>>>,
    },
}

/// Worker mailbox envelope: shared exact-runtime commands, the shared stop
/// request, plus the backend's own native command vocabulary.
pub(in crate::input::screen) enum CaptureWorkerCommand<C> {
    Exact(CaptureExactCommand),
    /// Asks the worker to leave its loop at the next checkpoint. Sessions
    /// send it from `wake` and `Drop` so a blocked worker observes the abort
    /// flag promptly.
    Stop,
    Backend(C),
}

pub struct CaptureExactCommandRejected;

pub trait CaptureExactRuntimeOwner {
    type Source: CapturePublicationSource;

    const BACKEND_NAME: &'static str;
    const ABORTED_BINDING_ERROR: &'static str;

    fn source(&self) -> &Self::Source;
    fn binding(&self) -> &ScreenWorkerBinding;
    fn bind_routes(&mut self, authority: &ScreenCommittedState) -> anyhow::Result<bool>;
    fn is_bound(&self) -> bool;
}

pub trait CaptureExactRuntimeCollection<R> {
    fn iter_mut<'a>(&'a mut self) -> impl Iterator<Item = &'a mut R>
    where
        R: 'a;
}

pub trait CaptureExactRuntimeStore<R>: CaptureExactRuntimeCollection<R> {
    type Prepared;

    fn prepare(runtime: R) -> Self::Prepared;

    fn install(&mut self, prepared: Self::Prepared);

    fn retain(&mut self, retain: impl FnMut(&R) -> bool);
}

impl<R> CaptureExactRuntimeCollection<R> for ExactBoxList<R> {
    fn iter_mut<'a>(&'a mut self) -> impl Iterator<Item = &'a mut R>
    where
        R: 'a,
    {
        ExactBoxList::iter_mut(self)
    }
}

impl<R> CaptureExactRuntimeStore<R> for ExactBoxList<R> {
    type Prepared = Box<ExactBoxNode<R>>;

    fn prepare(runtime: R) -> Self::Prepared {
        Self::boxed_node(runtime)
    }

    fn install(&mut self, prepared: Self::Prepared) {
        self.push_boxed(prepared);
    }

    fn retain(&mut self, mut retain: impl FnMut(&R) -> bool) {
        ExactBoxList::retain(self, |runtime| retain(runtime));
    }
}

impl<R> CaptureExactRuntimeCollection<R> for Vec<R> {
    fn iter_mut<'a>(&'a mut self) -> impl Iterator<Item = &'a mut R>
    where
        R: 'a,
    {
        self.as_mut_slice().iter_mut()
    }
}

impl<R> CaptureExactRuntimeStore<R> for Vec<R> {
    type Prepared = R;

    fn prepare(runtime: R) -> Self::Prepared {
        runtime
    }

    fn install(&mut self, prepared: Self::Prepared) {
        self.push(prepared);
    }

    fn retain(&mut self, retain: impl FnMut(&R) -> bool) {
        Vec::retain(self, retain);
    }
}

impl<R> CaptureExactRuntimeCollection<R> for [R] {
    fn iter_mut<'a>(&'a mut self) -> impl Iterator<Item = &'a mut R>
    where
        R: 'a,
    {
        <[R]>::iter_mut(self)
    }
}

pub trait CaptureExactCommandEndpoint: Clone + Send + 'static {
    fn source_name(&self) -> &'static str;

    fn authority(&self) -> CaptureSessionAuthority;

    fn send_exact(&self, command: CaptureExactCommand) -> Result<(), CaptureExactCommandRejected>;

    fn wake(&self) {}
}

fn exact_preparation_abort<E>(
    endpoint: E,
    authority: CaptureSessionAuthority,
    cancelled: Arc<AtomicBool>,
) -> impl FnOnce()
where
    E: CaptureExactCommandEndpoint,
{
    move || {
        cancelled.store(true, Ordering::Release);
        let _ = endpoint.send_exact(CaptureExactCommand::Reap {
            authority,
            completion: None,
        });
        endpoint.wake();
    }
}

pub fn begin_capture_exact_preparation<E>(
    endpoint: &E,
    ticket: ScreenWorkerPreparationTicket,
) -> anyhow::Result<ScreenWorkerPreparation>
where
    E: CaptureExactCommandEndpoint,
{
    let cancelled = Arc::new(AtomicBool::new(false));
    let (completion, completed) = oneshot::channel();
    let authority = endpoint.authority();
    endpoint
        .send_exact(CaptureExactCommand::Prepare {
            authority,
            ticket,
            cancelled: Arc::clone(&cancelled),
            completion,
        })
        .map_err(|_| {
            anyhow::anyhow!(
                "{} worker rejected exact publication preparation",
                endpoint.source_name()
            )
        })?;
    endpoint.wake();
    let source_name = endpoint.source_name();
    let abort = exact_preparation_abort(endpoint.clone(), authority, cancelled);
    Ok(ScreenWorkerPreparation::with_abort(
        async move {
            completed.await.map_err(|_| {
                anyhow::anyhow!("{source_name} worker exited during exact publication preparation")
            })?
        },
        abort,
    ))
}

pub fn begin_capture_exact_retirement<E>(endpoint: &E) -> ScreenWorkerRetirement
where
    E: CaptureExactCommandEndpoint,
{
    let (completion, completed) = oneshot::channel();
    let source_name = endpoint.source_name();
    if endpoint
        .send_exact(CaptureExactCommand::Reap {
            authority: endpoint.authority(),
            completion: Some(completion),
        })
        .is_err()
    {
        return ScreenWorkerRetirement::new(async move {
            Err(anyhow::anyhow!(
                "{source_name} worker rejected exact publication retirement"
            ))
        });
    }
    endpoint.wake();
    ScreenWorkerRetirement::new(async move {
        completed.await.map_err(|_| {
            anyhow::anyhow!("{source_name} worker exited during exact publication retirement")
        })?
    })
}

pub fn execute_capture_exact_command<S, O, R, C>(
    command: CaptureExactCommand,
    shared: &CaptureExactPublicationShared<S, O>,
    runtimes: &mut C,
    prepare: impl FnOnce(
        ScreenWorkerPreparationTicket,
        Option<&S>,
    ) -> anyhow::Result<(ScreenPreparedWorkerToken, Option<(R, O)>)>,
) where
    S: CapturePublicationSource,
    O: CaptureOwnedSource,
    R: CaptureExactRuntimeOwner,
    C: CaptureExactRuntimeStore<R>,
{
    match command {
        CaptureExactCommand::Prepare {
            authority,
            ticket,
            cancelled,
            completion,
        } => {
            if cancelled.load(Ordering::Acquire) {
                let _ = completion.send(Err(anyhow::anyhow!(
                    "{} exact publication preparation was cancelled",
                    R::BACKEND_NAME
                )));
                return;
            }
            let Some(source) = shared.with_current_source(authority, |source| source) else {
                let _ = completion.send(Err(anyhow::anyhow!(
                    "{} exact publication authority is stale",
                    R::BACKEND_NAME
                )));
                return;
            };
            match prepare(ticket, source.as_ref()) {
                Ok((token, runtime)) if !cancelled.load(Ordering::Acquire) => {
                    let runtime = runtime.map(|(runtime, owned_source)| {
                        (C::prepare(runtime), ExactBoxList::boxed_node(owned_source))
                    });
                    let mut completion = Some(completion);
                    let committed = shared.with_current_owned_sources(authority, |owned_sources| {
                        if let Some((runtime, owned_source)) = runtime {
                            owned_sources.push_boxed(owned_source);
                            runtimes.install(runtime);
                        }
                        completion
                            .take()
                            .expect("exact preparation completion is sent once")
                            .send(Ok(token))
                    });
                    match committed {
                        Some(Ok(())) => {}
                        Some(Err(_)) => {
                            reap_capture_exact_runtimes(authority, runtimes, shared);
                        }
                        None => {
                            let _ = completion
                                .take()
                                .expect("stale exact preparation retains its completion")
                                .send(Err(anyhow::anyhow!(
                                    "{} exact publication authority changed during preparation",
                                    R::BACKEND_NAME
                                )));
                        }
                    }
                }
                Ok((_token, _runtime)) => {
                    let _ = completion.send(Err(anyhow::anyhow!(
                        "{} exact publication preparation was cancelled",
                        R::BACKEND_NAME
                    )));
                }
                Err(error) => {
                    let _ = completion.send(Err(error));
                }
            }
        }
        CaptureExactCommand::Reap {
            authority,
            completion,
        } => {
            reap_capture_exact_runtimes(authority, runtimes, shared);
            if let Some(completion) = completion {
                let _ = completion.send(Ok(()));
            }
        }
    }
}

pub fn reap_capture_exact_runtimes<S, O, R, C>(
    session_authority: CaptureSessionAuthority,
    runtimes: &mut C,
    shared: &CaptureExactPublicationShared<S, O>,
) where
    S: CapturePublicationSource,
    O: CaptureOwnedSource,
    R: CaptureExactRuntimeOwner,
    C: CaptureExactRuntimeStore<R>,
{
    if !shared.reap_owned_sources_if_current(session_authority) {
        return;
    }
    let authority = shared.hub().map(|hub| hub.committed_state());
    runtimes.retain(|runtime| {
        authority
            .as_ref()
            .is_some_and(|authority| authority.owns_runtime_binding(runtime.binding()))
    });
}

pub fn bind_current_capture_exact_runtime<'a, R, C>(
    runtimes: &'a mut C,
    source: &R::Source,
    hub: &ScreenPublicationHub,
    after_bind: impl FnOnce(&mut C, &ScreenWorkerBinding) -> anyhow::Result<()>,
) -> anyhow::Result<Option<&'a mut R>>
where
    R: CaptureExactRuntimeOwner,
    C: CaptureExactRuntimeCollection<R> + ?Sized,
{
    let authority = hub.committed_state();
    let Some(current_binding) = authority.runtime_binding(source.source_id()).cloned() else {
        return Ok(None);
    };
    let newly_bound = {
        let runtime = runtimes.iter_mut().find(|runtime| {
            runtime.source() == source && runtime.binding().is_same(&current_binding)
        });
        let Some(runtime) = runtime else {
            return Ok(None);
        };
        if !authority.owns_runtime_binding(runtime.binding()) {
            return Ok(None);
        }
        match runtime.binding().state() {
            ScreenWorkerBindingState::Active | ScreenWorkerBindingState::Retired => {}
            ScreenWorkerBindingState::Prepared | ScreenWorkerBindingState::Armed => {
                return Ok(None);
            }
            ScreenWorkerBindingState::Aborted => {
                anyhow::bail!(R::ABORTED_BINDING_ERROR);
            }
        }
        runtime.bind_routes(&authority)?
    };
    if newly_bound {
        after_bind(runtimes, &current_binding)?;
    }
    Ok(runtimes.iter_mut().find(|runtime| {
        runtime.source() == source
            && runtime.binding().is_same(&current_binding)
            && runtime.is_bound()
    }))
}

pub trait CapturePublicationFence<E> {
    fn admits(&self, epoch: &E) -> bool;
}

/// Fenced record of the capture epoch a worker session currently serves.
///
/// Backends activate one epoch per live session and consult it before
/// accepting frames; the fence retires every epoch a superseded authority
/// could still name. The exact publication hub owns the frames themselves.
pub struct CaptureActivity<F, E> {
    fence: F,
    active: Option<E>,
}

impl<F, E> Default for CaptureActivity<F, E>
where
    F: Default,
{
    fn default() -> Self {
        Self {
            fence: F::default(),
            active: None,
        }
    }
}

impl<F, E> CaptureActivity<F, E>
where
    F: CapturePublicationFence<E>,
    E: PartialEq,
{
    /// Activate `active` when the fence admits it.
    ///
    /// Returns the displaced epoch when a different one was active, `Ok(None)`
    /// when it was already active, and the rejected epoch when the fence
    /// refuses it.
    pub fn activate(&mut self, active: E) -> Result<Option<E>, E> {
        if !self.fence.admits(&active) {
            return Err(active);
        }
        if self.active.as_ref() == Some(&active) {
            return Ok(None);
        }
        Ok(self.active.replace(active))
    }

    pub fn replace_fence_and_activate(&mut self, fence: F, active: E) -> Result<Option<E>, E> {
        self.fence = fence;
        self.activate(active)
    }

    pub fn replace_fence(&mut self, fence: F) -> Option<E> {
        self.fence = fence;
        self.clear()
    }

    /// Install `fence` when it differs from the current one, returning the
    /// epoch that installation displaced.
    pub fn replace_fence_if_changed(&mut self, fence: F) -> Option<E>
    where
        F: PartialEq,
    {
        if self.fence == fence {
            return None;
        }
        self.replace_fence(fence)
    }

    pub fn fence(&self) -> &F {
        &self.fence
    }

    #[cfg(feature = "macos-capture-fixtures")]
    pub fn is_active(&self, active: &E) -> bool {
        self.active.as_ref() == Some(active)
    }

    #[cfg(any(feature = "windows-capture-fixtures", test))]
    pub fn active(&self) -> Option<&E> {
        self.active.as_ref()
    }

    pub fn clear(&mut self) -> Option<E> {
        self.active.take()
    }
}
