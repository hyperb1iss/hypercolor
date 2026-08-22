use super::{CaptureExactCommandEndpoint, CaptureSessionAuthority};

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
    fn wake(&self);
    fn is_finished(&self) -> bool;
    fn finish(self) -> Self::Exit;
    fn detach(self);
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

    pub(in crate::input::screen) fn install(&mut self, session: S) -> Result<(), S> {
        if !self.can_install_successor() {
            return Err(session);
        }
        self.active = Some(session);
        Ok(())
    }

    pub(in crate::input::screen) fn retire_active(&mut self) -> Option<CaptureSessionAuthority> {
        let session = self.active.take()?;
        let authority = session.authority();
        session.abort();
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
