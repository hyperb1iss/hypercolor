//! Windows Raw Input lifecycle adapter for the shared host-input fold.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

#[cfg(all(target_os = "windows", feature = "windows-capture-fixtures"))]
use std::sync::Mutex;

use hypercolor_types::host_input::{HostInputBatch, HostInputCapabilities};
use hypercolor_windows_input::{
    RawInputConfig, RawInputError, RawInputSession, SessionState, WorkerState,
    interactive_session_state,
};
use tracing::{info, warn};

#[cfg(all(target_os = "windows", feature = "windows-capture-fixtures"))]
use crate::input::host_fold::HostInputPublishOutcome;
use crate::input::host_fold::{HostInputFold, HostInputSink};
use crate::input::traits::{
    InputData, InputSource, InteractionData, InteractionDegradation, InteractionSource,
    InteractionSourceRole, SourceRoleBinding,
};
use crate::input::{
    SourceIssue, SourceKind, SourceSessionSlot, SourceStatusHandle, SourceStatusReporter,
    TerminalFailureLatch,
};
use hypercolor_types::event::TimedInputEvent;

const DEFAULT_EVENT_LIMIT: usize = 256;
const SOURCE_ID: &str = "windows:raw-input";
static NEXT_SESSION_GENERATION: AtomicU64 = AtomicU64::new(1);

pub struct WindowsHostInput {
    name: String,
    running: bool,
    capture_active: bool,
    capture_keyboard: bool,
    capture_pointer: bool,
    fold: HostInputFold,
    direct_sink: HostInputSink,
    session: Option<RawInputSession>,
    session_generation: u64,
    worker_failure: TerminalFailureLatch,
    degraded: Option<InteractionDegradation>,
    status: SourceStatusReporter,
    status_session: SourceSessionSlot,
    #[cfg(all(target_os = "windows", feature = "windows-capture-fixtures"))]
    fixture: Option<Arc<WindowsHostInputFixtureState>>,
}

#[cfg(all(target_os = "windows", feature = "windows-capture-fixtures"))]
struct WindowsHostInputFixtureState {
    sink: Mutex<Option<HostInputSink>>,
}

#[cfg(all(target_os = "windows", feature = "windows-capture-fixtures"))]
#[doc(hidden)]
pub struct WindowsHostInputFixture {
    state: Arc<WindowsHostInputFixtureState>,
    status_session: SourceSessionSlot,
}

#[cfg(all(target_os = "windows", feature = "windows-capture-fixtures"))]
impl WindowsHostInputFixture {
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.state
            .sink
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_some()
    }

    pub fn publish(&self, batch: HostInputBatch<'_>) -> anyhow::Result<bool> {
        let sink = self
            .state
            .sink
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
            .ok_or_else(|| anyhow::anyhow!("deterministic Windows Raw Input source is inactive"))?;
        let published = sink.publish(batch) == HostInputPublishOutcome::Published;
        if published && let Some(status) = self.status_session.load() {
            status.mark_event_driven_live_without_deadline(0);
        }
        Ok(published)
    }
}

impl WindowsHostInput {
    #[must_use]
    pub fn new(capture_keyboard: bool, capture_pointer: bool) -> Self {
        let mut fold = HostInputFold::new(DEFAULT_EVENT_LIMIT);
        let capabilities = HostInputCapabilities {
            keyboard: capture_keyboard,
            pointer: capture_pointer,
        };
        let direct_sink = fold.begin_session(SOURCE_ID, capabilities);
        Self {
            name: "WindowsHostInput".to_owned(),
            running: false,
            capture_active: false,
            capture_keyboard,
            capture_pointer,
            fold,
            direct_sink,
            session: None,
            session_generation: 0,
            worker_failure: TerminalFailureLatch::default(),
            degraded: None,
            status: SourceStatusReporter::new(
                "windows_host_input",
                SourceKind::Interaction,
                "raw_input",
                true,
                true,
                false,
            ),
            status_session: SourceSessionSlot::new(),
            #[cfg(all(target_os = "windows", feature = "windows-capture-fixtures"))]
            fixture: None,
        }
    }

    #[cfg(all(target_os = "windows", feature = "windows-capture-fixtures"))]
    #[doc(hidden)]
    #[must_use]
    pub fn new_deterministic_fixture(
        capture_keyboard: bool,
        capture_pointer: bool,
    ) -> (Self, WindowsHostInputFixture) {
        let mut source = Self::new(capture_keyboard, capture_pointer);
        let state = Arc::new(WindowsHostInputFixtureState {
            sink: Mutex::new(None),
        });
        source.fixture = Some(Arc::clone(&state));
        let fixture = WindowsHostInputFixture {
            state,
            status_session: source.status_session.clone(),
        };
        (source, fixture)
    }

    #[must_use]
    pub fn device_count(&self) -> usize {
        if self.degraded.is_some() {
            0
        } else {
            self.fold.device_count()
        }
    }

    #[must_use]
    pub fn degradation(&self) -> Option<InteractionDegradation> {
        self.degraded.clone()
    }

    #[must_use]
    pub const fn epoch(&self) -> u64 {
        self.session_generation
    }

    pub fn fold_and_snapshot(
        &mut self,
        batch: HostInputBatch<'_>,
    ) -> (InteractionData, Vec<TimedInputEvent>) {
        let _ = self.direct_sink.publish(batch);
        let sample = self.fold.sample_and_drain();
        (sample.interaction, sample.events)
    }

    fn begin_fold_session(&mut self) -> HostInputSink {
        let capabilities = HostInputCapabilities {
            keyboard: self.capture_keyboard,
            pointer: self.capture_pointer,
        };
        self.session_generation = NEXT_SESSION_GENERATION
            .fetch_add(1, Ordering::Relaxed)
            .max(1);
        let sink = self.fold.begin_session(SOURCE_ID, capabilities);
        self.direct_sink = sink.clone();
        sink
    }

    fn start_session(&mut self) {
        if self.session.is_some() {
            return;
        }
        self.worker_failure.reset();
        if interactive_session_state() == SessionState::NoInteractiveSession {
            self.mark_no_interactive_session();
            return;
        }

        let sink = self.begin_fold_session();
        #[cfg(all(target_os = "windows", feature = "windows-capture-fixtures"))]
        if let Some(fixture) = self.fixture.as_ref() {
            *fixture
                .sink
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(sink);
            self.degraded = None;
            if let Some(status) = self.status_session.load() {
                status.mark_event_driven_live_without_deadline(0);
            }
            return;
        }

        let config = RawInputConfig {
            keyboard: self.capture_keyboard,
            mouse: self.capture_pointer,
            clock: Arc::new(crate::input::input_mono_ms),
            session_generation: self.session_generation,
        };
        match RawInputSession::start(config, move |batch| {
            let _ = sink.publish(batch);
        }) {
            Ok(session) => {
                info!(source = %self.name, "Started Windows Raw Input capture");
                self.degraded = None;
                self.session = Some(session);
                if let Some(status) = self.status.session() {
                    status.mark_event_driven_live_without_deadline(self.device_count());
                }
            }
            Err(error) => {
                self.fold.end_session();
                if matches!(&error, RawInputError::NoInteractiveSession) {
                    self.mark_no_interactive_session();
                } else {
                    warn!(source = %self.name, %error, "Windows Raw Input capture unavailable");
                    self.degraded = Some(InteractionDegradation::Unavailable(error.to_string()));
                    if let Some(status) = self.status.session() {
                        status.unavailable(SourceIssue::new(
                            "windows_raw_input_unavailable",
                            error.to_string(),
                            true,
                        ));
                    }
                }
            }
        }
    }

    fn mark_no_interactive_session(&mut self) {
        self.degraded = Some(InteractionDegradation::NoInteractiveSession);
        if let Some(status) = self.status.session() {
            status.unavailable(
                SourceIssue::new(
                    InteractionDegradation::NoInteractiveSession.code(),
                    "Raw Input requires an interactive desktop session",
                    true,
                )
                .with_remediation("run the foreground daemon inside the signed-in desktop session"),
            );
        }
    }

    fn stop_session(&mut self) {
        #[cfg(all(target_os = "windows", feature = "windows-capture-fixtures"))]
        if let Some(fixture) = self.fixture.as_ref() {
            fixture
                .sink
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take();
        }
        if let Some(mut session) = self.session.take() {
            session.stop();
        }
        self.fold.end_session();
        self.worker_failure.reset();
    }

    fn publish_worker_failure_once(&mut self) -> bool {
        if self.worker_failure.is_latched() {
            return true;
        }
        let failure = self.worker_failure.take(|| {
            self.session
                .as_ref()
                .and_then(|session| match session.worker_state() {
                    WorkerState::Running => None,
                    WorkerState::Failed(reason) => Some(reason),
                })
        });
        let Some(reason) = failure else {
            return false;
        };
        self.degraded = Some(InteractionDegradation::Unavailable(reason.clone()));
        if let Some(status) = self.status.session() {
            status.failed(SourceIssue::new(
                "windows_raw_input_worker_failed",
                reason,
                true,
            ));
        }
        true
    }

    fn capture_session_active(&self) -> bool {
        if self.session.is_some() {
            return true;
        }
        #[cfg(all(target_os = "windows", feature = "windows-capture-fixtures"))]
        {
            self.fixture.as_ref().is_some_and(|fixture| {
                fixture
                    .sink
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .is_some()
            })
        }
        #[cfg(not(all(target_os = "windows", feature = "windows-capture-fixtures")))]
        false
    }
}

impl InputSource for WindowsHostInput {
    fn name(&self) -> &str {
        &self.name
    }

    fn start(&mut self) -> anyhow::Result<()> {
        if self.running {
            return Ok(());
        }
        if self.capture_active {
            if let Some(session) = self.status.begin_session()? {
                self.status_session.store(session);
            }
            self.start_session();
        }
        self.running = true;
        Ok(())
    }

    fn stop(&mut self) {
        self.status_session.clear();
        self.status.stop();
        self.stop_session();
        self.running = false;
    }

    fn sample(&mut self) -> anyhow::Result<InputData> {
        if self.publish_worker_failure_once() || !self.running || !self.capture_session_active() {
            return Ok(InputData::None);
        }
        if let Some(status) = self.status_session.load() {
            status.mark_event_driven_live_without_deadline(self.device_count());
        }
        Ok(InputData::Interaction(self.fold.sample()))
    }

    fn sample_and_drain_with_delta_secs(
        &mut self,
        _delta_secs: f32,
    ) -> (anyhow::Result<InputData>, Vec<TimedInputEvent>) {
        if self.publish_worker_failure_once() || !self.running || !self.capture_session_active() {
            return (Ok(InputData::None), Vec::new());
        }
        let sample = self.fold.sample_and_drain();
        if let Some(status) = self.status_session.load() {
            status.mark_event_driven_live_without_deadline(self.device_count());
        }
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
        if self.publish_worker_failure_once() {
            Vec::new()
        } else {
            self.fold.drain_events()
        }
    }
}

impl SourceRoleBinding for WindowsHostInput {
    type Role = InteractionSourceRole;
}

impl InteractionSource for WindowsHostInput {
    fn interaction_diagnostics(&self) -> Option<crate::input::InteractionDiagnostics> {
        let worker_failure =
            self.session
                .as_ref()
                .and_then(|session| match session.worker_state() {
                    WorkerState::Running => None,
                    WorkerState::Failed(reason) => {
                        Some(InteractionDegradation::Unavailable(reason))
                    }
                });
        Some(crate::input::InteractionDiagnostics {
            backend: "raw_input",
            host_capture: true,
            capturing: self.capture_active
                && self.capture_session_active()
                && worker_failure.is_none(),
            devices_opened: self.device_count(),
            devices_denied: 0,
            degraded: worker_failure.or_else(|| self.degraded.clone()),
        })
    }

    fn set_interaction_capture_active(&mut self, active: bool) -> anyhow::Result<()> {
        self.status.set_policy(true, true, active)?;
        if self.capture_active == active {
            return Ok(());
        }
        self.capture_active = active;
        if !self.running {
            return Ok(());
        }
        if active {
            if let Some(session) = self.status.begin_session()? {
                self.status_session.store(session);
            }
            self.start_session();
        } else {
            self.status_session.clear();
            self.stop_session();
        }
        Ok(())
    }
}
