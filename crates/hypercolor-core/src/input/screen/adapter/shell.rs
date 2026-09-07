use super::{CaptureBackend, ScreenCaptureAdapter};
use crate::input::status::SourceSessionSlot;
use crate::input::{SourceStatusHandle, SourceStatusReporter};

/// The neutral half of every screen capture source.
///
/// Platform sources embed one shell and add only native control on top:
/// the shell owns the adapter, the status reporter and its live session
/// slot, and the running flag, and it implements the status choreography
/// that the `InputSource` and `ScreenSource` impls repeat verbatim.
pub(in crate::input::screen) struct CaptureSourceShell<B: CaptureBackend> {
    pub(in crate::input::screen) adapter: ScreenCaptureAdapter<B>,
    pub(in crate::input::screen) status: SourceStatusReporter,
    pub(in crate::input::screen) status_session: SourceSessionSlot,
    pub(in crate::input::screen) running: bool,
}

impl<B: CaptureBackend> CaptureSourceShell<B> {
    pub(in crate::input::screen) fn new(
        adapter: ScreenCaptureAdapter<B>,
        status: SourceStatusReporter,
        status_session: SourceSessionSlot,
    ) -> Self {
        Self {
            adapter,
            status,
            status_session,
            running: false,
        }
    }

    pub(in crate::input::screen) fn status_handle(&self) -> SourceStatusHandle {
        self.status.handle()
    }

    /// Opens a status session for active capture, when the reporter grants one.
    pub(in crate::input::screen) fn arm_status_session(&mut self) -> anyhow::Result<()> {
        if let Some(session) = self.status.begin_session()? {
            self.status_session.store(session);
        }
        Ok(())
    }

    /// Unwinds a failed start: no session, reporter stopped, worker retired.
    pub(in crate::input::screen) fn fail_start(&mut self) {
        self.status_session.clear();
        self.status.stop();
        self.adapter.shutdown();
    }

    /// Marks the source stopped before the backend tears its worker down.
    pub(in crate::input::screen) fn begin_stop(&mut self) {
        self.status_session.clear();
        self.status.stop();
        self.running = false;
    }

    /// Status-side half of a demand change, applied before the backend acts.
    ///
    /// Policy reflects the requested activity immediately; the session slot
    /// follows the activity edge (cleared on deactivation, opened on
    /// activation while running).
    pub(in crate::input::screen) fn begin_demand_status(
        &mut self,
        was_active: bool,
        active: bool,
    ) -> anyhow::Result<()> {
        self.status.set_policy(true, true, active)?;
        if was_active == active {
            return Ok(());
        }
        if !active {
            self.status_session.clear();
        }
        if active && self.running {
            self.arm_status_session()?;
        }
        Ok(())
    }

    /// Restores the status side after the backend rejected a demand change.
    pub(in crate::input::screen) fn rollback_demand_status(
        &mut self,
        was_active: bool,
    ) -> anyhow::Result<()> {
        self.status_session.clear();
        self.status.stop();
        self.status.set_policy(true, true, was_active)?;
        if was_active && self.running {
            self.arm_status_session()?;
        }
        Ok(())
    }
}
