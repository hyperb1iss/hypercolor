//! Linux host-input adapter over the platform acquisition crate.

use std::sync::Arc;

pub use hypercolor_linux_input::{DeviceOpenState, DeviceOpenStatus};
use hypercolor_linux_input::{
    EvdevInputConfig, EvdevInputSession, EvdevWorkerState, start_evdev_input,
};
use hypercolor_types::host_input::HostInputCapabilities;

use crate::input::traits::{
    InputData, InputSource, InteractionSource, InteractionSourceRole, SourceRoleBinding,
};
use crate::input::{
    HostInputFold, SourceIssue, SourceKind, SourceStatusHandle, SourceStatusReporter,
    classify_source_resource_scan, input_mono_ms,
};
use hypercolor_types::event::TimedInputEvent;

const BACKEND: &str = "evdev";

/// Core lifecycle and status adapter for Linux evdev acquisition.
pub struct EvdevHostInput {
    name: String,
    running: bool,
    capture_active: bool,
    capture_keyboard: bool,
    capture_pointer: bool,
    session_generation: u64,
    observed_status_revision: u64,
    worker_failure_reported: bool,
    fold: HostInputFold,
    session: Option<EvdevInputSession>,
    status: SourceStatusReporter,
}

impl EvdevHostInput {
    /// Create a host-input source for the enabled device kinds.
    #[must_use]
    pub fn new(capture_keyboard: bool, capture_pointer: bool) -> Self {
        Self {
            name: "EvdevHostInput".to_owned(),
            running: false,
            capture_active: false,
            capture_keyboard,
            capture_pointer,
            session_generation: 0,
            observed_status_revision: u64::MAX,
            worker_failure_reported: false,
            fold: HostInputFold::default(),
            session: None,
            status: SourceStatusReporter::new(
                "evdev_host_input",
                SourceKind::Interaction,
                BACKEND,
                true,
                true,
                false,
            ),
        }
    }

    /// Snapshot the latest per-node discovery results.
    #[must_use]
    pub fn device_status(&self) -> Vec<DeviceOpenStatus> {
        self.session
            .as_ref()
            .map_or_else(Vec::new, EvdevInputSession::device_status)
    }

    fn start_session(&mut self) -> anyhow::Result<()> {
        self.session_generation = self.session_generation.wrapping_add(1).max(1);
        self.worker_failure_reported = false;
        self.observed_status_revision = u64::MAX;
        let sink = self.fold.begin_session(
            "linux:evdev",
            HostInputCapabilities {
                keyboard: self.capture_keyboard,
                pointer: self.capture_pointer,
            },
        );
        let config = EvdevInputConfig {
            keyboard: self.capture_keyboard,
            pointer: self.capture_pointer,
            session_generation: self.session_generation,
            clock: Arc::new(input_mono_ms),
        };
        match start_evdev_input(config, move |batch| {
            let _ = sink.publish(batch);
        }) {
            Ok(session) => {
                self.session = Some(session);
                self.publish_resource_health_if_changed();
                Ok(())
            }
            Err(error) => {
                self.fold.end_session();
                Err(error.into())
            }
        }
    }

    fn stop_session(&mut self) {
        if let Some(mut session) = self.session.take() {
            session.stop();
        }
        self.fold.end_session();
        self.worker_failure_reported = false;
    }

    fn publish_resource_health_if_changed(&mut self) {
        let Some(session) = self.session.as_ref() else {
            return;
        };
        let revision = session.device_status_revision();
        if revision == self.observed_status_revision {
            return;
        }
        self.observed_status_revision = revision;
        let statuses = session.device_status();
        let opened = statuses
            .iter()
            .filter(|entry| entry.state == DeviceOpenState::Opened)
            .count();
        let denied = statuses
            .iter()
            .filter(|entry| entry.state == DeviceOpenState::PermissionDenied)
            .count();
        let failed = statuses
            .iter()
            .filter(|entry| matches!(&entry.state, DeviceOpenState::Failed(_)))
            .count();
        if let Some(status) = self.status.session() {
            status.publish_resource_scan_health(classify_source_resource_scan(
                opened, denied, failed,
            ));
        }
    }

    fn observe_worker(&mut self) -> bool {
        let Some(session) = self.session.as_ref() else {
            return false;
        };
        match session.worker_state() {
            EvdevWorkerState::Running => {
                self.publish_resource_health_if_changed();
                true
            }
            EvdevWorkerState::Failed(detail) => {
                if !self.worker_failure_reported {
                    if let Some(status) = self.status.session() {
                        status.failed(SourceIssue::new("evdev_worker_exited", detail, true));
                    }
                    self.fold.end_session();
                    self.worker_failure_reported = true;
                }
                false
            }
        }
    }
}

impl InputSource for EvdevHostInput {
    fn name(&self) -> &str {
        &self.name
    }

    fn start(&mut self) -> anyhow::Result<()> {
        if self.running {
            return Ok(());
        }
        if self.capture_active {
            self.status.begin_session()?;
            if let Err(error) = self.start_session() {
                self.status.stop();
                return Err(error);
            }
        }
        self.running = true;
        Ok(())
    }

    fn stop(&mut self) {
        self.status.stop();
        self.stop_session();
        self.running = false;
    }

    fn sample(&mut self) -> anyhow::Result<InputData> {
        if !self.running || !self.observe_worker() {
            return Ok(InputData::None);
        }
        Ok(InputData::Interaction(self.fold.sample()))
    }

    fn sample_and_drain_with_delta_secs(
        &mut self,
        _delta_secs: f32,
    ) -> (anyhow::Result<InputData>, Vec<TimedInputEvent>) {
        if !self.running || !self.observe_worker() {
            return (Ok(InputData::None), Vec::new());
        }
        let sample = self.fold.sample_and_drain();
        (
            Ok(InputData::Interaction(sample.interaction)),
            sample.events,
        )
    }

    fn is_running(&self) -> bool {
        self.running
    }

    fn source_status_handle(&self) -> Option<SourceStatusHandle> {
        Some(self.status.handle())
    }

    fn source_status_reporter(&mut self) -> Option<&mut SourceStatusReporter> {
        Some(&mut self.status)
    }

    fn drain_events(&mut self) -> Vec<TimedInputEvent> {
        if !self.running || !self.observe_worker() {
            return Vec::new();
        }
        self.fold.drain_events()
    }
}

impl SourceRoleBinding for EvdevHostInput {
    type Role = InteractionSourceRole;
}

impl InteractionSource for EvdevHostInput {
    fn interaction_diagnostics(&self) -> Option<crate::input::InteractionDiagnostics> {
        let statuses = self.device_status();
        let devices_opened = statuses
            .iter()
            .filter(|entry| entry.state == DeviceOpenState::Opened)
            .count();
        let devices_denied = statuses
            .iter()
            .filter(|entry| entry.state == DeviceOpenState::PermissionDenied)
            .count();
        Some(crate::input::InteractionDiagnostics {
            backend: BACKEND,
            host_capture: true,
            capturing: self.capture_active
                && self.session.is_some()
                && !self.worker_failure_reported,
            devices_opened,
            devices_denied,
            degraded: (devices_denied > 0 && devices_opened == 0)
                .then_some(crate::input::InteractionDegradation::AccessDenied),
        })
    }

    fn set_interaction_capture_active(&mut self, active: bool) -> anyhow::Result<()> {
        let previous = self.capture_active;
        self.status.set_policy(true, true, active)?;
        if previous == active {
            return Ok(());
        }
        if !self.running {
            self.capture_active = active;
            return Ok(());
        }
        if active {
            self.status.begin_session()?;
            if let Err(error) = self.start_session() {
                self.status.stop();
                self.status.set_policy(true, true, previous)?;
                return Err(error);
            }
        } else {
            self.status.stop();
            self.stop_session();
        }
        self.capture_active = active;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inactive_source_is_target_independent() {
        let mut source = EvdevHostInput::new(true, true);
        source
            .start()
            .expect("inactive source starts without native I/O");
        assert!(source.is_running());
        assert!(matches!(source.sample(), Ok(InputData::None)));
        source.stop();
        assert!(!source.is_running());
    }
}
