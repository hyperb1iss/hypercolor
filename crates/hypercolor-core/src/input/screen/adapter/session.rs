use std::time::{Duration, Instant};

use super::{
    CaptureExactCommandEndpoint, CaptureSessionAuthority, ReservedCaptureSessionAuthority,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(not(any(target_os = "windows", test)), allow(dead_code))]
pub(in crate::input::screen) enum CaptureSuccessorPolicy {
    WaitForRetirement,
    AllowOverlap,
}

pub(in crate::input::screen) trait CaptureSession: Sized {
    type Exit;
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
#[cfg_attr(
    not(any(target_os = "linux", target_os = "windows", test)),
    allow(dead_code)
)]
pub(in crate::input::screen) struct CaptureSessionDeadline(Instant);

impl CaptureSessionDeadline {
    pub(in crate::input::screen) fn after(timeout: Duration) -> Self {
        Self(Instant::now() + timeout)
    }

    #[cfg(any(target_os = "linux", target_os = "windows", test))]
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

    #[cfg(target_os = "windows")]
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
