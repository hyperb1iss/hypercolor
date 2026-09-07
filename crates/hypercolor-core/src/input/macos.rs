//! macOS host input folded from Core Graphics event-tap batches.

use std::sync::Arc;
#[cfg(feature = "macos-native-fixtures")]
use std::sync::Mutex;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::time::Instant;

#[cfg(feature = "macos-native-fixtures")]
use hypercolor_macos_input::MacosVirtualDesktop;
use hypercolor_macos_input::{
    MacosArchitecture, MacosAuthorizationState, MacosCapabilityOwner, MacosDaemonOwnerConflict,
    MacosInputConfig, MacosInputError, MacosInputGapReason, MacosInputPublicationOutcome,
    MacosInputSession, MacosInputStatusSnapshot, MacosProtectedSourceState, MacosWorkerDegradation,
    MacosWorkerState, input_diagnostics_envelope, input_monitoring_granted,
    request_input_monitoring,
};
use hypercolor_types::host_input::{HostInputBatch, HostInputCapabilities};
#[cfg(feature = "macos-native-fixtures")]
use hypercolor_types::host_input::{HostInputEvent, HostPointerSnapshot};
use tracing::{info, warn};

use crate::input::traits::{
    CapabilityActionDisposition, CapabilityActionIdentity, InputData, InputSource, InteractionData,
    InteractionDegradation, InteractionSource, InteractionSourceRole,
    ProtectedSourceAuthorizationAction, SourceCapabilityContext, SourceRoleBinding,
};
use crate::input::{
    HostInputFold, HostInputFoldDiagnostics, HostInputPublishOutcome, HostInputSink, SourceIssue,
    SourceKind, SourceSessionSlot, SourceStatusHandle, SourceStatusReporter,
};
use hypercolor_types::event::TimedInputEvent;

const SOURCE_ID: &str = "host:macos";
const DEFAULT_EVENT_LIMIT: usize = crate::input::InteractionBatch::MAX_EVENTS;
const AUTHORIZATION_NONE: u8 = 0;
const AUTHORIZATION_GRANTED: u8 = 1;
const AUTHORIZATION_DENIED: u8 = 2;

fn native_process_architecture() -> (Option<MacosArchitecture>, MacosArchitecture, Option<bool>) {
    let executable = if cfg!(target_arch = "aarch64") {
        MacosArchitecture::AppleSilicon
    } else {
        MacosArchitecture::Intel
    };
    // The capture crate compiles everywhere; off-platform the probe reports
    // ScreenCaptureKit as unavailable, which collapses to "host unknown".
    let capabilities = hypercolor_macos_capture::MacosScreenCaptureSession::capabilities().ok();
    let host = capabilities.map(|capabilities| match capabilities.host_architecture {
        hypercolor_macos_capture::MacosHostArchitecture::AppleSilicon => {
            MacosArchitecture::AppleSilicon
        }
        hypercolor_macos_capture::MacosHostArchitecture::Intel => MacosArchitecture::Intel,
    });
    let translated = capabilities.map(|capabilities| capabilities.translated_process);
    (host, executable, translated)
}

pub type MacosInputFoldDiagnostics = HostInputFoldDiagnostics;

static NEXT_EPOCH: AtomicU64 = AtomicU64::new(1);

pub struct MacosHostInput {
    name: String,
    running: bool,
    capture_active: bool,
    capture_keyboard: bool,
    capture_pointer: bool,
    session_generation: u64,
    topology_generation: Arc<AtomicU64>,
    fold: HostInputFold,
    sink: Option<HostInputSink>,
    session: Option<MacosInputSession>,
    degraded: Option<InteractionDegradation>,
    status: SourceStatusReporter,
    status_session: SourceSessionSlot,
    keyboard_tcc: MacosAuthorizationState,
    authorization_last_transition_at: Option<Instant>,
    owner: MacosCapabilityOwner,
    owner_conflict: Option<Arc<MacosDaemonOwnerConflict>>,
    owner_designated_requirement_hash: Option<Arc<str>>,
    host_architecture: Option<MacosArchitecture>,
    executable_architecture: MacosArchitecture,
    translated_process: Option<bool>,
    authorization_result: Arc<AtomicU8>,
    #[cfg(feature = "macos-native-fixtures")]
    fixture: Option<Arc<FixtureState>>,
}

#[cfg(feature = "macos-native-fixtures")]
#[derive(Debug, Clone, PartialEq)]
pub struct MacosInputFixtureBackend {
    pub preflight_granted: bool,
    pub request_granted: bool,
    pub effective_masks: hypercolor_macos_input::EffectiveEventMasks,
    pub owner_restart_succeeds: bool,
    pub virtual_desktop: MacosVirtualDesktop,
}

#[cfg(feature = "macos-native-fixtures")]
impl MacosInputFixtureBackend {
    #[must_use]
    pub fn new(
        preflight_granted: bool,
        request_granted: bool,
        effective_masks: hypercolor_macos_input::EffectiveEventMasks,
        owner_restart_succeeds: bool,
        virtual_desktop: MacosVirtualDesktop,
    ) -> Self {
        Self {
            preflight_granted,
            request_granted,
            effective_masks,
            owner_restart_succeeds,
            virtual_desktop,
        }
    }
}

#[cfg(feature = "macos-native-fixtures")]
struct FixtureState {
    backend: Mutex<MacosInputFixtureBackend>,
    active_session: Mutex<Option<FixtureSession>>,
    topology_generation: Arc<AtomicU64>,
}

#[cfg(feature = "macos-native-fixtures")]
struct FixtureSession {
    generation: u64,
    sink: HostInputSink,
}

#[cfg(feature = "macos-native-fixtures")]
pub struct MacosHostInputFixture {
    state: Arc<FixtureState>,
}

#[cfg(feature = "macos-native-fixtures")]
impl MacosHostInputFixture {
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.state
            .active_session
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_some()
    }

    #[must_use]
    pub fn effective_masks(&self) -> hypercolor_macos_input::EffectiveEventMasks {
        self.state
            .backend
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .effective_masks
    }

    #[must_use]
    pub fn active_epoch(&self) -> Option<u64> {
        self.state
            .active_session
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .map(|session| session.generation)
    }

    pub fn publish(&self, events: &[HostInputEvent], at_ms: u64) -> anyhow::Result<bool> {
        let epoch = self
            .active_epoch()
            .ok_or_else(|| anyhow::anyhow!("deterministic macOS input source is inactive"))?;
        self.publish_with_epoch(epoch, events, at_ms)
    }

    pub fn publish_with_epoch(
        &self,
        epoch: u64,
        events: &[HostInputEvent],
        at_ms: u64,
    ) -> anyhow::Result<bool> {
        self.publish_batch_with_epoch(epoch, events, None, at_ms)
    }

    pub fn publish_batch_with_epoch(
        &self,
        epoch: u64,
        events: &[HostInputEvent],
        pointer: Option<HostPointerSnapshot>,
        at_ms: u64,
    ) -> anyhow::Result<bool> {
        let active = self
            .state
            .active_session
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(session) = active.as_ref() else {
            return Ok(false);
        };
        if session.generation != epoch {
            return Ok(false);
        }
        if let Some(pointer) = pointer {
            self.state
                .topology_generation
                .store(pointer.coordinate_space_generation, Ordering::Relaxed);
        }
        Ok(session.sink.publish(HostInputBatch {
            events,
            pointer,
            at_ms,
            device_catalog_generation: 0,
        }) == HostInputPublishOutcome::Published)
    }

    pub fn request_input_monitoring_and_restart_owner(&self) -> anyhow::Result<bool> {
        let mut backend = self
            .state
            .backend
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !backend.request_granted {
            return Ok(false);
        }
        if !backend.owner_restart_succeeds {
            anyhow::bail!("deterministic macOS input owner restart failed");
        }
        backend.preflight_granted = true;
        Ok(true)
    }
}

impl MacosHostInput {
    #[must_use]
    pub fn new(capture_keyboard: bool, capture_pointer: bool) -> Self {
        let keyboard_tcc = if !capture_keyboard {
            MacosAuthorizationState::Unknown
        } else if input_monitoring_granted() {
            MacosAuthorizationState::Authorized
        } else {
            MacosAuthorizationState::NotDetermined
        };
        let (host_architecture, executable_architecture, translated_process) =
            native_process_architecture();
        let mut source = Self {
            name: "MacosHostInput".to_owned(),
            running: false,
            capture_active: false,
            capture_keyboard,
            capture_pointer,
            session_generation: 0,
            topology_generation: Arc::new(AtomicU64::new(0)),
            fold: HostInputFold::new(DEFAULT_EVENT_LIMIT),
            sink: None,
            session: None,
            degraded: None,
            status: SourceStatusReporter::new(
                "macos_host_input",
                SourceKind::Interaction,
                "cg_event_tap",
                true,
                true,
                false,
            ),
            status_session: SourceSessionSlot::new(),
            keyboard_tcc,
            authorization_last_transition_at: None,
            owner: MacosCapabilityOwner::Standalone,
            owner_conflict: None,
            owner_designated_requirement_hash: None,
            host_architecture,
            executable_architecture,
            translated_process,
            authorization_result: Arc::new(AtomicU8::new(AUTHORIZATION_NONE)),
            #[cfg(feature = "macos-native-fixtures")]
            fixture: None,
        };
        source
            .refresh_platform_status()
            .expect("new macOS input status is not retired");
        source
    }

    #[cfg(feature = "macos-native-fixtures")]
    #[must_use]
    pub fn new_deterministic_fixture(
        capture_keyboard: bool,
        capture_pointer: bool,
        backend: MacosInputFixtureBackend,
    ) -> (Self, MacosHostInputFixture) {
        let mut source = Self::new(capture_keyboard, capture_pointer);
        let preflight_granted = backend.preflight_granted;
        let state = Arc::new(FixtureState {
            backend: Mutex::new(backend),
            active_session: Mutex::new(None),
            topology_generation: Arc::clone(&source.topology_generation),
        });
        source.fixture = Some(Arc::clone(&state));
        source.keyboard_tcc = if capture_keyboard && preflight_granted {
            MacosAuthorizationState::Authorized
        } else if capture_keyboard {
            MacosAuthorizationState::NotDetermined
        } else {
            MacosAuthorizationState::Unknown
        };
        source
            .refresh_platform_status()
            .expect("fixture macOS input status is not retired");
        let fixture = MacosHostInputFixture { state };
        (source, fixture)
    }

    #[must_use]
    pub fn epoch(&self) -> u64 {
        self.session_generation
    }

    #[must_use]
    pub fn degradation(&self) -> Option<InteractionDegradation> {
        self.degraded.clone()
    }

    #[must_use]
    pub const fn capture_kinds(&self) -> (bool, bool) {
        (self.capture_keyboard, self.capture_pointer)
    }

    pub fn set_capability_owner(&mut self, owner: MacosCapabilityOwner) -> anyhow::Result<()> {
        self.owner = owner;
        self.refresh_platform_status()
    }

    fn set_daemon_ownership(
        &mut self,
        owner: MacosCapabilityOwner,
        conflict: Option<MacosDaemonOwnerConflict>,
        designated_requirement_hash: Option<Arc<str>>,
    ) -> anyhow::Result<()> {
        self.owner = owner;
        self.owner_conflict = conflict.map(Arc::new);
        self.owner_designated_requirement_hash = designated_requirement_hash;
        self.refresh_platform_status()
    }

    #[must_use]
    pub fn fold_diagnostics(&self) -> MacosInputFoldDiagnostics {
        self.fold.diagnostics()
    }

    pub fn fold_and_snapshot(
        &mut self,
        batch: HostInputBatch<'_>,
    ) -> (InteractionData, Vec<TimedInputEvent>) {
        if self.sink.is_none() {
            let _ = self.begin_fold_session(self.capture_keyboard, self.capture_pointer);
        }
        if let Some(pointer) = batch.pointer {
            self.topology_generation
                .store(pointer.coordinate_space_generation, Ordering::Relaxed);
        }
        if let Some(sink) = &self.sink {
            let _ = sink.publish(batch);
        }
        let sample = self.fold.sample_and_drain();
        (sample.interaction, sample.events)
    }

    fn begin_fold_session(&mut self, keyboard: bool, pointer: bool) -> HostInputSink {
        self.session_generation = NEXT_EPOCH.fetch_add(1, Ordering::Relaxed);
        self.topology_generation.store(0, Ordering::Relaxed);
        let sink = self
            .fold
            .begin_session(SOURCE_ID, HostInputCapabilities { keyboard, pointer });
        self.sink = Some(sink.clone());
        sink
    }

    fn end_fold_session(&mut self) {
        self.session_generation = NEXT_EPOCH.fetch_add(1, Ordering::Relaxed);
        self.topology_generation.store(0, Ordering::Relaxed);
        self.sink = None;
        self.fold.end_session();
    }

    fn permission_granted(&self) -> bool {
        #[cfg(feature = "macos-native-fixtures")]
        if let Some(fixture) = &self.fixture {
            return fixture
                .backend
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .preflight_granted;
        }
        self.keyboard_tcc == MacosAuthorizationState::Authorized
    }

    fn effective_kinds(&self) -> (bool, bool) {
        if let Some(session) = &self.session {
            let masks = session.effective_masks();
            return (masks.keyboard != 0, masks.pointer != 0);
        }
        #[cfg(feature = "macos-native-fixtures")]
        if let Some(fixture) = &self.fixture
            && self.fixture_session_active()
        {
            let masks = fixture
                .backend
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .effective_masks;
            return (
                self.capture_keyboard && masks.keyboard != 0,
                self.capture_pointer && masks.pointer != 0,
            );
        }
        (false, false)
    }

    fn refresh_platform_status(&mut self) -> anyhow::Result<()> {
        let (keyboard_live, pointer_live) = self.effective_kinds();
        let interrupted = matches!(self.degraded, Some(InteractionDegradation::Unavailable(_)));
        let revoked =
            self.degraded == Some(InteractionDegradation::InputMonitoringPermissionRevoked);
        let keyboard = if !self.capture_keyboard {
            MacosProtectedSourceState::Disabled
        } else if revoked {
            MacosProtectedSourceState::Revoked
        } else {
            match self.keyboard_tcc {
                MacosAuthorizationState::Unknown | MacosAuthorizationState::NotDetermined => {
                    MacosProtectedSourceState::NeedsUserAction
                }
                MacosAuthorizationState::Denied => MacosProtectedSourceState::PermissionDenied,
                MacosAuthorizationState::Authorized if !self.capture_active => {
                    MacosProtectedSourceState::ReadyIdle
                }
                MacosAuthorizationState::Authorized if keyboard_live && interrupted => {
                    MacosProtectedSourceState::Interrupted
                }
                MacosAuthorizationState::Authorized if keyboard_live => {
                    MacosProtectedSourceState::Live
                }
                MacosAuthorizationState::Authorized => {
                    MacosProtectedSourceState::NeedsProcessRestart
                }
            }
        };
        let pointer = if !self.capture_pointer {
            MacosProtectedSourceState::Disabled
        } else if !self.capture_active {
            MacosProtectedSourceState::ReadyIdle
        } else if pointer_live && interrupted {
            MacosProtectedSourceState::Interrupted
        } else if pointer_live {
            MacosProtectedSourceState::Live
        } else {
            MacosProtectedSourceState::Failed
        };
        let native = self.session.as_ref().map(MacosInputSession::diagnostics);
        let capture_session_generation = self
            .capture_session_active()
            .then_some(self.session_generation);
        let topology_generation = match self.topology_generation.load(Ordering::Relaxed) {
            0 => None,
            generation => Some(generation),
        };
        let folded_state_gaps = self.fold.diagnostics().state_gaps;
        // Read from the session's health-tick cache: probing Carbon here would
        // put an FFI call on the render thread at frame rate.
        let secure_input_active = native.is_some_and(|diagnostics| diagnostics.secure_input_active);
        let diagnostics = MacosInputStatusSnapshot {
            keyboard,
            pointer,
            keyboard_tcc: self.keyboard_tcc,
            secure_input_active,
            keyboard_owner: self.owner,
            pointer_owner: self.owner,
            owner_conflict: self.owner_conflict.as_deref().cloned(),
            authorization_last_transition_age_ms: self.authorization_last_transition_at.map(
                |transition| u64::try_from(transition.elapsed().as_millis()).unwrap_or(u64::MAX),
            ),
            owner_designated_requirement_hash: self.owner_designated_requirement_hash.clone(),
            host_architecture: self.host_architecture,
            executable_architecture: self.executable_architecture,
            translated_process: self.translated_process,
            capture_session_generation,
            topology_generation,
            native_diagnostics: native,
            folded_state_gaps,
        };
        self.status
            .set_action_issue(protected_input_action_issue(keyboard))?;
        let diagnostics = input_diagnostics_envelope(&diagnostics)
            .inspect_err(|error| tracing::warn!(%error, "dropping invalid macOS input diagnostics"))
            .ok();
        self.status.set_diagnostics(diagnostics)?;
        Ok(())
    }

    fn set_keyboard_tcc(&mut self, state: MacosAuthorizationState) {
        if self.keyboard_tcc != state {
            self.keyboard_tcc = state;
            self.authorization_last_transition_at = Some(Instant::now());
        }
    }

    fn apply_pending_authorization(&mut self) -> anyhow::Result<()> {
        match self
            .authorization_result
            .swap(AUTHORIZATION_NONE, Ordering::AcqRel)
        {
            AUTHORIZATION_NONE => return Ok(()),
            AUTHORIZATION_GRANTED => {
                self.set_keyboard_tcc(MacosAuthorizationState::Authorized);
                if matches!(
                    self.degraded,
                    Some(InteractionDegradation::InputMonitoringPermissionDenied)
                ) {
                    self.degraded = None;
                }
                if self.running && self.capture_active {
                    self.stop_session();
                    self.start_session();
                }
            }
            AUTHORIZATION_DENIED => {
                self.set_keyboard_tcc(MacosAuthorizationState::Denied);
                if self.capture_active {
                    self.degraded = Some(InteractionDegradation::InputMonitoringPermissionDenied);
                }
            }
            _ => unreachable!("macOS authorization result is bounded"),
        }
        self.refresh_platform_status()
    }

    fn active_kind_count(&self) -> usize {
        if let Some(session) = &self.session {
            let masks = session.effective_masks();
            return usize::from(masks.keyboard != 0) + usize::from(masks.pointer != 0);
        }
        #[cfg(feature = "macos-native-fixtures")]
        if let Some(fixture) = &self.fixture
            && self.fixture_session_active()
        {
            let masks = fixture
                .backend
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .effective_masks;
            return usize::from(self.capture_keyboard && masks.keyboard != 0)
                + usize::from(self.capture_pointer && masks.pointer != 0);
        }
        0
    }

    fn start_session(&mut self) {
        if self.session.is_some() || self.fixture_session_active() {
            return;
        }
        self.degraded = None;
        let keyboard_granted = !self.capture_keyboard || self.permission_granted();
        let requested_keyboard = self.capture_keyboard && keyboard_granted;
        let requested_pointer = self.capture_pointer;
        #[cfg(feature = "macos-native-fixtures")]
        let (effective_keyboard, effective_pointer) =
            self.fixture
                .as_ref()
                .map_or((requested_keyboard, requested_pointer), |fixture| {
                    let masks = fixture
                        .backend
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .effective_masks;
                    (
                        requested_keyboard && masks.keyboard != 0,
                        requested_pointer && masks.pointer != 0,
                    )
                });
        #[cfg(not(feature = "macos-native-fixtures"))]
        let (effective_keyboard, effective_pointer) = (requested_keyboard, requested_pointer);
        if !keyboard_granted {
            self.degraded = Some(InteractionDegradation::InputMonitoringPermissionDenied);
        }

        #[cfg(feature = "macos-native-fixtures")]
        if let Some(fixture) = self.fixture.clone() {
            if keyboard_granted
                && ((self.capture_keyboard && !effective_keyboard)
                    || (self.capture_pointer && !effective_pointer))
            {
                self.degraded = Some(InteractionDegradation::Unavailable(
                    "macOS event tap did not activate every requested input kind".to_owned(),
                ));
            }
            if effective_keyboard || effective_pointer {
                let sink = self.begin_fold_session(effective_keyboard, effective_pointer);
                let generation = self.session_generation;
                *fixture
                    .active_session
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) =
                    Some(FixtureSession { generation, sink });
            }
            self.publish_started_status(effective_keyboard, effective_pointer);
            return;
        }

        if !effective_keyboard && !effective_pointer {
            self.publish_started_status(false, false);
            return;
        }

        let sink = self.begin_fold_session(effective_keyboard, effective_pointer);
        let topology_generation = Arc::clone(&self.topology_generation);
        let config = MacosInputConfig {
            keyboard: effective_keyboard,
            pointer: effective_pointer,
            clock: Arc::new(crate::input::input_mono_ms),
        };
        match MacosInputSession::start(config, move |batch| {
            if let Some(pointer) = batch.pointer {
                topology_generation.store(pointer.coordinate_space_generation, Ordering::Relaxed);
            }
            if sink.publish(batch) == HostInputPublishOutcome::Published {
                MacosInputPublicationOutcome::Published
            } else {
                MacosInputPublicationOutcome::Rejected
            }
        }) {
            Ok(session) => {
                info!(
                    source = %self.name,
                    keyboard = effective_keyboard,
                    pointer = effective_pointer,
                    "Started macOS event-tap input capture"
                );
                self.session = Some(session);
                self.publish_started_status(effective_keyboard, effective_pointer);
            }
            Err(error) => {
                self.end_fold_session();
                self.degraded = Some(classify_start_error(&error));
                warn!(source = %self.name, %error, "macOS event-tap input capture unavailable");
                if let Some(status) = self.status.session() {
                    status.unavailable(issue_for_error(&error));
                }
            }
        }
    }

    fn publish_started_status(&self, keyboard: bool, pointer: bool) {
        let Some(status) = self.status.session() else {
            return;
        };
        let resources = usize::from(keyboard) + usize::from(pointer);
        let missing_keyboard = self.capture_keyboard && !keyboard;
        let missing_pointer = self.capture_pointer && !pointer;
        if missing_keyboard || missing_pointer {
            let issue = if missing_keyboard && !self.permission_granted() {
                permission_issue()
            } else {
                event_mask_issue()
            };
            if resources == 0 {
                status.unavailable(issue);
            } else {
                status.degraded_with_resources(issue, resources);
            }
        } else {
            status.mark_event_driven_live_without_deadline(resources);
        }
    }

    fn stop_session(&mut self) {
        #[cfg(feature = "macos-native-fixtures")]
        if let Some(fixture) = &self.fixture {
            *fixture
                .active_session
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
        }
        if let Some(mut session) = self.session.take() {
            session.stop();
        }
        self.end_fold_session();
    }

    fn fixture_session_active(&self) -> bool {
        #[cfg(feature = "macos-native-fixtures")]
        {
            self.fixture.as_ref().is_some_and(|fixture| {
                fixture
                    .active_session
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .is_some()
            })
        }
        #[cfg(not(feature = "macos-native-fixtures"))]
        {
            false
        }
    }

    fn capture_session_active(&self) -> bool {
        self.session.is_some() || self.fixture_session_active()
    }

    fn refresh_worker_health(&mut self) {
        let Some(session) = &self.session else {
            return;
        };
        let state = session.worker_state();
        match state {
            MacosWorkerState::Running => {}
            MacosWorkerState::Degraded(reason) => {
                self.degraded = Some(InteractionDegradation::Unavailable(reason.to_string()));
                if let Some(status) = self.status.session() {
                    status.degraded_with_resources(
                        worker_degradation_issue(&reason),
                        self.active_kind_count(),
                    );
                }
            }
            MacosWorkerState::PermissionRevoked => {
                self.set_keyboard_tcc(MacosAuthorizationState::Denied);
                self.degraded = Some(InteractionDegradation::InputMonitoringPermissionRevoked);
                if let Some(status) = self.status.session() {
                    status.unavailable(permission_revoked_issue());
                }
            }
            MacosWorkerState::Failed(reason) => {
                self.degraded = Some(InteractionDegradation::Unavailable(reason.clone()));
                if let Some(status) = self.status.session() {
                    status.failed(SourceIssue::new(
                        "macos_input_run_loop_exited",
                        reason,
                        true,
                    ));
                }
            }
        }
    }
}

fn protected_input_action_issue(state: MacosProtectedSourceState) -> Option<SourceIssue> {
    match state {
        MacosProtectedSourceState::NeedsUserAction => Some(
            SourceIssue::new(
                "authorization_required",
                "Input Monitoring authorization is required",
                true,
            )
            .with_remediation("Authorize Input Monitoring"),
        ),
        MacosProtectedSourceState::PermissionDenied => Some(
            SourceIssue::new(
                "authorization_denied",
                "Input Monitoring authorization was denied",
                true,
            )
            .with_remediation("Authorize Input Monitoring"),
        ),
        MacosProtectedSourceState::Revoked => Some(
            SourceIssue::new(
                "authorization_revoked",
                "Input Monitoring authorization was revoked",
                true,
            )
            .with_remediation("Authorize Input Monitoring"),
        ),
        MacosProtectedSourceState::NeedsProcessRestart => Some(
            SourceIssue::new(
                "process_restart_required",
                "Input Monitoring authorization requires a process restart",
                true,
            )
            .with_remediation("Restart the active Hypercolor process"),
        ),
        _ => None,
    }
}

impl InputSource for MacosHostInput {
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
        self.refresh_platform_status()?;
        Ok(())
    }

    fn stop(&mut self) {
        self.status_session.clear();
        self.status.stop();
        self.stop_session();
        self.running = false;
        self.refresh_platform_status()
            .expect("live macOS input status is not retired");
    }

    fn sample(&mut self) -> anyhow::Result<InputData> {
        self.apply_pending_authorization()?;
        self.refresh_worker_health();
        self.refresh_platform_status()?;
        if !self.running || !self.capture_session_active() {
            return Ok(InputData::None);
        }
        Ok(InputData::Interaction(self.fold.sample()))
    }

    fn sample_and_drain_with_delta_secs(
        &mut self,
        _delta_secs: f32,
    ) -> (anyhow::Result<InputData>, Vec<TimedInputEvent>) {
        if let Err(error) = self.apply_pending_authorization() {
            return (Err(error), Vec::new());
        }
        self.refresh_worker_health();
        if let Err(error) = self.refresh_platform_status() {
            return (Err(error), Vec::new());
        }
        if !self.running || !self.capture_session_active() {
            return (Ok(InputData::None), Vec::new());
        }
        let sample = self.fold.sample_and_drain();
        (
            Ok(InputData::Interaction(sample.interaction)),
            sample.events,
        )
    }

    fn drain_events(&mut self) -> Vec<TimedInputEvent> {
        if !self.running || !self.capture_session_active() {
            return Vec::new();
        }
        self.fold.drain_events()
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
}

impl InteractionSource for MacosHostInput {
    fn set_capability_context(&mut self, context: &SourceCapabilityContext) -> anyhow::Result<()> {
        let Some(owner) = MacosCapabilityOwner::from_id(&context.owner) else {
            return Ok(());
        };
        let conflict = context.conflict.as_ref().and_then(|conflict| {
            Some(MacosDaemonOwnerConflict {
                active: MacosCapabilityOwner::from_id(&conflict.active)?,
                contender: MacosCapabilityOwner::from_id(&conflict.contender)?,
                observed_at_ms: conflict.observed_at_ms,
            })
        });
        self.set_daemon_ownership(owner, conflict, context.identity_hash.clone())
    }

    fn input_authorization_action(&self) -> Option<ProtectedSourceAuthorizationAction> {
        if !self.capture_keyboard {
            return None;
        }
        let result = Arc::clone(&self.authorization_result);
        #[cfg(feature = "macos-native-fixtures")]
        let fixture = self.fixture.clone();
        Some(ProtectedSourceAuthorizationAction::new(
            Arc::new(move || {
                #[cfg(feature = "macos-native-fixtures")]
                let granted = fixture
                    .as_ref()
                    .map_or_else(request_input_monitoring, |fixture| {
                        let mut backend = fixture
                            .backend
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        if backend.request_granted {
                            backend.preflight_granted = true;
                            true
                        } else {
                            false
                        }
                    });
                #[cfg(not(feature = "macos-native-fixtures"))]
                let granted = request_input_monitoring();
                result.store(
                    if granted {
                        AUTHORIZATION_GRANTED
                    } else {
                        AUTHORIZATION_DENIED
                    },
                    Ordering::Release,
                );
                Ok(granted)
            }),
            protected_action_identity(self.owner, false),
        ))
    }

    fn interaction_diagnostics(&self) -> Option<crate::input::InteractionDiagnostics> {
        let worker_degradation =
            self.session
                .as_ref()
                .and_then(|session| match session.worker_state() {
                    MacosWorkerState::Running => None,
                    MacosWorkerState::Degraded(reason) => {
                        Some(InteractionDegradation::Unavailable(reason.to_string()))
                    }
                    MacosWorkerState::Failed(reason) => {
                        Some(InteractionDegradation::Unavailable(reason))
                    }
                    MacosWorkerState::PermissionRevoked => {
                        Some(InteractionDegradation::InputMonitoringPermissionRevoked)
                    }
                });
        Some(crate::input::InteractionDiagnostics {
            backend: "cg_event_tap",
            host_capture: true,
            capturing: self.capture_active && self.capture_session_active(),
            devices_opened: self.active_kind_count(),
            devices_denied: 0,
            degraded: worker_degradation.or_else(|| self.degraded.clone()),
        })
    }

    fn set_interaction_capture_active(&mut self, active: bool) -> anyhow::Result<()> {
        self.status.set_policy(true, true, active)?;
        if self.capture_active == active {
            self.refresh_platform_status()?;
            return Ok(());
        }
        self.capture_active = active;
        if !self.running {
            self.refresh_platform_status()?;
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
        self.refresh_platform_status()?;
        Ok(())
    }
}

impl SourceRoleBinding for MacosHostInput {
    type Role = InteractionSourceRole;
}

fn protected_action_identity(
    owner: MacosCapabilityOwner,
    presentation_required: bool,
) -> CapabilityActionIdentity {
    let requires_ui = matches!(
        owner,
        MacosCapabilityOwner::App | MacosCapabilityOwner::Broker
    ) || presentation_required
        && matches!(
            owner,
            MacosCapabilityOwner::LaunchdService | MacosCapabilityOwner::HomebrewService
        );
    CapabilityActionIdentity::new(
        owner.as_str(),
        if requires_ui {
            CapabilityActionDisposition::RequiresUi
        } else {
            CapabilityActionDisposition::Local
        },
    )
}

fn permission_issue() -> SourceIssue {
    SourceIssue::new(
        InteractionDegradation::InputMonitoringPermissionDenied.code(),
        "keyboard capture requires macOS Input Monitoring permission",
        true,
    )
    .with_remediation(
        "open System Settings > Privacy & Security > Input Monitoring, enable Hypercolor, then relaunch the signed app",
    )
}

fn event_mask_issue() -> SourceIssue {
    SourceIssue::new(
        "macos_input_tap_create_failed",
        "macOS event tap did not activate every requested input kind",
        true,
    )
}

fn permission_revoked_issue() -> SourceIssue {
    SourceIssue::new(
        InteractionDegradation::InputMonitoringPermissionRevoked.code(),
        "macOS revoked Input Monitoring during host input capture",
        true,
    )
    .with_remediation(
        "open System Settings > Privacy & Security > Input Monitoring, enable Hypercolor, then relaunch the signed app",
    )
}

fn worker_degradation_issue(reason: &MacosWorkerDegradation) -> SourceIssue {
    let code = match reason {
        MacosWorkerDegradation::TapDisabled(MacosInputGapReason::TapDisabledTimeout) => {
            "macos_input_tap_disabled_timeout"
        }
        MacosWorkerDegradation::TapDisabled(MacosInputGapReason::TapDisabledUserInput) => {
            "macos_input_tap_disabled_user_input"
        }
        MacosWorkerDegradation::TapDisabled(_) | MacosWorkerDegradation::DisplayTopology(_) => {
            "macos_input_run_loop_exited"
        }
    };
    SourceIssue::new(code, reason.to_string(), true)
}

fn classify_start_error(error: &MacosInputError) -> InteractionDegradation {
    if matches!(error, MacosInputError::PermissionDenied) {
        InteractionDegradation::InputMonitoringPermissionDenied
    } else {
        InteractionDegradation::Unavailable(error.to_string())
    }
}

fn issue_for_error(error: &MacosInputError) -> SourceIssue {
    if matches!(error, MacosInputError::PermissionDenied) {
        permission_issue()
    } else if matches!(error, MacosInputError::TapCreation(_)) {
        SourceIssue::new("macos_input_tap_create_failed", error.to_string(), true)
    } else {
        SourceIssue::new("macos_input_run_loop_exited", error.to_string(), true)
    }
}
