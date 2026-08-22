mod authority;
mod backend;
pub(in crate::input::screen) mod exact;
mod session;
#[cfg(any(target_os = "linux", target_os = "windows", test))]
mod settings;
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
pub(in crate::input::screen) use backend::{CaptureBackend, ScreenCaptureAdapter};
pub(in crate::input::screen) use exact::{
    CaptureExactPublicationShared, CaptureExactState, CaptureOwnedSource, CapturePublicationSource,
};
pub(in crate::input::screen) use session::{
    CaptureSession, CaptureSessionDeadline, CaptureSessionReadiness, CaptureSessionSet,
    CaptureSessionTransaction, CaptureSuccessorPolicy, PreparedCaptureSession,
};
#[cfg(any(target_os = "linux", target_os = "windows", test))]
pub(in crate::input::screen) use settings::VersionedCaptureSettings;

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
    const SOURCE_NAME: &'static str;

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
                E::SOURCE_NAME
            )
        })?;
    endpoint.wake();
    let abort = exact_preparation_abort(endpoint.clone(), authority, cancelled);
    Ok(ScreenWorkerPreparation::with_abort(
        async move {
            completed.await.map_err(|_| {
                anyhow::anyhow!(
                    "{} worker exited during exact publication preparation",
                    E::SOURCE_NAME
                )
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
    if endpoint
        .send_exact(CaptureExactCommand::Reap {
            authority: endpoint.authority(),
            completion: Some(completion),
        })
        .is_err()
    {
        return ScreenWorkerRetirement::new(async {
            Err(anyhow::anyhow!(
                "{} worker rejected exact publication retirement",
                E::SOURCE_NAME
            ))
        });
    }
    endpoint.wake();
    ScreenWorkerRetirement::new(async move {
        completed.await.map_err(|_| {
            anyhow::anyhow!(
                "{} worker exited during exact publication retirement",
                E::SOURCE_NAME
            )
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

pub struct CapturePublicationCheckpoint<T> {
    latest: Option<T>,
}

#[cfg(any(
    feature = "macos-capture-fixtures",
    target_os = "linux",
    target_os = "windows",
    test
))]
pub struct CapturePublicationSnapshot<E, T> {
    pub(super) epoch: E,
    pub(super) revision: u64,
    pub(super) value: T,
}

#[cfg(any(target_os = "linux", target_os = "windows", test))]
pub struct DisplacedCapturePublication<T> {
    pub(super) latest: Option<T>,
}

pub struct CapturePublication<F, E, T> {
    fence: F,
    revision: u64,
    active: Option<E>,
    latest: Option<T>,
}

impl<F, E, T> Default for CapturePublication<F, E, T>
where
    F: Default,
{
    fn default() -> Self {
        Self {
            fence: F::default(),
            revision: 0,
            active: None,
            latest: None,
        }
    }
}

impl<F, E, T> CapturePublication<F, E, T>
where
    F: CapturePublicationFence<E>,
    E: PartialEq,
{
    #[cfg(any(target_os = "linux", target_os = "windows", test))]
    pub fn activate(&mut self, active: E) -> Result<Option<DisplacedCapturePublication<T>>, E> {
        if !self.fence.admits(&active) {
            return Err(active);
        }
        if self.active.as_ref() != Some(&active) {
            let displaced = DisplacedCapturePublication {
                latest: self.latest.take(),
            };
            self.active = Some(active);
            return Ok(Some(displaced));
        }
        Ok(None)
    }

    pub fn activate_preserving_latest(&mut self, active: E) -> Result<Option<E>, E> {
        if !self.fence.admits(&active) {
            return Err(active);
        }
        if self.active.as_ref() == Some(&active) {
            return Ok(None);
        }
        Ok(self.active.replace(active))
    }

    pub fn replace_fence_preserving_latest(&mut self, fence: F, active: E) -> Result<Option<E>, E> {
        self.fence = fence;
        self.activate_preserving_latest(active)
    }

    #[cfg(any(target_os = "linux", target_os = "windows", test))]
    pub fn replace_fence(&mut self, fence: F) -> DisplacedCapturePublication<T> {
        self.fence = fence;
        self.clear()
    }

    #[cfg(any(target_os = "linux", test))]
    pub fn replace_fence_if_changed(&mut self, fence: F) -> Option<DisplacedCapturePublication<T>>
    where
        F: PartialEq,
    {
        (self.fence != fence).then(|| self.replace_fence(fence))
    }

    #[cfg(any(target_os = "linux", target_os = "windows", test))]
    pub fn fence(&self) -> &F {
        &self.fence
    }

    #[cfg(any(
        feature = "macos-capture-fixtures",
        target_os = "linux",
        target_os = "windows",
        test
    ))]
    pub fn is_active(&self, active: &E) -> bool {
        self.active.as_ref() == Some(active)
    }

    #[cfg(any(target_os = "windows", test))]
    pub fn active(&self) -> Option<&E> {
        self.active.as_ref()
    }

    #[cfg(any(target_os = "windows", test))]
    pub fn latest(&self) -> Option<&T> {
        self.latest.as_ref()
    }

    #[cfg(any(target_os = "linux", target_os = "windows", test))]
    pub fn clear(&mut self) -> DisplacedCapturePublication<T> {
        self.active = None;
        DisplacedCapturePublication {
            latest: self.latest.take(),
        }
    }

    pub fn clear_latest(&mut self) -> Option<T> {
        self.latest.take()
    }

    pub fn checkpoint(&self) -> CapturePublicationCheckpoint<T>
    where
        T: Clone,
    {
        CapturePublicationCheckpoint {
            latest: self.latest.clone(),
        }
    }

    pub fn restore_checkpoint(
        &mut self,
        active: Option<&E>,
        checkpoint: CapturePublicationCheckpoint<T>,
    ) -> Result<Option<T>, CapturePublicationCheckpoint<T>> {
        if self.active.as_ref() != active {
            return Err(checkpoint);
        }
        if checkpoint.latest.is_some() {
            self.revision = self.revision.wrapping_add(1);
        }
        Ok(std::mem::replace(&mut self.latest, checkpoint.latest))
    }

    #[cfg(any(
        feature = "macos-capture-fixtures",
        target_os = "linux",
        target_os = "windows",
        test
    ))]
    pub fn publish(&mut self, active: &E, value: T) -> Result<Option<T>, T> {
        if self.active.as_ref() != Some(active) {
            return Err(value);
        }
        self.revision = self.revision.wrapping_add(1);
        Ok(self.latest.replace(value))
    }

    #[cfg(any(
        feature = "macos-capture-fixtures",
        target_os = "linux",
        target_os = "windows",
        test
    ))]
    pub fn snapshot(&self) -> Option<CapturePublicationSnapshot<E, T>>
    where
        E: Clone,
        T: Clone,
    {
        Some(CapturePublicationSnapshot {
            epoch: self.active.as_ref()?.clone(),
            revision: self.revision,
            value: self.latest.as_ref()?.clone(),
        })
    }
}
