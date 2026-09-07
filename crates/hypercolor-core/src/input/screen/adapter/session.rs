use std::marker::PhantomData;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use super::{
    CaptureExactCommand, CaptureExactCommandEndpoint, CaptureExactCommandRejected,
    CaptureSessionAuthority, CaptureWorkerCommand, ReservedCaptureSessionAuthority,
};

/// Transport that delivers envelopes to a worker loop.
///
/// Implemented for `std::sync::mpsc` senders here; backends whose worker runs
/// on a native event loop implement it for that loop's sender beside the
/// backend.
pub(in crate::input::screen) trait CaptureCommandSender<C>:
    Clone + Send + 'static
{
    /// Delivers one envelope; the worker being gone is the only failure.
    fn send_command(
        &self,
        command: CaptureWorkerCommand<C>,
    ) -> Result<(), CaptureExactCommandRejected>;
}

impl<C: Send + 'static> CaptureCommandSender<C> for mpsc::Sender<CaptureWorkerCommand<C>> {
    fn send_command(
        &self,
        command: CaptureWorkerCommand<C>,
    ) -> Result<(), CaptureExactCommandRejected> {
        self.send(command).map_err(|_| CaptureExactCommandRejected)
    }
}

/// Where an endpoint reads the session authority it stamps on commands.
///
/// Most sessions hold one authority for their whole life; a session that
/// re-keys itself in place (the Wayland worker reconnects under a successor
/// generation without a new thread) reads a live counter instead.
pub(in crate::input::screen) trait CaptureAuthoritySource:
    Clone + Send + 'static
{
    fn current_authority(&self) -> CaptureSessionAuthority;
}

impl CaptureAuthoritySource for CaptureSessionAuthority {
    fn current_authority(&self) -> CaptureSessionAuthority {
        *self
    }
}

impl CaptureAuthoritySource for Arc<AtomicU64> {
    fn current_authority(&self) -> CaptureSessionAuthority {
        CaptureSessionAuthority::new(self.load(Ordering::Acquire))
    }
}

type WakeHook = Arc<dyn Fn() + Send + Sync>;

/// Shared exact-command endpoint over any worker transport.
///
/// Backends hand out clones of this from `CaptureSession::exact_endpoint`;
/// the adapter never sees the transport or the native command vocabulary.
pub(in crate::input::screen) struct CaptureCommandEndpoint<C, S, A> {
    source_name: &'static str,
    authority: A,
    sender: S,
    wake: Option<WakeHook>,
    _command: PhantomData<fn() -> C>,
}

impl<C, S: Clone, A: Clone> Clone for CaptureCommandEndpoint<C, S, A> {
    fn clone(&self) -> Self {
        Self {
            source_name: self.source_name,
            authority: self.authority.clone(),
            sender: self.sender.clone(),
            wake: self.wake.clone(),
            _command: PhantomData,
        }
    }
}

impl<C, S, A> CaptureCommandEndpoint<C, S, A>
where
    S: CaptureCommandSender<C>,
    A: CaptureAuthoritySource,
{
    pub(in crate::input::screen) fn new(
        source_name: &'static str,
        authority: A,
        sender: S,
    ) -> Self {
        Self {
            source_name,
            authority,
            sender,
            wake: None,
            _command: PhantomData,
        }
    }

    /// Installs a hook that nudges a worker blocked outside its mailbox.
    pub(in crate::input::screen) fn with_wake(
        mut self,
        wake: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        self.wake = Some(Arc::new(wake));
        self
    }
}

impl<C, S, A> CaptureExactCommandEndpoint for CaptureCommandEndpoint<C, S, A>
where
    C: 'static,
    S: CaptureCommandSender<C>,
    A: CaptureAuthoritySource,
{
    fn source_name(&self) -> &'static str {
        self.source_name
    }

    fn authority(&self) -> CaptureSessionAuthority {
        self.authority.current_authority()
    }

    fn send_exact(&self, command: CaptureExactCommand) -> Result<(), CaptureExactCommandRejected> {
        self.sender
            .send_command(CaptureWorkerCommand::Exact(command))
    }

    fn wake(&self) {
        if let Some(wake) = &self.wake {
            wake();
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::input::screen) enum CaptureSuccessorPolicy {
    WaitForRetirement,
    AllowOverlap,
}

/// What a finished worker leaves behind for the source to inspect.
pub(in crate::input::screen) trait CaptureSessionExit {
    /// Describes an abnormal exit (panic or error); `None` for a clean exit.
    fn failure(&self) -> Option<String>;
}

impl CaptureSessionExit for std::thread::Result<()> {
    fn failure(&self) -> Option<String> {
        self.as_ref()
            .err()
            .map(|panic| format!("worker panicked: {panic:?}"))
    }
}

impl CaptureSessionExit for anyhow::Result<()> {
    fn failure(&self) -> Option<String> {
        self.as_ref().err().map(|error| format!("{error:#}"))
    }
}

pub(in crate::input::screen) trait CaptureSession: Sized {
    type Exit: CaptureSessionExit;
    type ExactEndpoint: CaptureExactCommandEndpoint;

    const SUCCESSOR_POLICY: CaptureSuccessorPolicy;

    fn authority(&self) -> CaptureSessionAuthority;
    fn exact_endpoint(&self) -> Self::ExactEndpoint;
    fn abort(&self);
    fn retire_for_successor(&self) {
        self.abort();
    }
    fn wake(&self);
    fn start(&self);
    fn is_finished(&self) -> bool;
    fn finish(self) -> Self::Exit;
    fn detach(self);
}

#[derive(Clone, Copy)]
pub(in crate::input::screen) struct CaptureSessionDeadline(Instant);

impl CaptureSessionDeadline {
    pub(in crate::input::screen) fn after(timeout: Duration) -> Self {
        Self(Instant::now() + timeout)
    }

    pub(in crate::input::screen) fn remaining(self) -> Duration {
        self.0.saturating_duration_since(Instant::now())
    }
}

pub(in crate::input::screen) trait CaptureSessionReadiness {
    fn wait(self, deadline: CaptureSessionDeadline) -> anyhow::Result<()>;
}

impl CaptureSessionReadiness for () {
    fn wait(self, _deadline: CaptureSessionDeadline) -> anyhow::Result<()> {
        Ok(())
    }
}

pub(in crate::input::screen) struct CaptureSessionTransaction<S, R>
where
    S: CaptureSession,
    R: CaptureSessionReadiness,
{
    session: Option<S>,
    readiness: Option<R>,
    reservation: Option<ReservedCaptureSessionAuthority>,
}

pub(in crate::input::screen) struct PreparedCaptureSession<S: CaptureSession> {
    session: Option<S>,
    reservation: Option<ReservedCaptureSessionAuthority>,
}

pub(in crate::input::screen) struct CaptureSessionCommit {
    authority: CaptureSessionAuthority,
}

impl CaptureSessionCommit {
    pub(in crate::input::screen) const fn authority(&self) -> CaptureSessionAuthority {
        self.authority
    }
}

impl<S, R> CaptureSessionTransaction<S, R>
where
    S: CaptureSession,
    R: CaptureSessionReadiness,
{
    pub(in crate::input::screen) fn new(
        session: S,
        readiness: R,
        reservation: ReservedCaptureSessionAuthority,
    ) -> Self {
        assert_eq!(session.authority(), reservation.authority());
        Self {
            session: Some(session),
            readiness: Some(readiness),
            reservation: Some(reservation),
        }
    }

    pub(in crate::input::screen) fn prepare(
        mut self,
        deadline: CaptureSessionDeadline,
    ) -> anyhow::Result<PreparedCaptureSession<S>> {
        self.readiness
            .take()
            .expect("capture session readiness is consumed exactly once")
            .wait(deadline)?;
        if self
            .session
            .as_ref()
            .is_some_and(CaptureSession::is_finished)
        {
            anyhow::bail!("capture session exited before readiness committed");
        }
        Ok(PreparedCaptureSession {
            session: self.session.take(),
            reservation: self.reservation.take(),
        })
    }
}

impl<S: CaptureSession> PreparedCaptureSession<S> {
    pub(in crate::input::screen) fn authority(&self) -> CaptureSessionAuthority {
        self.session
            .as_ref()
            .expect("prepared capture session remains owned before commit")
            .authority()
    }

    pub(in crate::input::screen) fn commit_into<P, D>(
        mut self,
        sessions: &mut CaptureSessionSet<S>,
        checkpoint: impl FnOnce(&ReservedCaptureSessionAuthority) -> Option<P>,
        commit_authority: impl FnOnce(ReservedCaptureSessionAuthority, P) -> D,
    ) -> Result<CaptureSessionCommit, Self> {
        let candidate = self
            .session
            .as_ref()
            .expect("prepared capture session remains owned before commit");
        if candidate.is_finished() || !sessions.can_prepare_successor() {
            return Err(self);
        }
        let authority = candidate.authority();
        let reservation = self
            .reservation
            .as_ref()
            .expect("prepared capture authority remains owned before commit");
        let Some(checkpoint) = checkpoint(reservation) else {
            return Err(self);
        };
        if S::SUCCESSOR_POLICY == CaptureSuccessorPolicy::AllowOverlap {
            sessions.retire_active_for_successor();
        }
        let session = self
            .session
            .take()
            .expect("prepared capture session commits exactly once");
        sessions
            .install(session)
            .unwrap_or_else(|_| unreachable!("capture successor admission was proven"));
        let reservation = self
            .reservation
            .take()
            .expect("prepared capture authority commits exactly once");
        let displaced = commit_authority(reservation, checkpoint);
        drop(displaced);
        sessions.start_active(authority);
        Ok(CaptureSessionCommit { authority })
    }
}

fn detach_session<S: CaptureSession>(session: S) {
    session.abort();
    session.wake();
    session.detach();
}

impl<S, R> Drop for CaptureSessionTransaction<S, R>
where
    S: CaptureSession,
    R: CaptureSessionReadiness,
{
    fn drop(&mut self) {
        if let Some(session) = self.session.take() {
            detach_session(session);
        }
    }
}

impl<S: CaptureSession> Drop for PreparedCaptureSession<S> {
    fn drop(&mut self) {
        if let Some(session) = self.session.take() {
            detach_session(session);
        }
    }
}

pub(in crate::input::screen) struct CaptureSessionSet<S: CaptureSession> {
    active: Option<S>,
    retiring: Vec<S>,
}

impl<S: CaptureSession> Default for CaptureSessionSet<S> {
    fn default() -> Self {
        Self {
            active: None,
            retiring: Vec::new(),
        }
    }
}

impl<S: CaptureSession> CaptureSessionSet<S> {
    pub(in crate::input::screen) const fn active(&self) -> Option<&S> {
        self.active.as_ref()
    }

    pub(in crate::input::screen) fn exact_endpoint(&self) -> Option<S::ExactEndpoint> {
        self.active.as_ref().map(CaptureSession::exact_endpoint)
    }

    pub(in crate::input::screen) fn can_install_successor(&self) -> bool {
        self.active.is_none()
            && (S::SUCCESSOR_POLICY == CaptureSuccessorPolicy::AllowOverlap
                || self.retiring.is_empty())
    }

    pub(in crate::input::screen) fn can_prepare_successor(&self) -> bool {
        match S::SUCCESSOR_POLICY {
            CaptureSuccessorPolicy::WaitForRetirement => {
                self.active.is_none() && self.retiring.is_empty()
            }
            CaptureSuccessorPolicy::AllowOverlap => true,
        }
    }

    pub(in crate::input::screen) fn install(&mut self, session: S) -> Result<(), S> {
        if !self.can_install_successor() {
            return Err(session);
        }
        self.active = Some(session);
        Ok(())
    }

    fn start_active(&self, authority: CaptureSessionAuthority) {
        let session = self
            .active
            .as_ref()
            .filter(|session| session.authority() == authority)
            .expect("committed capture session remains active before start");
        session.start();
    }

    pub(in crate::input::screen) fn retire_active(&mut self) -> Option<CaptureSessionAuthority> {
        let session = self.active.take()?;
        let authority = session.authority();
        session.abort();
        session.wake();
        self.retiring.push(session);
        Some(authority)
    }

    fn retire_active_for_successor(&mut self) -> Option<CaptureSessionAuthority> {
        let session = self.active.take()?;
        let authority = session.authority();
        session.retire_for_successor();
        session.wake();
        self.retiring.push(session);
        Some(authority)
    }

    pub(in crate::input::screen) fn take_active_for_settlement(&mut self) -> Option<S> {
        self.active.take()
    }

    pub(in crate::input::screen) fn take_finished_active(
        &mut self,
    ) -> Option<(CaptureSessionAuthority, S::Exit)> {
        if !self
            .active
            .as_ref()
            .is_some_and(CaptureSession::is_finished)
        {
            return None;
        }
        let session = self.active.take().expect("finished session remains active");
        let authority = session.authority();
        Some((authority, session.finish()))
    }

    pub(in crate::input::screen) fn reap_finished(
        &mut self,
        mut on_exit: impl FnMut(CaptureSessionAuthority, S::Exit),
    ) {
        let mut index = 0;
        while index < self.retiring.len() {
            if !self.retiring[index].is_finished() {
                index += 1;
                continue;
            }
            let session = self.retiring.remove(index);
            let authority = session.authority();
            on_exit(authority, session.finish());
        }
    }

    #[cfg(test)]
    pub(super) fn retiring_len(&self) -> usize {
        self.retiring.len()
    }

    #[cfg(test)]
    pub(super) fn retiring_capacity(&self) -> usize {
        self.retiring.capacity()
    }
}

impl<S: CaptureSession> Drop for CaptureSessionSet<S> {
    fn drop(&mut self) {
        for session in self
            .active
            .take()
            .into_iter()
            .chain(self.retiring.drain(..))
        {
            session.abort();
            session.wake();
            session.detach();
        }
    }
}
