//! macOS host input folded from Core Graphics event-tap batches.

use std::collections::{BTreeSet, VecDeque};
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use hypercolor_macos_input::{
    MacosArchitecture, MacosAuthorizationState, MacosCapabilityOwner, MacosDaemonOwnerConflict,
    MacosInputBatch, MacosInputConfig, MacosInputError, MacosInputEvent, MacosInputGapReason,
    MacosInputPublicationOutcome, MacosInputSession, MacosInputStatusSnapshot, MacosModifierFlags,
    MacosPointerButton, MacosProtectedSourceState, MacosScrollPhase, MacosScrollUnit,
    MacosVirtualDesktop, MacosWorkerDegradation, MacosWorkerState, input_diagnostics_envelope,
    input_monitoring_granted, request_input_monitoring,
};
use tracing::{info, warn};

use crate::input::keymap::{macos_key_name, macos_media_key_name};
use crate::input::traits::{
    CapabilityActionDisposition, CapabilityActionIdentity, InputData, InputSource, InteractionData,
    InteractionDegradation, MotionAggregate, PointerMode, ProtectedSourceAuthorizationAction,
    SourceCapabilityContext,
};
use crate::input::{
    LegacyWheelProjector, SourceIssue, SourceKind, SourceSessionSlot, SourceStatusHandle,
    SourceStatusReporter,
};
use crate::types::event::{
    InputButtonState, InputEvent, PointerScrollPhase, PointerScrollUnit, TimedInputEvent,
};

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
    #[cfg(target_os = "macos")]
    {
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
    #[cfg(not(target_os = "macos"))]
    {
        (None, executable, None)
    }
}

type HeldStateKey = (Vec<String>, Vec<String>, i32, i32, i32, i32, bool);

#[derive(Debug, Clone, Copy, PartialEq)]
struct PointerSnapshot {
    x: i32,
    y: i32,
    norm_x: f32,
    norm_y: f32,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MacosInputFoldDiagnostics {
    pub impossible_key_edges: u64,
    pub impossible_button_edges: u64,
    pub unsupported_keys: u64,
    pub unsupported_media_keys: u64,
    pub scroll_overflows: u64,
    pub state_gaps: u64,
    pub topology_resets: u64,
}

#[derive(Default)]
struct SharedState {
    events: VecDeque<TimedInputEvent>,
    dropped: u32,
    pressed_keys: BTreeSet<String>,
    recent_keys: VecDeque<String>,
    held_buttons: BTreeSet<String>,
    motion: MotionAggregate,
    pointer: Option<PointerSnapshot>,
    pointer_present: bool,
    topology_generation: Option<u64>,
    legacy_wheel_projector: LegacyWheelProjector,
    diagnostics: MacosInputFoldDiagnostics,
    epoch: u64,
}

impl SharedState {
    fn clear_live_state(&mut self) {
        self.events.clear();
        self.dropped = 0;
        self.pressed_keys.clear();
        self.recent_keys.clear();
        self.held_buttons.clear();
        self.motion = MotionAggregate::default();
        self.pointer = None;
        self.pointer_present = false;
        self.topology_generation = None;
        self.legacy_wheel_projector.reset();
    }
}

static NEXT_EPOCH: AtomicU64 = AtomicU64::new(1);

pub struct MacosHostInput {
    name: String,
    running: bool,
    capture_active: bool,
    capture_keyboard: bool,
    capture_pointer: bool,
    event_limit: usize,
    generation: u64,
    last_state_key: Option<HeldStateKey>,
    shared: Arc<Mutex<SharedState>>,
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
    active_epoch: Mutex<Option<u64>>,
}

#[cfg(feature = "macos-native-fixtures")]
pub struct MacosHostInputFixture {
    state: Arc<FixtureState>,
    shared: Arc<Mutex<SharedState>>,
    event_limit: usize,
}

#[cfg(feature = "macos-native-fixtures")]
impl MacosHostInputFixture {
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.state
            .active_epoch
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
        *self
            .state
            .active_epoch
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn publish(&self, events: &[MacosInputEvent], at_ms: u64) -> anyhow::Result<bool> {
        let epoch = self
            .active_epoch()
            .ok_or_else(|| anyhow::anyhow!("deterministic macOS input source is inactive"))?;
        self.publish_with_epoch(epoch, events, at_ms)
    }

    pub fn publish_with_epoch(
        &self,
        epoch: u64,
        events: &[MacosInputEvent],
        at_ms: u64,
    ) -> anyhow::Result<bool> {
        let desktop = self
            .state
            .backend
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .virtual_desktop;
        Ok(publish_macos_batch(
            &self.shared,
            MacosInputBatch {
                epoch,
                at_ms,
                events,
                virtual_desktop: desktop,
            },
            self.event_limit,
        ))
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
            event_limit: DEFAULT_EVENT_LIMIT,
            generation: 0,
            last_state_key: None,
            shared: Arc::new(Mutex::new(SharedState::default())),
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
            active_epoch: Mutex::new(None),
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
        let fixture = MacosHostInputFixture {
            state,
            shared: Arc::clone(&source.shared),
            event_limit: source.event_limit,
        };
        (source, fixture)
    }

    #[must_use]
    pub fn epoch(&self) -> u64 {
        self.shared.lock().map_or(0, |state| state.epoch)
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
        self.shared
            .lock()
            .map(|state| state.diagnostics)
            .unwrap_or_default()
    }

    pub fn fold_and_snapshot(
        &mut self,
        batch: MacosInputBatch<'_>,
    ) -> (InteractionData, Vec<TimedInputEvent>) {
        publish_macos_batch(&self.shared, batch, self.event_limit);
        let shared = Arc::clone(&self.shared);
        let Ok(mut state) = shared.lock() else {
            return (InteractionData::default(), Vec::new());
        };
        let events = drain_events(&mut state.events);
        let snapshot = self.build_snapshot(&mut state);
        (snapshot, events)
    }

    fn build_snapshot(&mut self, state: &mut SharedState) -> InteractionData {
        let mut data = InteractionData::default();
        data.keyboard.pressed_keys = state.pressed_keys.iter().cloned().collect();
        data.keyboard.recent_keys = state.recent_keys.drain(..).collect();
        data.mouse.buttons = state.held_buttons.iter().cloned().collect();
        data.mouse.down = !data.mouse.buttons.is_empty();
        if state.pointer_present
            && let Some(pointer) = state.pointer
        {
            data.mouse.mode = PointerMode::Absolute;
            data.mouse.x = pointer.x;
            data.mouse.y = pointer.y;
            data.mouse.norm_x = pointer.norm_x;
            data.mouse.norm_y = pointer.norm_y;
        }
        data.batch.motion = std::mem::take(&mut state.motion);
        data.batch.dropped_events = std::mem::take(&mut state.dropped);

        #[expect(
            clippy::cast_possible_truncation,
            clippy::as_conversions,
            reason = "normalized coordinates are clamped before scaling"
        )]
        let cursor_key = (
            (data.mouse.norm_x * 10_000.0) as i32,
            (data.mouse.norm_y * 10_000.0) as i32,
        );
        let state_key = (
            data.keyboard.pressed_keys.clone(),
            data.mouse.buttons.clone(),
            cursor_key.0,
            cursor_key.1,
            data.mouse.x,
            data.mouse.y,
            state.pointer_present,
        );
        if self.last_state_key.as_ref() != Some(&state_key) || !data.keyboard.recent_keys.is_empty()
        {
            self.generation = self.generation.wrapping_add(1);
            self.last_state_key = Some(state_key);
        }
        data.generation = self.generation;
        data
    }

    fn rotate_epoch_and_clear(&mut self) -> u64 {
        let epoch = NEXT_EPOCH.fetch_add(1, Ordering::Relaxed);
        let mut state = self
            .shared
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.clear_live_state();
        state.epoch = epoch;
        drop(state);
        self.last_state_key = None;
        epoch
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
        let (capture_session_generation, topology_generation, folded_state_gaps) = self
            .shared
            .lock()
            .map(|state| {
                (
                    self.capture_session_active().then_some(state.epoch),
                    state.topology_generation,
                    state.diagnostics.state_gaps,
                )
            })
            .unwrap_or((None, None, 0));
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
        let epoch = self.rotate_epoch_and_clear();

        if !keyboard_granted {
            self.degraded = Some(InteractionDegradation::InputMonitoringPermissionDenied);
        }

        #[cfg(feature = "macos-native-fixtures")]
        if let Some(fixture) = &self.fixture {
            if keyboard_granted
                && ((self.capture_keyboard && !effective_keyboard)
                    || (self.capture_pointer && !effective_pointer))
            {
                self.degraded = Some(InteractionDegradation::Unavailable(
                    "macOS event tap did not activate every requested input kind".to_owned(),
                ));
            }
            *fixture
                .active_epoch
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) =
                (effective_keyboard || effective_pointer).then_some(epoch);
            self.publish_started_status(effective_keyboard, effective_pointer);
            return;
        }

        if !effective_keyboard && !effective_pointer {
            self.publish_started_status(false, false);
            return;
        }

        let shared = Arc::clone(&self.shared);
        let event_limit = self.event_limit;
        let config = MacosInputConfig {
            keyboard: effective_keyboard,
            pointer: effective_pointer,
            epoch,
            clock: Arc::new(crate::input::input_mono_ms),
        };
        match MacosInputSession::start(config, move |batch| {
            if publish_macos_batch(&shared, batch, event_limit) {
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
                self.rotate_epoch_and_clear();
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
                .active_epoch
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
        }
        if let Some(mut session) = self.session.take() {
            session.stop();
        }
        self.rotate_epoch_and_clear();
    }

    fn fixture_session_active(&self) -> bool {
        #[cfg(feature = "macos-native-fixtures")]
        {
            self.fixture.as_ref().is_some_and(|fixture| {
                fixture
                    .active_epoch
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
        let shared = Arc::clone(&self.shared);
        let Ok(mut state) = shared.lock() else {
            return Ok(InputData::None);
        };
        Ok(InputData::Interaction(self.build_snapshot(&mut state)))
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
        let shared = Arc::clone(&self.shared);
        let Ok(mut state) = shared.lock() else {
            return (Ok(InputData::None), Vec::new());
        };
        let events = drain_events(&mut state.events);
        let snapshot = self.build_snapshot(&mut state);
        (Ok(InputData::Interaction(snapshot)), events)
    }

    fn drain_events(&mut self) -> Vec<TimedInputEvent> {
        if !self.running || !self.capture_session_active() {
            return Vec::new();
        }
        self.shared
            .lock()
            .map_or_else(|_| Vec::new(), |mut state| drain_events(&mut state.events))
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

    fn is_interaction_source(&self) -> bool {
        true
    }

    fn is_host_capture_source(&self) -> bool {
        true
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

fn publish_macos_batch(
    shared: &Arc<Mutex<SharedState>>,
    batch: MacosInputBatch<'_>,
    event_limit: usize,
) -> bool {
    let Ok(mut state) = shared.lock() else {
        return false;
    };
    if state.epoch != batch.epoch {
        return false;
    }
    for event in batch.events {
        fold_event(
            &mut state,
            event,
            batch.virtual_desktop,
            batch.at_ms,
            event_limit,
        );
    }
    true
}

fn fold_event(
    state: &mut SharedState,
    event: &MacosInputEvent,
    desktop: MacosVirtualDesktop,
    at_ms: u64,
    event_limit: usize,
) {
    match event {
        MacosInputEvent::Key {
            virtual_keycode,
            pressed,
            autorepeat,
        } => fold_key(
            state,
            *virtual_keycode,
            *pressed,
            *autorepeat,
            at_ms,
            event_limit,
        ),
        MacosInputEvent::ModifierFlags {
            virtual_keycode,
            flags,
        } => fold_modifier(state, *virtual_keycode, *flags, at_ms, event_limit),
        MacosInputEvent::Button { button, pressed } => {
            fold_button(state, *button, *pressed, at_ms, event_limit);
        }
        MacosInputEvent::Motion {
            x,
            y,
            delta_x,
            delta_y,
        } => fold_motion(state, desktop, *x, *y, *delta_x, *delta_y),
        MacosInputEvent::Wheel {
            fixed_delta_x,
            fixed_delta_y,
            unit,
            phase,
            momentum_phase,
        } => fold_scroll(
            state,
            *fixed_delta_x,
            *fixed_delta_y,
            *unit,
            *phase,
            *momentum_phase,
            at_ms,
            event_limit,
        ),
        MacosInputEvent::MediaKey {
            nx_key_type,
            pressed,
            repeat,
        } => fold_media_key(state, *nx_key_type, *pressed, *repeat, at_ms, event_limit),
        MacosInputEvent::StateGap { reason: _ } => {
            state.diagnostics.state_gaps = state.diagnostics.state_gaps.saturating_add(1);
            synthesize_releases(state, at_ms, event_limit);
            state.topology_generation = None;
            state.legacy_wheel_projector.reset();
        }
    }
}

fn fold_key(
    state: &mut SharedState,
    virtual_keycode: u16,
    pressed: bool,
    autorepeat: bool,
    at_ms: u64,
    event_limit: usize,
) {
    let Some(key) = macos_key_name(virtual_keycode) else {
        state.diagnostics.unsupported_keys = state.diagnostics.unsupported_keys.saturating_add(1);
        return;
    };
    fold_named_key(
        state,
        key,
        pressed,
        autorepeat,
        Some(format!("macos:key:{virtual_keycode:02x}")),
        at_ms,
        event_limit,
    );
}

fn fold_media_key(
    state: &mut SharedState,
    nx_key_type: u16,
    pressed: bool,
    repeat: bool,
    at_ms: u64,
    event_limit: usize,
) {
    let Some(key) = macos_media_key_name(nx_key_type) else {
        state.diagnostics.unsupported_media_keys =
            state.diagnostics.unsupported_media_keys.saturating_add(1);
        return;
    };
    fold_named_key(
        state,
        key,
        pressed,
        repeat,
        Some(format!("macos:nx:{nx_key_type}")),
        at_ms,
        event_limit,
    );
}

fn fold_named_key(
    state: &mut SharedState,
    key: &str,
    pressed: bool,
    autorepeat: bool,
    physical_code: Option<String>,
    at_ms: u64,
    event_limit: usize,
) {
    let held = state.pressed_keys.contains(key);
    let button_state = if pressed && autorepeat {
        if !held {
            state.diagnostics.impossible_key_edges =
                state.diagnostics.impossible_key_edges.saturating_add(1);
        }
        InputButtonState::Repeated
    } else if pressed {
        if held {
            state.diagnostics.impossible_key_edges =
                state.diagnostics.impossible_key_edges.saturating_add(1);
            InputButtonState::Repeated
        } else {
            state.pressed_keys.insert(key.to_owned());
            state.recent_keys.push_back(key.to_owned());
            cap_recent(&mut state.recent_keys, event_limit);
            InputButtonState::Pressed
        }
    } else {
        if !state.pressed_keys.remove(key) {
            state.diagnostics.impossible_key_edges =
                state.diagnostics.impossible_key_edges.saturating_add(1);
        }
        InputButtonState::Released
    };
    push_event(
        state,
        TimedInputEvent {
            event: InputEvent::Key {
                source_id: SOURCE_ID.to_owned(),
                key: key.to_owned(),
                state: button_state,
            },
            at_ms,
            seq: 0,
            physical_code,
            repeat_count: 1,
        },
        event_limit,
    );
}

fn fold_modifier(
    state: &mut SharedState,
    virtual_keycode: u16,
    flags: MacosModifierFlags,
    at_ms: u64,
    event_limit: usize,
) {
    let Some((key, mask, counterpart)) = modifier_key(virtual_keycode) else {
        state.diagnostics.unsupported_keys = state.diagnostics.unsupported_keys.saturating_add(1);
        return;
    };
    let held = state.pressed_keys.contains(key);
    let active = flags.contains(mask);
    let pressed = if active != held {
        active
    } else if key == "CapsLock" {
        // The CapsLock flag carries the lock state rather than key travel:
        // the physical release re-reports the unchanged flag. Not an edge
        // and not an anomaly, so it neither emits nor counts.
        return;
    } else if active && counterpart.is_some_and(|other| state.pressed_keys.contains(other)) {
        false
    } else {
        state.diagnostics.impossible_key_edges =
            state.diagnostics.impossible_key_edges.saturating_add(1);
        return;
    };
    fold_named_key(
        state,
        key,
        pressed,
        false,
        Some(format!("macos:key:{virtual_keycode:02x}")),
        at_ms,
        event_limit,
    );
}

fn modifier_key(
    virtual_keycode: u16,
) -> Option<(&'static str, MacosModifierFlags, Option<&'static str>)> {
    match virtual_keycode {
        0x38 => Some(("ShiftLeft", MacosModifierFlags::SHIFT, Some("ShiftRight"))),
        0x3c => Some(("ShiftRight", MacosModifierFlags::SHIFT, Some("ShiftLeft"))),
        0x3b => Some((
            "ControlLeft",
            MacosModifierFlags::CONTROL,
            Some("ControlRight"),
        )),
        0x3e => Some((
            "ControlRight",
            MacosModifierFlags::CONTROL,
            Some("ControlLeft"),
        )),
        0x3a => Some(("AltLeft", MacosModifierFlags::ALTERNATE, Some("AltRight"))),
        0x3d => Some(("AltRight", MacosModifierFlags::ALTERNATE, Some("AltLeft"))),
        0x37 => Some(("MetaLeft", MacosModifierFlags::COMMAND, Some("MetaRight"))),
        0x36 => Some(("MetaRight", MacosModifierFlags::COMMAND, Some("MetaLeft"))),
        0x39 => Some(("CapsLock", MacosModifierFlags::ALPHA_SHIFT, None)),
        _ => None,
    }
}

fn fold_button(
    state: &mut SharedState,
    button: MacosPointerButton,
    pressed: bool,
    at_ms: u64,
    event_limit: usize,
) {
    let name = pointer_button_name(button);
    let changed = if pressed {
        state.held_buttons.insert(name.clone())
    } else {
        state.held_buttons.remove(&name)
    };
    if !changed {
        state.diagnostics.impossible_button_edges =
            state.diagnostics.impossible_button_edges.saturating_add(1);
    }
    push_event(
        state,
        TimedInputEvent {
            event: InputEvent::MouseButton {
                source_id: SOURCE_ID.to_owned(),
                button: name.clone(),
                state: if pressed {
                    InputButtonState::Pressed
                } else {
                    InputButtonState::Released
                },
            },
            at_ms,
            seq: 0,
            physical_code: Some(format!("macos:button:{name}")),
            repeat_count: 1,
        },
        event_limit,
    );
}

fn pointer_button_name(button: MacosPointerButton) -> String {
    match button {
        MacosPointerButton::Left => "left".to_owned(),
        MacosPointerButton::Right => "right".to_owned(),
        MacosPointerButton::Middle => "middle".to_owned(),
        MacosPointerButton::Other(number) => {
            format!("button{}", u32::from(number).saturating_add(1))
        }
    }
}

fn fold_motion(
    state: &mut SharedState,
    desktop: MacosVirtualDesktop,
    x: f64,
    y: f64,
    delta_x: f64,
    delta_y: f64,
) {
    let (norm_x, norm_y) = desktop.normalize(x, y);
    state.pointer = Some(PointerSnapshot {
        x: saturating_i32(x),
        y: saturating_i32(y),
        norm_x: norm_x as f32,
        norm_y: norm_y as f32,
    });
    state.pointer_present = true;
    if state.topology_generation != Some(desktop.topology_generation) {
        state.topology_generation = Some(desktop.topology_generation);
        state.diagnostics.topology_resets = state.diagnostics.topology_resets.saturating_add(1);
        return;
    }
    let dx = (delta_x / desktop.width) as f32;
    let dy = (delta_y / desktop.height) as f32;
    state.motion.dx += dx;
    state.motion.dy += dy;
    state.motion.distance += dx.hypot(dy);
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::as_conversions,
    reason = "finite desktop coordinates saturate at the public i32 boundary"
)]
fn saturating_i32(value: f64) -> i32 {
    value
        .round()
        .clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32
}

#[expect(
    clippy::too_many_arguments,
    reason = "the native wheel record and fold context are one atomic event"
)]
fn fold_scroll(
    state: &mut SharedState,
    fixed_delta_x: i64,
    fixed_delta_y: i64,
    unit: MacosScrollUnit,
    phase: MacosScrollPhase,
    momentum_phase: MacosScrollPhase,
    at_ms: u64,
    event_limit: usize,
) {
    let (delta_x_q16_16, delta_y_q16_16, canonical_unit) = match unit {
        MacosScrollUnit::Notches => {
            let (x, overflow_x) = checked_line120(fixed_delta_x);
            let (y, overflow_y) = checked_line120(fixed_delta_y);
            if overflow_x || overflow_y {
                state.diagnostics.scroll_overflows =
                    state.diagnostics.scroll_overflows.saturating_add(1);
            }
            (x, y, PointerScrollUnit::Line120)
        }
        MacosScrollUnit::Pixels => (fixed_delta_x, fixed_delta_y, PointerScrollUnit::Pixels),
    };
    push_event(
        state,
        TimedInputEvent {
            event: InputEvent::PointerScroll {
                source_id: SOURCE_ID.to_owned(),
                delta_x_q16_16,
                delta_y_q16_16,
                unit: canonical_unit,
                phase: canonical_phase(phase),
                momentum_phase: canonical_phase(momentum_phase),
            },
            at_ms,
            seq: 0,
            physical_code: Some("macos:scroll".to_owned()),
            repeat_count: 1,
        },
        event_limit,
    );
    if canonical_unit != PointerScrollUnit::Line120 {
        return;
    }
    let legacy_delta = state.legacy_wheel_projector.project(delta_y_q16_16);
    if legacy_delta == 0 {
        return;
    }
    push_event(
        state,
        TimedInputEvent {
            event: InputEvent::MouseWheel {
                source_id: SOURCE_ID.to_owned(),
                delta_hi_res: legacy_delta,
            },
            at_ms,
            seq: 0,
            physical_code: Some("macos:legacy-wheel-shadow".to_owned()),
            repeat_count: 1,
        },
        event_limit,
    );
}

fn checked_line120(value: i64) -> (i64, bool) {
    value.checked_mul(120).map_or_else(
        || {
            (
                if value.is_negative() {
                    i64::MIN
                } else {
                    i64::MAX
                },
                true,
            )
        },
        |scaled| (scaled, false),
    )
}

const fn canonical_phase(phase: MacosScrollPhase) -> PointerScrollPhase {
    match phase {
        MacosScrollPhase::None => PointerScrollPhase::None,
        MacosScrollPhase::MayBegin => PointerScrollPhase::MayBegin,
        MacosScrollPhase::Began => PointerScrollPhase::Began,
        MacosScrollPhase::Stationary => PointerScrollPhase::Stationary,
        MacosScrollPhase::Changed => PointerScrollPhase::Changed,
        MacosScrollPhase::Ended => PointerScrollPhase::Ended,
        MacosScrollPhase::Cancelled => PointerScrollPhase::Cancelled,
    }
}

fn synthesize_releases(state: &mut SharedState, at_ms: u64, event_limit: usize) {
    let keys = std::mem::take(&mut state.pressed_keys);
    for key in keys {
        push_event(
            state,
            TimedInputEvent {
                event: InputEvent::Key {
                    source_id: SOURCE_ID.to_owned(),
                    key,
                    state: InputButtonState::Released,
                },
                at_ms,
                seq: 0,
                physical_code: None,
                repeat_count: 1,
            },
            event_limit,
        );
    }
    let buttons = std::mem::take(&mut state.held_buttons);
    for button in buttons {
        push_event(
            state,
            TimedInputEvent {
                event: InputEvent::MouseButton {
                    source_id: SOURCE_ID.to_owned(),
                    button,
                    state: InputButtonState::Released,
                },
                at_ms,
                seq: 0,
                physical_code: None,
                repeat_count: 1,
            },
            event_limit,
        );
    }
}

fn push_event(state: &mut SharedState, event: TimedInputEvent, limit: usize) {
    if limit == 0 {
        state.dropped = state
            .dropped
            .saturating_add(u32::try_from(state.events.len()).unwrap_or(u32::MAX))
            .saturating_add(1);
        state.events.clear();
        return;
    }
    while state.events.len() >= limit {
        state.events.pop_front();
        state.dropped = state.dropped.saturating_add(1);
    }
    state.events.push_back(event);
}

fn cap_recent(recent: &mut VecDeque<String>, limit: usize) {
    while recent.len() > limit {
        recent.pop_front();
    }
}

fn drain_events(events: &mut VecDeque<TimedInputEvent>) -> Vec<TimedInputEvent> {
    events.drain(..).collect()
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
