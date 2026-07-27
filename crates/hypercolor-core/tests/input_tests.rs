//! Tests for the input source abstraction layer.

#[cfg(target_os = "linux")]
use hypercolor_core::input::SensorPoller;
#[cfg(target_os = "windows")]
use hypercolor_core::input::WindowsHostInput;
use hypercolor_core::input::audio::AudioInput;
use hypercolor_core::input::screen::CaptureConfig as ScreenCaptureConfig;
use hypercolor_core::input::screen::ScreenCaptureInput;
#[cfg(target_os = "windows")]
use hypercolor_core::input::screen::WindowsScreenCaptureInput;
#[cfg(target_os = "linux")]
use hypercolor_core::input::screen::{CaptureConfig, WaylandScreenCaptureInput};
use hypercolor_core::input::{
    AudioReconfigurationConflict, BrowserInputSource, INPUT_EVENT_RING_CAPACITY, InputData,
    InputManager, InputSource, MediaSource, NetSource, ScreenData, SourceFreshness, SourceIssue,
    SourceKind, SourceResourceScanHealth, SourceSessionSlot, SourceSessionWriter, SourceState,
    SourceStatusError, SourceStatusHandle, SourceStatusReporter, SourceStatusWriter,
    SourceTimestampField, TerminalFailureLatch, classify_source_resource_scan,
};
use hypercolor_core::types::audio::{AudioData, AudioPipelineConfig, AudioSourceType};
use hypercolor_core::types::event::{InputButtonState, InputEvent, TimedInputEvent, ZoneColors};
use std::collections::VecDeque;
#[cfg(target_os = "linux")]
use std::fs;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

// ── Mock Sources ───────────────────────────────────────────────────────────

/// A mock audio input source that produces a known `AudioData` snapshot.
struct MockAudioSource {
    running: bool,
    rms_level: f32,
}

struct StatusAwareScreenSource {
    running: bool,
    status: SourceStatusReporter,
    session_sink: Arc<Mutex<Option<SourceSessionWriter>>>,
}

impl StatusAwareScreenSource {
    fn new(session_sink: Arc<Mutex<Option<SourceSessionWriter>>>) -> Self {
        Self {
            running: false,
            status: SourceStatusReporter::new(
                "status_aware_screen",
                SourceKind::Screen,
                "test",
                true,
                true,
                true,
            ),
            session_sink,
        }
    }
}

impl InputSource for StatusAwareScreenSource {
    fn name(&self) -> &'static str {
        "StatusAwareScreen"
    }

    fn source_status_handle(&self) -> Option<SourceStatusHandle> {
        Some(self.status.handle())
    }

    fn source_status_reporter(&mut self) -> Option<&mut SourceStatusReporter> {
        Some(&mut self.status)
    }

    fn start(&mut self) -> anyhow::Result<()> {
        let session = self
            .status
            .begin_session()?
            .expect("manager binds a graph generation before start");
        session.mark_event_driven_live_without_deadline(1);
        *self.session_sink.lock().expect("session sink lock") = Some(session);
        self.running = true;
        Ok(())
    }

    fn stop(&mut self) {
        self.status.stop();
        self.running = false;
    }

    fn sample(&mut self) -> anyhow::Result<InputData> {
        Ok(InputData::None)
    }

    fn is_running(&self) -> bool {
        self.running
    }

    fn is_screen_source(&self) -> bool {
        true
    }
}

#[derive(Default)]
struct DemandTransitionState {
    active: bool,
    transitions: Vec<bool>,
}

struct FallibleStatusScreenSource {
    name: &'static str,
    running: bool,
    fail_on: Option<bool>,
    state: Arc<Mutex<DemandTransitionState>>,
    status: SourceStatusReporter,
}

impl FallibleStatusScreenSource {
    fn new(
        name: &'static str,
        fail_on: Option<bool>,
        state: Arc<Mutex<DemandTransitionState>>,
    ) -> Self {
        Self {
            name,
            running: false,
            fail_on,
            state,
            status: SourceStatusReporter::new(name, SourceKind::Screen, "test", true, true, false),
        }
    }
}

impl InputSource for FallibleStatusScreenSource {
    fn name(&self) -> &'static str {
        self.name
    }

    fn source_status_handle(&self) -> Option<SourceStatusHandle> {
        Some(self.status.handle())
    }

    fn source_status_reporter(&mut self) -> Option<&mut SourceStatusReporter> {
        Some(&mut self.status)
    }

    fn start(&mut self) -> anyhow::Result<()> {
        self.running = true;
        Ok(())
    }

    fn stop(&mut self) {
        self.status.stop();
        self.running = false;
    }

    fn sample(&mut self) -> anyhow::Result<InputData> {
        Ok(InputData::None)
    }

    fn is_running(&self) -> bool {
        self.running
    }

    fn is_screen_source(&self) -> bool {
        true
    }

    fn set_screen_capture_active(&mut self, active: bool) -> anyhow::Result<()> {
        self.status.set_policy(true, true, active)?;
        {
            let mut state = self.state.lock().expect("demand state lock");
            state.active = active;
            state.transitions.push(active);
        }
        if active && self.running {
            self.status.begin_session()?;
        }
        if self.fail_on == Some(active) {
            anyhow::bail!("injected demand transition failure");
        }
        Ok(())
    }
}

struct RestartingStatusScreenSource {
    running: bool,
    target_fps: u32,
    sessions: Arc<Mutex<Vec<u64>>>,
    status: SourceStatusReporter,
}

impl RestartingStatusScreenSource {
    fn new(sessions: Arc<Mutex<Vec<u64>>>) -> Self {
        Self {
            running: false,
            target_fps: ScreenCaptureConfig::default().target_fps,
            sessions,
            status: SourceStatusReporter::new(
                "restarting_status_screen",
                SourceKind::Screen,
                "test",
                true,
                true,
                true,
            ),
        }
    }

    fn begin_status_session(&mut self) -> anyhow::Result<()> {
        let session = self
            .status
            .begin_session()?
            .expect("manager binds a graph generation");
        self.sessions
            .lock()
            .expect("session log lock")
            .push(session.session_generation());
        Ok(())
    }
}

impl InputSource for RestartingStatusScreenSource {
    fn name(&self) -> &'static str {
        "RestartingStatusScreen"
    }

    fn source_status_handle(&self) -> Option<SourceStatusHandle> {
        Some(self.status.handle())
    }

    fn source_status_reporter(&mut self) -> Option<&mut SourceStatusReporter> {
        Some(&mut self.status)
    }

    fn start(&mut self) -> anyhow::Result<()> {
        self.running = true;
        self.begin_status_session()
    }

    fn stop(&mut self) {
        self.status.stop();
        self.running = false;
    }

    fn sample(&mut self) -> anyhow::Result<InputData> {
        Ok(InputData::None)
    }

    fn is_running(&self) -> bool {
        self.running
    }

    fn is_screen_source(&self) -> bool {
        true
    }

    fn reconfigure_screen_capture(&mut self, config: &ScreenCaptureConfig) -> anyhow::Result<()> {
        if self.target_fps != config.target_fps {
            self.target_fps = config.target_fps;
            self.begin_status_session()?;
        }
        Ok(())
    }

    fn reselect_screen_source(&mut self) -> anyhow::Result<()> {
        self.begin_status_session()
    }
}

impl MockAudioSource {
    fn new(rms_level: f32) -> Self {
        Self {
            running: false,
            rms_level,
        }
    }
}

impl InputSource for MockAudioSource {
    fn name(&self) -> &'static str {
        "MockAudio"
    }

    fn start(&mut self) -> anyhow::Result<()> {
        self.running = true;
        Ok(())
    }

    fn stop(&mut self) {
        self.running = false;
    }

    fn sample(&mut self) -> anyhow::Result<InputData> {
        let mut data = AudioData::silence();
        data.rms_level = self.rms_level;
        Ok(InputData::Audio(data))
    }

    fn is_running(&self) -> bool {
        self.running
    }
}

struct ReconfigurableAudioSource {
    running: bool,
    capture_active: bool,
    config: AudioPipelineConfig,
    name: String,
}

impl ReconfigurableAudioSource {
    fn new() -> Self {
        Self {
            running: false,
            capture_active: false,
            config: AudioPipelineConfig::default(),
            name: "AudioInput(default)".to_owned(),
        }
    }
}

impl InputSource for ReconfigurableAudioSource {
    fn name(&self) -> &str {
        &self.name
    }

    fn start(&mut self) -> anyhow::Result<()> {
        self.running = true;
        Ok(())
    }

    fn stop(&mut self) {
        self.running = false;
    }

    fn sample(&mut self) -> anyhow::Result<InputData> {
        let mut data = AudioData::silence();
        data.rms_level =
            if matches!(self.config.source, AudioSourceType::None) || !self.capture_active {
                0.0
            } else {
                0.5
            };
        Ok(InputData::Audio(data))
    }

    fn is_running(&self) -> bool {
        self.running
    }

    fn is_audio_source(&self) -> bool {
        true
    }

    fn reconfigure_audio(
        &mut self,
        config: &AudioPipelineConfig,
        name: &str,
        capture_active: bool,
    ) -> anyhow::Result<()> {
        self.config = config.clone();
        name.clone_into(&mut self.name);
        self.running = true;
        self.capture_active = capture_active;
        Ok(())
    }

    fn set_audio_capture_active(&mut self, active: bool) -> anyhow::Result<()> {
        self.capture_active = active;
        Ok(())
    }
}

struct StatusReconfigurableAudioSource {
    running: bool,
    status: SourceStatusReporter,
}

impl StatusReconfigurableAudioSource {
    fn new() -> Self {
        Self {
            running: false,
            status: SourceStatusReporter::new(
                "status_reconfigurable_audio",
                SourceKind::Audio,
                "test",
                true,
                true,
                false,
            ),
        }
    }
}

impl InputSource for StatusReconfigurableAudioSource {
    fn name(&self) -> &'static str {
        "StatusReconfigurableAudio"
    }

    fn source_status_handle(&self) -> Option<SourceStatusHandle> {
        Some(self.status.handle())
    }

    fn source_status_reporter(&mut self) -> Option<&mut SourceStatusReporter> {
        Some(&mut self.status)
    }

    fn start(&mut self) -> anyhow::Result<()> {
        self.running = true;
        Ok(())
    }

    fn stop(&mut self) {
        self.status.stop();
        self.running = false;
    }

    fn sample(&mut self) -> anyhow::Result<InputData> {
        Ok(InputData::None)
    }

    fn is_running(&self) -> bool {
        self.running
    }

    fn is_audio_source(&self) -> bool {
        true
    }

    fn reconfigure_audio(
        &mut self,
        config: &AudioPipelineConfig,
        _name: &str,
        capture_active: bool,
    ) -> anyhow::Result<()> {
        let configured = !matches!(config.source, AudioSourceType::None);
        self.status
            .set_policy(configured, true, configured && capture_active)?;
        self.running = true;
        if configured && capture_active {
            let session = self
                .status
                .begin_session()?
                .expect("manager binds audio reconfiguration generation");
            session.mark_event_driven_live_without_deadline(1);
        }
        Ok(())
    }
}

/// A mock screen capture source that produces a known set of zone colors.
struct MockScreenSource {
    running: bool,
    zone_count: usize,
}

impl MockScreenSource {
    fn new(zone_count: usize) -> Self {
        Self {
            running: false,
            zone_count,
        }
    }
}

impl InputSource for MockScreenSource {
    fn name(&self) -> &'static str {
        "MockScreen"
    }

    fn start(&mut self) -> anyhow::Result<()> {
        self.running = true;
        Ok(())
    }

    fn stop(&mut self) {
        self.running = false;
    }

    fn sample(&mut self) -> anyhow::Result<InputData> {
        let zones: Vec<ZoneColors> = (0..self.zone_count)
            .map(|i| ZoneColors {
                zone_id: format!("screen:zone_{i}"),
                colors: vec![[128, 64, 32]; 10],
            })
            .collect();
        Ok(InputData::Screen(ScreenData::from_zones(zones, 0, 0)))
    }

    fn is_running(&self) -> bool {
        self.running
    }
}

/// A mock source that always fails on start.
struct FailingSource;

impl InputSource for FailingSource {
    fn name(&self) -> &'static str {
        "FailingSource"
    }

    fn start(&mut self) -> anyhow::Result<()> {
        anyhow::bail!("device not found")
    }

    fn stop(&mut self) {}

    fn sample(&mut self) -> anyhow::Result<InputData> {
        anyhow::bail!("not running")
    }

    fn is_running(&self) -> bool {
        false
    }
}

/// A mock source that starts fine but fails on sample.
struct FaultySampleSource {
    running: bool,
}

impl FaultySampleSource {
    fn new() -> Self {
        Self { running: false }
    }
}

struct DeltaAwareSource {
    running: bool,
}

impl DeltaAwareSource {
    fn new() -> Self {
        Self { running: false }
    }
}

impl InputSource for DeltaAwareSource {
    fn name(&self) -> &'static str {
        "DeltaAware"
    }

    fn start(&mut self) -> anyhow::Result<()> {
        self.running = true;
        Ok(())
    }

    fn stop(&mut self) {
        self.running = false;
    }

    fn sample(&mut self) -> anyhow::Result<InputData> {
        Ok(InputData::None)
    }

    fn sample_with_delta_secs(&mut self, delta_secs: f32) -> anyhow::Result<InputData> {
        let mut data = AudioData::silence();
        data.rms_level = delta_secs;
        Ok(InputData::Audio(data))
    }

    fn is_running(&self) -> bool {
        self.running
    }
}

impl InputSource for FaultySampleSource {
    fn name(&self) -> &'static str {
        "FaultySample"
    }

    fn start(&mut self) -> anyhow::Result<()> {
        self.running = true;
        Ok(())
    }

    fn stop(&mut self) {
        self.running = false;
    }

    fn sample(&mut self) -> anyhow::Result<InputData> {
        anyhow::bail!("capture stream died")
    }

    fn is_running(&self) -> bool {
        self.running
    }
}

struct EventfulSource {
    running: bool,
    events: Vec<TimedInputEvent>,
}

struct StatuslessDemandSource {
    running: bool,
    capture_active: bool,
}

struct SequencedSource {
    samples: VecDeque<InputData>,
    interaction: bool,
    running: bool,
}

impl SequencedSource {
    fn new(samples: impl IntoIterator<Item = InputData>, interaction: bool) -> Self {
        Self {
            samples: samples.into_iter().collect(),
            interaction,
            running: false,
        }
    }
}

impl InputSource for SequencedSource {
    fn name(&self) -> &'static str {
        "Sequenced"
    }

    fn start(&mut self) -> anyhow::Result<()> {
        self.running = true;
        Ok(())
    }

    fn stop(&mut self) {
        self.running = false;
    }

    fn sample(&mut self) -> anyhow::Result<InputData> {
        Ok(if self.running {
            self.samples.pop_front().unwrap_or(InputData::None)
        } else {
            InputData::None
        })
    }

    fn is_running(&self) -> bool {
        self.running
    }

    fn is_interaction_source(&self) -> bool {
        self.interaction
    }
}

impl EventfulSource {
    fn new(events: Vec<InputEvent>) -> Self {
        Self {
            running: false,
            events: events
                .into_iter()
                .map(|event| TimedInputEvent {
                    event,
                    at_ms: 0,
                    seq: 0,
                    physical_code: None,
                    repeat_count: 1,
                })
                .collect(),
        }
    }
}

impl InputSource for EventfulSource {
    fn name(&self) -> &'static str {
        "EventfulSource"
    }

    fn start(&mut self) -> anyhow::Result<()> {
        self.running = true;
        Ok(())
    }

    fn stop(&mut self) {
        self.running = false;
    }

    fn sample(&mut self) -> anyhow::Result<InputData> {
        Ok(InputData::None)
    }

    fn is_running(&self) -> bool {
        self.running
    }

    fn drain_events(&mut self) -> Vec<TimedInputEvent> {
        std::mem::take(&mut self.events)
    }
}

impl InputSource for StatuslessDemandSource {
    fn name(&self) -> &'static str {
        "statusless-demand"
    }

    fn start(&mut self) -> anyhow::Result<()> {
        self.running = true;
        Ok(())
    }

    fn stop(&mut self) {
        self.running = false;
    }

    fn sample(&mut self) -> anyhow::Result<InputData> {
        Ok(InputData::None)
    }

    fn is_running(&self) -> bool {
        self.running && self.capture_active
    }

    fn is_interaction_source(&self) -> bool {
        true
    }

    fn set_interaction_capture_active(&mut self, active: bool) -> anyhow::Result<()> {
        self.capture_active = active;
        Ok(())
    }
}

struct CaptureTrackingAudioSource {
    running: bool,
    capture_active: bool,
}

impl CaptureTrackingAudioSource {
    fn new() -> Self {
        Self {
            running: false,
            capture_active: false,
        }
    }
}

struct CaptureTrackingScreenSource {
    running: bool,
    capture_active: bool,
}

struct CountingScreenDemandSource {
    running: bool,
    capture_active: bool,
    activations: Arc<AtomicUsize>,
}

impl CountingScreenDemandSource {
    fn new(activations: Arc<AtomicUsize>) -> Self {
        Self {
            running: false,
            capture_active: false,
            activations,
        }
    }
}

impl InputSource for CountingScreenDemandSource {
    fn name(&self) -> &'static str {
        "CountingScreenDemand"
    }

    fn start(&mut self) -> anyhow::Result<()> {
        self.running = true;
        Ok(())
    }

    fn stop(&mut self) {
        self.running = false;
        self.capture_active = false;
    }

    fn sample(&mut self) -> anyhow::Result<InputData> {
        Ok(InputData::None)
    }

    fn is_running(&self) -> bool {
        self.running
    }

    fn is_screen_source(&self) -> bool {
        true
    }

    fn set_screen_capture_active(&mut self, active: bool) -> anyhow::Result<()> {
        if active && !self.capture_active {
            self.activations.fetch_add(1, Ordering::Relaxed);
        }
        self.capture_active = active;
        Ok(())
    }
}

impl CaptureTrackingScreenSource {
    fn new() -> Self {
        Self {
            running: false,
            capture_active: false,
        }
    }
}

impl InputSource for CaptureTrackingScreenSource {
    fn name(&self) -> &'static str {
        "CaptureTrackingScreen"
    }

    fn start(&mut self) -> anyhow::Result<()> {
        self.running = true;
        Ok(())
    }

    fn stop(&mut self) {
        self.running = false;
        self.capture_active = false;
    }

    fn sample(&mut self) -> anyhow::Result<InputData> {
        if !self.running || !self.capture_active {
            return Ok(InputData::None);
        }

        Ok(InputData::Screen(ScreenData::from_zones(
            vec![ZoneColors {
                zone_id: "screen:zone_0".to_owned(),
                colors: vec![[32, 64, 128]],
            }],
            1,
            1,
        )))
    }

    fn is_running(&self) -> bool {
        self.running
    }

    fn is_screen_source(&self) -> bool {
        true
    }

    fn set_screen_capture_active(&mut self, active: bool) -> anyhow::Result<()> {
        self.capture_active = active;
        Ok(())
    }
}

impl InputSource for CaptureTrackingAudioSource {
    fn name(&self) -> &'static str {
        "CaptureTrackingAudio"
    }

    fn start(&mut self) -> anyhow::Result<()> {
        self.running = true;
        Ok(())
    }

    fn stop(&mut self) {
        self.running = false;
    }

    fn sample(&mut self) -> anyhow::Result<InputData> {
        let mut data = AudioData::silence();
        data.rms_level = if self.capture_active { 1.0 } else { 0.0 };
        Ok(InputData::Audio(data))
    }

    fn is_running(&self) -> bool {
        self.running
    }

    fn is_audio_source(&self) -> bool {
        true
    }

    fn reconfigure_audio(
        &mut self,
        _config: &AudioPipelineConfig,
        _name: &str,
        capture_active: bool,
    ) -> anyhow::Result<()> {
        self.capture_active = capture_active;
        Ok(())
    }
}

// ── InputSource Trait Tests ────────────────────────────────────────────────

#[test]
fn audio_source_lifecycle() {
    let mut src = MockAudioSource::new(0.75);
    assert!(!src.is_running());
    assert_eq!(src.name(), "MockAudio");

    src.start().expect("start should succeed");
    assert!(src.is_running());

    src.stop();
    assert!(!src.is_running());
}

#[test]
fn audio_source_produces_known_data() {
    let mut src = MockAudioSource::new(0.42);
    let data = src.sample().expect("sample should succeed");

    match data {
        InputData::Audio(audio) => {
            assert!((audio.rms_level - 0.42).abs() < f32::EPSILON);
            assert!(!audio.beat_detected);
        }
        _ => panic!("expected InputData::Audio"),
    }
}

#[test]
fn screen_source_produces_zone_colors() {
    let mut src = MockScreenSource::new(3);
    let data = src.sample().expect("sample should succeed");

    match data {
        InputData::Screen(screen) => {
            assert_eq!(screen.zone_colors.len(), 3);
            assert_eq!(screen.zone_colors[0].zone_id, "screen:zone_0");
            assert_eq!(screen.zone_colors[2].zone_id, "screen:zone_2");
            assert_eq!(screen.zone_colors[1].colors.len(), 10);
            assert_eq!(screen.zone_colors[1].colors[0], [128, 64, 32]);
        }
        _ => panic!("expected InputData::Screen"),
    }
}

#[test]
fn failing_source_reports_error() {
    let mut src = FailingSource;
    let result = src.start();
    assert!(result.is_err());
    assert!(!src.is_running());
}

#[test]
fn input_data_none_variant() {
    // Verify None variant can be created and matched.
    let data = InputData::None;
    assert!(matches!(data, InputData::None));
}

// ── InputManager Tests ─────────────────────────────────────────────────────

#[test]
fn manager_starts_empty() {
    let mut mgr = InputManager::new();
    let samples = mgr.sample_all();
    assert!(samples.is_empty());
}

#[test]
fn manager_default_is_empty() {
    let mut mgr = InputManager::default();
    let samples = mgr.sample_all();
    assert!(samples.is_empty());
    assert_eq!(mgr.source_count(), 0);
    assert!(mgr.source_names().is_empty());
}

#[test]
fn status_registry_advances_while_manager_ownership_is_held() {
    let session_sink = Arc::new(Mutex::new(None));
    let mut manager = InputManager::new();
    manager.add_source(Box::new(StatusAwareScreenSource::new(Arc::clone(
        &session_sink,
    ))));
    manager.start_all().expect("status-aware source starts");
    let registry = manager.source_status_registry();
    let manager = Mutex::new(manager);
    let manager_guard = manager.lock().expect("manager ownership lock");
    let session = session_sink
        .lock()
        .expect("session sink lock")
        .clone()
        .expect("worker session was published");

    assert!(session.degraded(SourceIssue::new(
        "worker_degraded",
        "worker can publish without borrowing the manager",
        true,
    )));
    let snapshot = registry.snapshot();
    let statuses = snapshot.statuses();
    assert_eq!(statuses.len(), 1);
    assert_eq!(statuses[0].state, SourceState::Degraded);
    assert_eq!(
        statuses[0].issue.as_ref().map(|issue| issue.code.as_ref()),
        Some("worker_degraded")
    );
    drop(manager_guard);
}

#[test]
fn manager_graph_generation_advances_and_removal_retires_status() {
    let session_sink = Arc::new(Mutex::new(None));
    let mut manager = InputManager::new();
    manager.add_source(Box::new(StatusAwareScreenSource::new(session_sink)));
    let registry = manager.source_status_registry();
    let added = registry.snapshot();
    assert_eq!(added.source_graph_generation(), 1);
    let removed_handle = added.handles()[0].clone();

    manager.start_all().expect("status-aware source starts");
    let started_generation = manager.source_graph_generation();
    assert!(started_generation > added.source_graph_generation());
    manager.remove_screen_sources();

    let removed = registry.snapshot();
    assert!(removed.source_graph_generation() > started_generation);
    assert!(removed.handles().is_empty());
    let retired = removed_handle.snapshot();
    assert!(retired.retired);
    assert_eq!(
        retired.source_graph_generation,
        removed.source_graph_generation()
    );
}

#[test]
fn replacing_source_advances_graph_and_retires_previous_handle() {
    let session_sink = Arc::new(Mutex::new(None));
    let mut manager = InputManager::new();
    manager.add_source(Box::new(StatusAwareScreenSource::new(session_sink)));
    let registry = manager.source_status_registry();
    let graph = manager.input_graph_handle();
    let before = registry.snapshot();
    let previous_slot_id = graph.snapshot().slots()[0].id();
    let previous_handle = before.handles()[0].clone();

    let replacement = manager.replace_source(0, Box::new(BrowserInputSource::new()));

    assert!(replacement.is_ok());
    let after = registry.snapshot();
    let replacement_graph = graph.snapshot();
    assert!(after.source_graph_generation() > before.source_graph_generation());
    assert_eq!(
        replacement_graph.generation(),
        after.source_graph_generation()
    );
    assert_ne!(replacement_graph.slots()[0].id(), previous_slot_id);
    assert_eq!(after.handles().len(), 1);
    assert_eq!(
        after.handles()[0].snapshot().source_id.as_ref(),
        "browser_input"
    );
    let retired = previous_handle.snapshot();
    assert!(retired.retired);
    assert_eq!(
        retired.source_graph_generation,
        after.source_graph_generation()
    );
}

#[test]
fn manager_instruments_statusless_sources_in_their_graph_slot() {
    let mut manager = InputManager::new();
    manager.add_source(Box::new(EventfulSource::new(Vec::new())));
    manager
        .start_all()
        .expect("compatibility source should start");

    let graph = manager.input_graph_handle().snapshot();
    let registry = manager.source_status_registry().snapshot();
    let graph_status = graph.slots()[0].status().snapshot();
    let registry_status = registry.handles()[0].snapshot();

    assert_eq!(graph.generation(), registry.source_graph_generation());
    assert_eq!(graph_status.source_graph_generation, graph.generation());
    assert!(Arc::ptr_eq(&graph_status, &registry_status));
    assert_eq!(graph_status.kind, SourceKind::Interaction);
    assert_eq!(graph_status.state, SourceState::Live);
    assert_eq!(graph_status.freshness, SourceFreshness::NotApplicable);
    assert!(graph_status.source_id.starts_with("compatibility:"));
}

#[test]
fn manager_tracks_statusless_source_demand_transitions() {
    let mut manager = InputManager::new();
    manager.add_source(Box::new(StatuslessDemandSource {
        running: false,
        capture_active: true,
    }));
    manager.start_all().expect("statusless source should start");
    let registry = manager.source_status_registry();

    manager
        .set_interaction_capture_active(false)
        .expect("statusless source should deactivate");
    let inactive = registry.snapshot().handles()[0].snapshot();
    assert!(!inactive.demanded);
    assert_eq!(inactive.state, SourceState::Stopped);

    manager
        .set_interaction_capture_active(true)
        .expect("statusless source should reactivate");
    let active = registry.snapshot().handles()[0].snapshot();
    assert!(active.demanded);
    assert_eq!(active.state, SourceState::Live);
    assert!(active.session_generation > inactive.session_generation);
}

#[test]
fn input_graph_event_ring_bounds_history_and_advances_cursor_once() {
    let events = (0..INPUT_EVENT_RING_CAPACITY + 44)
        .map(|index| InputEvent::Key {
            source_id: "event-ring".into(),
            key: format!("Key{index}"),
            state: InputButtonState::Pressed,
        })
        .collect();
    let mut manager = InputManager::new();
    manager.add_source(Box::new(EventfulSource::new(events)));
    manager.start_all().expect("event source should start");
    manager.sample_sources(1.0 / 60.0);

    let graph = manager.input_graph_handle().snapshot();
    let slot = &graph.slots()[0];
    let mut delivered = Vec::new();
    let first = slot.read_events_since(0, &mut delivered);
    assert_eq!(delivered.len(), INPUT_EVENT_RING_CAPACITY);
    assert_eq!(first.dropped, 44);
    assert_eq!(first.dropped_total, 44);
    assert_eq!(
        first.next_cursor,
        u64::try_from(INPUT_EVENT_RING_CAPACITY + 44).expect("event count fits u64")
    );

    delivered.clear();
    let second = slot.read_events_since(first.next_cursor, &mut delivered);
    assert!(
        delivered.is_empty(),
        "an advanced cursor must not replay events"
    );
    assert_eq!(second.dropped, 0);
    assert_eq!(second.next_cursor, first.next_cursor);
}

#[test]
fn input_graph_rejects_zero_repeat_events_before_publication() {
    let event = InputEvent::Key {
        source_id: "invalid-repeat".into(),
        key: "KeyA".into(),
        state: InputButtonState::Pressed,
    };
    let mut manager = InputManager::new();
    manager.add_source(Box::new(EventfulSource {
        running: false,
        events: vec![
            TimedInputEvent {
                event: event.clone(),
                at_ms: 1,
                seq: 1,
                physical_code: None,
                repeat_count: 0,
            },
            TimedInputEvent {
                event,
                at_ms: 2,
                seq: 2,
                physical_code: None,
                repeat_count: 1,
            },
        ],
    }));
    manager.start_all().expect("event source should start");
    manager.sample_sources(1.0 / 60.0);

    let graph = manager.input_graph_handle().snapshot();
    let mut delivered = Vec::new();
    let read = graph.slots()[0].read_events_since(0, &mut delivered);

    assert_eq!(delivered.len(), 1);
    assert_eq!(delivered[0].seq, 2);
    assert_eq!(read.next_cursor, 1);
    assert_eq!(read.dropped, 0);
    assert_eq!(read.dropped_total, 1);
}

#[test]
fn input_graph_clears_absent_live_data_but_retains_change_only_snapshots() {
    let mut interaction = hypercolor_core::input::InteractionData::default();
    interaction.keyboard.pressed_keys.push("KeyA".to_owned());
    interaction.generation = 1;
    let media = Arc::new(hypercolor_types::media::MediaState::unavailable());
    let mut manager = InputManager::new();
    manager.add_source(Box::new(SequencedSource::new(
        [InputData::Interaction(interaction), InputData::None],
        true,
    )));
    manager.add_source(Box::new(SequencedSource::new(
        [InputData::Media(Arc::clone(&media)), InputData::None],
        false,
    )));
    manager.start_all().expect("sequenced sources should start");
    let graph = manager.input_graph_handle();

    manager.sample_sources(1.0 / 60.0);
    let first = graph.snapshot();
    assert!(matches!(
        first.slots()[0].latest().as_deref(),
        Some(InputData::Interaction(_))
    ));
    assert!(matches!(
        first.slots()[1].latest().as_deref(),
        Some(InputData::Media(_))
    ));

    manager.sample_sources(1.0 / 60.0);
    let second = graph.snapshot();
    assert!(second.slots()[0].latest().is_none());
    let Some(InputData::Media(retained)) = second.slots()[1].latest().as_deref().cloned() else {
        panic!("change-only media snapshot should remain published");
    };
    assert!(Arc::ptr_eq(&retained, &media));
}

#[test]
fn manager_restart_uses_a_new_canonical_graph_generation() {
    let mut manager = InputManager::new();
    manager.add_source(Box::new(BrowserInputSource::new()));
    let registry = manager.source_status_registry();

    manager.start_all().expect("browser source starts");
    let first = registry.snapshot().statuses()[0].clone();
    assert_eq!(first.state, SourceState::Live);
    manager.stop_all();
    assert_eq!(
        registry.snapshot().statuses()[0].state,
        SourceState::Stopped
    );

    manager.start_all().expect("browser source restarts");
    let second = registry.snapshot().statuses()[0].clone();
    assert_eq!(second.state, SourceState::Live);
    assert!(
        second.source_graph_generation > first.source_graph_generation,
        "manager restart must advance the canonical graph generation"
    );
    assert!(second.session_generation > first.session_generation);
}

#[test]
fn removed_source_fences_worker_owned_session() {
    let session_sink = Arc::new(Mutex::new(None));
    let mut manager = InputManager::new();
    manager.add_source(Box::new(StatusAwareScreenSource::new(Arc::clone(
        &session_sink,
    ))));
    manager.start_all().expect("status-aware source starts");
    let stale_worker = session_sink
        .lock()
        .expect("session sink lock")
        .clone()
        .expect("worker session was published");

    manager.remove_screen_sources();

    assert!(!stale_worker.failed(SourceIssue::new(
        "late_worker_exit",
        "retired workers cannot overwrite final status",
        false,
    )));
}

#[test]
fn production_source_constructors_expose_status_handles() {
    let config = AudioPipelineConfig::default();
    let mut sources: Vec<Box<dyn InputSource>> = vec![
        Box::new(AudioInput::new(&config)),
        Box::new(BrowserInputSource::new()),
        Box::new(MediaSource::new()),
        Box::new(NetSource::new()),
        Box::new(ScreenCaptureInput::new(ScreenCaptureConfig::default())),
    ];
    for source in &mut sources {
        assert!(
            source.source_status_handle().is_some(),
            "{} omitted its production status handle",
            source.name()
        );
        let source_name = source.name().to_owned();
        assert!(
            source.source_status_reporter().is_some(),
            "{source_name} omitted its production status reporter"
        );
    }

    #[cfg(target_os = "windows")]
    {
        let mut windows_sources: Vec<Box<dyn InputSource>> = vec![
            Box::new(WindowsScreenCaptureInput::new(
                ScreenCaptureConfig::default(),
            )),
            Box::new(WindowsHostInput::new(true, true)),
        ];
        for source in &mut windows_sources {
            assert!(
                source.source_status_handle().is_some(),
                "{} omitted its production status handle",
                source.name()
            );
            let source_name = source.name().to_owned();
            assert!(
                source.source_status_reporter().is_some(),
                "{source_name} omitted its production status reporter"
            );
        }
    }

    #[cfg(target_os = "linux")]
    {
        use hypercolor_core::input::EvdevHostInput;

        let mut linux_sources: Vec<Box<dyn InputSource>> = vec![
            Box::new(EvdevHostInput::new(true, true)),
            Box::new(WaylandScreenCaptureInput::new(CaptureConfig::default())),
        ];
        for source in &mut linux_sources {
            assert!(
                source.source_status_handle().is_some(),
                "{} omitted its production status handle",
                source.name()
            );
            let source_name = source.name().to_owned();
            assert!(
                source.source_status_reporter().is_some(),
                "{source_name} omitted its production status reporter"
            );
        }
    }

    #[cfg(target_os = "macos")]
    {
        let mut source = hypercolor_core::input::InteractionInput::new();
        assert!(source.source_status_handle().is_some());
        assert!(source.source_status_reporter().is_some());
    }
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
#[test]
fn unsupported_media_platform_reports_structured_unavailable_status() {
    let mut manager = InputManager::new();
    manager.add_source(Box::new(MediaSource::new()));
    let registry = manager.source_status_registry();
    manager
        .start_all()
        .expect("unsupported media stub starts inertly");

    let statuses = registry.snapshot().statuses();
    assert_eq!(statuses.len(), 1);
    assert_eq!(statuses[0].state, SourceState::Unavailable);
    let issue = statuses[0]
        .issue
        .as_ref()
        .expect("platform stub publishes a structured issue");
    assert_eq!(issue.code.as_ref(), "media_backend_unsupported");
    assert!(issue.remediation.is_some());
}

#[test]
fn manager_samples_multiple_sources() {
    let mut mgr = InputManager::new();
    mgr.add_source(Box::new(MockAudioSource::new(0.5)));
    mgr.add_source(Box::new(MockScreenSource::new(2)));

    assert_eq!(mgr.source_count(), 2);
    assert_eq!(mgr.source_names(), vec!["MockAudio", "MockScreen"]);

    let samples = mgr.sample_all();
    assert_eq!(samples.len(), 2);
    assert!(matches!(&samples[0], InputData::Audio(_)));
    assert!(matches!(&samples[1], InputData::Screen(_)));
}

#[cfg(target_os = "linux")]
#[test]
fn sensor_poller_does_not_cache_process_stat_fds() {
    let before = per_process_stat_fd_count();
    let mut poller = SensorPoller::with_interval(Duration::from_millis(20));
    let mut rx = poller.receiver();

    poller.start().expect("sensor poller should start");
    assert!(
        wait_for_sensor_snapshot(&mut rx, Duration::from_secs(1)),
        "sensor poller should publish at least one snapshot",
    );

    let after = per_process_stat_fd_count();
    poller.stop();

    assert!(
        after <= before.saturating_add(8),
        "sensor poller should not retain /proc/<pid>/stat fds (before={before}, after={after})",
    );
}

#[test]
fn manager_start_all_succeeds() {
    let mut mgr = InputManager::new();
    mgr.add_source(Box::new(MockAudioSource::new(0.1)));
    mgr.add_source(Box::new(MockScreenSource::new(1)));

    mgr.start_all().expect("start_all should succeed");
}

#[cfg(target_os = "linux")]
fn per_process_stat_fd_count() -> usize {
    fs::read_dir("/proc/self/fd")
        .expect("fd directory should be readable")
        .filter_map(Result::ok)
        .filter_map(|entry| fs::read_link(entry.path()).ok())
        .filter_map(|target| target.into_os_string().into_string().ok())
        .filter(|target| is_process_stat_path(target))
        .count()
}

#[cfg(target_os = "linux")]
fn wait_for_sensor_snapshot(
    rx: &mut tokio::sync::watch::Receiver<Arc<hypercolor_types::sensor::SystemSnapshot>>,
    timeout: Duration,
) -> bool {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("test runtime should build");
    runtime.block_on(async {
        matches!(
            tokio::time::timeout(timeout, rx.changed()).await,
            Ok(Ok(()))
        )
    })
}

#[cfg(target_os = "linux")]
fn is_process_stat_path(target: &str) -> bool {
    let Some(remainder) = target.strip_prefix("/proc/") else {
        return false;
    };

    let Some(pid) = remainder.strip_suffix("/stat") else {
        return false;
    };

    if pid.bytes().all(|byte| byte.is_ascii_digit()) {
        return true;
    }

    let Some((task_owner, task_id)) = pid.split_once("/task/") else {
        return false;
    };
    task_owner.bytes().all(|byte| byte.is_ascii_digit())
        && task_id.bytes().all(|byte| byte.is_ascii_digit())
}

#[test]
fn manager_start_all_rolls_back_on_failure() {
    let mut mgr = InputManager::new();

    // First source starts fine, second will fail.
    mgr.add_source(Box::new(MockAudioSource::new(0.1)));
    mgr.add_source(Box::new(FailingSource));

    let result = mgr.start_all();
    assert!(result.is_err());
}

#[test]
fn manager_stop_all_is_idempotent() {
    let mut mgr = InputManager::new();
    mgr.add_source(Box::new(MockAudioSource::new(0.1)));
    mgr.start_all().expect("start should succeed");

    // Stop twice — should not panic or error.
    mgr.stop_all();
    mgr.stop_all();
}

#[test]
fn manager_sample_gracefully_handles_errors() {
    let mut mgr = InputManager::new();
    mgr.add_source(Box::new(MockAudioSource::new(0.8)));
    mgr.add_source(Box::new(FaultySampleSource::new()));
    mgr.add_source(Box::new(MockScreenSource::new(1)));

    let samples = mgr.sample_all();
    assert_eq!(samples.len(), 3);

    // First source: audio data.
    assert!(matches!(&samples[0], InputData::Audio(_)));
    // Second source: graceful fallback to None.
    assert!(matches!(&samples[1], InputData::None));
    // Third source: screen data still works.
    assert!(matches!(&samples[2], InputData::Screen(_)));
}

#[test]
fn manager_sample_preserves_source_order() {
    let mut mgr = InputManager::new();
    mgr.add_source(Box::new(MockScreenSource::new(5)));
    mgr.add_source(Box::new(MockAudioSource::new(0.33)));
    mgr.add_source(Box::new(MockScreenSource::new(1)));

    let samples = mgr.sample_all();
    assert_eq!(samples.len(), 3);
    assert!(matches!(&samples[0], InputData::Screen(_)));
    assert!(matches!(&samples[1], InputData::Audio(_)));
    assert!(matches!(&samples[2], InputData::Screen(_)));
}

#[test]
fn manager_sample_all_with_delta_secs_uses_timing_aware_sources() {
    let mut mgr = InputManager::new();
    mgr.add_source(Box::new(DeltaAwareSource::new()));

    let samples = mgr.sample_all_with_delta_secs(0.25);
    assert_eq!(samples.len(), 1);

    match &samples[0] {
        InputData::Audio(audio) => {
            assert!((audio.rms_level - 0.25).abs() < f32::EPSILON);
        }
        _ => panic!("expected audio data"),
    }
}

#[test]
fn manager_drains_discrete_input_events_from_all_sources() {
    let mut mgr = InputManager::new();
    mgr.add_source(Box::new(EventfulSource::new(vec![InputEvent::Key {
        source_id: "host:/dev/input/event4".into(),
        key: "Space".into(),
        state: InputButtonState::Pressed,
    }])));
    mgr.add_source(Box::new(EventfulSource::new(vec![
        InputEvent::MidiRealtime {
            source_id: "midi:clock".into(),
            message: hypercolor_core::types::event::MidiRealtimeMessage::Clock,
        },
    ])));
    mgr.start_all().expect("eventful sources should start");

    let first = mgr.drain_events();
    assert_eq!(first.len(), 2);

    let second = mgr.drain_events();
    assert!(second.is_empty(), "events should drain exactly once");
}

#[test]
fn manager_does_not_drain_events_from_stopped_sources() {
    let mut mgr = InputManager::new();
    mgr.add_source(Box::new(EventfulSource::new(vec![InputEvent::Key {
        source_id: "host:/dev/input/event4".into(),
        key: "Space".into(),
        state: InputButtonState::Pressed,
    }])));
    mgr.start_all().expect("eventful source should start");
    mgr.stop_all();

    assert!(mgr.drain_events().is_empty());
}

#[test]
fn audio_data_values_propagate_through_input_data() {
    let mut mgr = InputManager::new();
    mgr.add_source(Box::new(MockAudioSource::new(0.99)));

    let samples = mgr.sample_all();
    assert_eq!(samples.len(), 1);

    match &samples[0] {
        InputData::Audio(audio) => {
            assert!((audio.rms_level - 0.99).abs() < f32::EPSILON);
            // Silence defaults for everything else.
            assert!((audio.bpm - 0.0).abs() < f32::EPSILON);
            assert!(!audio.onset_detected);
        }
        _ => panic!("expected audio data"),
    }
}

#[test]
fn screen_data_empty_zones_is_valid() {
    let mut src = MockScreenSource::new(0);
    let data = src.sample().expect("sample should succeed");

    match data {
        InputData::Screen(screen) => {
            assert!(screen.zone_colors.is_empty());
        }
        _ => panic!("expected InputData::Screen"),
    }
}

#[test]
fn manager_reconfigures_existing_audio_source_live() {
    let mut mgr = InputManager::new();
    mgr.add_source(Box::new(ReconfigurableAudioSource::new()));

    let config = AudioPipelineConfig {
        source: AudioSourceType::Named("microphone".to_owned()),
        ..AudioPipelineConfig::default()
    };

    mgr.apply_audio_runtime_config(true, &config, "AudioInput(microphone)", false)
        .expect("audio reconfigure should succeed");

    assert_eq!(mgr.source_count(), 1);
    assert_eq!(mgr.source_names(), vec!["AudioInput(microphone)"]);

    let samples = mgr.sample_all();
    match &samples[0] {
        InputData::Audio(audio) => assert!((audio.rms_level - 0.0).abs() < f32::EPSILON),
        _ => panic!("expected audio data"),
    }

    mgr.set_audio_capture_active(true)
        .expect("audio capture demand update should succeed");

    let samples = mgr.sample_all();
    match &samples[0] {
        InputData::Audio(audio) => assert!((audio.rms_level - 0.5).abs() < f32::EPSILON),
        _ => panic!("expected audio data"),
    }
}

#[test]
fn prepared_audio_reconfiguration_commits_after_native_work_is_complete() {
    let config = AudioPipelineConfig {
        source: AudioSourceType::None,
        ..AudioPipelineConfig::default()
    };
    let mut manager = InputManager::new();
    manager.add_source(Box::new(AudioInput::new(&config).with_name("audio-before")));
    manager.start_all().expect("audio source should start");

    let plan = manager
        .plan_audio_runtime_config(false, &config, "audio-after", false)
        .expect("production audio source supports preparation");
    let mut prepared = plan.prepare().expect("disabled audio preparation is local");
    manager
        .commit_audio_runtime_config(&mut prepared)
        .expect("unchanged graph should accept prepared audio")
        .retire();

    assert_eq!(manager.source_names(), vec!["audio-after"]);
}

#[test]
fn prepared_audio_enable_while_demanded_registers_a_live_source() {
    let config = AudioPipelineConfig {
        source: AudioSourceType::Microphone,
        ..AudioPipelineConfig::default()
    };
    let mut manager = InputManager::new();
    let mut prepared = manager
        .plan_audio_runtime_config(true, &config, "audio-active", true)
        .expect("absent audio source should support preparation")
        .prepare_with_synthetic_capture_for_testing()
        .expect("synthetic capture should stage without hardware");

    manager
        .commit_audio_runtime_config(&mut prepared)
        .expect("active prepared audio source should commit")
        .retire();

    assert_eq!(manager.source_names(), vec!["audio-active"]);
    let statuses = manager.source_status_registry().snapshot().statuses();
    assert_eq!(statuses.len(), 1);
    assert!(statuses[0].configured);
    assert!(statuses[0].demanded);
    assert_eq!(statuses[0].state, SourceState::Starting);
    assert_eq!(
        statuses[0].source_graph_generation,
        manager.source_graph_generation()
    );
    manager.stop_all();
}

#[test]
fn prepared_audio_reconfiguration_rejects_a_changed_input_graph() {
    let config = AudioPipelineConfig {
        source: AudioSourceType::None,
        ..AudioPipelineConfig::default()
    };
    let mut manager = InputManager::new();
    manager.add_source(Box::new(AudioInput::new(&config).with_name("audio-before")));
    manager.start_all().expect("audio source should start");
    let mut prepared = manager
        .plan_audio_runtime_config(false, &config, "audio-after", false)
        .expect("production audio source supports preparation")
        .prepare()
        .expect("disabled audio preparation is local");

    manager.add_source(Box::new(MockScreenSource::new(1)));
    let error = manager
        .commit_audio_runtime_config(&mut prepared)
        .expect_err("graph changes must fence a stale prepared runtime");

    assert!(matches!(
        error.downcast_ref::<AudioReconfigurationConflict>(),
        Some(AudioReconfigurationConflict::GraphChanged)
    ));
    assert!(error.to_string().contains("input graph changed"));
    assert_eq!(manager.source_names()[0], "audio-before");
}

#[test]
fn prepared_audio_reconfiguration_rejects_changed_capture_demand() {
    let config = AudioPipelineConfig {
        source: AudioSourceType::None,
        ..AudioPipelineConfig::default()
    };
    let mut manager = InputManager::new();
    manager
        .set_audio_capture_active(false)
        .expect("initial demand should be cached");
    let mut prepared = manager
        .plan_audio_runtime_config(false, &config, "audio-disabled", false)
        .expect("absent audio source supports disabled preparation")
        .prepare()
        .expect("disabled audio preparation is local");

    manager
        .set_audio_capture_active(true)
        .expect("concurrent demand should update the graph generation");
    let error = manager
        .commit_audio_runtime_config(&mut prepared)
        .expect_err("capture demand changes must fence a stale prepared runtime");

    assert!(error.to_string().contains("input graph changed"));
    assert_eq!(manager.source_count(), 0);
}

#[test]
fn prepared_audio_reconfiguration_rejects_terminal_failure_before_commit() {
    let config = AudioPipelineConfig {
        source: AudioSourceType::None,
        ..AudioPipelineConfig::default()
    };
    let mut manager = InputManager::new();
    manager.add_source(Box::new(AudioInput::new(&config).with_name("audio-before")));
    manager.start_all().expect("audio source should start");
    let mut prepared = manager
        .plan_audio_runtime_config(false, &config, "audio-after", false)
        .expect("production audio source supports preparation")
        .prepare()
        .expect("disabled audio preparation is local");
    prepared.fail_before_commit_for_testing();

    let error = manager
        .commit_audio_runtime_config(&mut prepared)
        .expect_err("a terminal staged failure must preserve the committed source");

    assert!(error.to_string().contains("failed before commit"));
    assert_eq!(manager.source_names(), vec!["audio-before"]);
}

#[test]
fn disabling_absent_audio_fences_an_older_enable_plan() {
    let disabled = AudioPipelineConfig {
        source: AudioSourceType::None,
        ..AudioPipelineConfig::default()
    };
    let enabled = AudioPipelineConfig {
        source: AudioSourceType::Microphone,
        ..AudioPipelineConfig::default()
    };
    let mut manager = InputManager::new();
    let initial_generation = manager.source_graph_generation();
    let older_enable = manager
        .plan_audio_runtime_config(true, &enabled, "audio-enabled", false)
        .expect("absent audio source supports prepared enable")
        .prepare()
        .expect("idle enable does not open native capture");
    let mut newer_disable = manager
        .plan_audio_runtime_config(false, &disabled, "audio-disabled", false)
        .expect("absent audio source supports prepared disable")
        .prepare()
        .expect("disable preparation is local");

    manager
        .commit_audio_runtime_config(&mut newer_disable)
        .expect("newer disable should commit")
        .retire();
    assert!(manager.source_graph_generation() > initial_generation);

    let mut older_enable = older_enable;
    let error = manager
        .commit_audio_runtime_config(&mut older_enable)
        .expect_err("newer disable must fence an older enable plan");
    assert!(matches!(
        error.downcast_ref::<AudioReconfigurationConflict>(),
        Some(AudioReconfigurationConflict::GraphChanged)
    ));
    assert_eq!(manager.source_count(), 0);
}

#[test]
fn manager_adds_audio_source_when_live_audio_is_enabled() {
    let mut mgr = InputManager::new();

    let config = AudioPipelineConfig {
        source: AudioSourceType::None,
        ..AudioPipelineConfig::default()
    };

    mgr.apply_audio_runtime_config(false, &config, "AudioInput(none)", false)
        .expect("disabling absent audio source should be a no-op");
    assert_eq!(mgr.source_count(), 0);

    let config = AudioPipelineConfig {
        source: AudioSourceType::Microphone,
        ..AudioPipelineConfig::default()
    };
    mgr.apply_audio_runtime_config(true, &config, "AudioInput(microphone)", false)
        .expect("enabling audio should add a source");

    assert_eq!(mgr.source_count(), 1);
    assert_eq!(mgr.source_names(), vec!["AudioInput(microphone)"]);
}

#[test]
fn manager_forces_capture_inactive_when_live_audio_is_disabled() {
    let mut mgr = InputManager::new();
    mgr.add_source(Box::new(CaptureTrackingAudioSource::new()));

    let config = AudioPipelineConfig {
        source: AudioSourceType::None,
        ..AudioPipelineConfig::default()
    };

    mgr.apply_audio_runtime_config(false, &config, "AudioInput(none)", true)
        .expect("disabling audio should clear capture demand");

    let samples = mgr.sample_all();
    match &samples[0] {
        InputData::Audio(audio) => assert!((audio.rms_level - 0.0).abs() < f32::EPSILON),
        _ => panic!("expected audio data"),
    }
}

#[test]
fn manager_reenables_existing_audio_source_after_live_disable() {
    let mut mgr = InputManager::new();
    mgr.add_source(Box::new(ReconfigurableAudioSource::new()));

    let enabled_config = AudioPipelineConfig {
        source: AudioSourceType::Named("microphone".to_owned()),
        ..AudioPipelineConfig::default()
    };

    mgr.apply_audio_runtime_config(true, &enabled_config, "AudioInput(microphone)", true)
        .expect("enabling audio should succeed");

    let samples = mgr.sample_all();
    match &samples[0] {
        InputData::Audio(audio) => assert!((audio.rms_level - 0.5).abs() < f32::EPSILON),
        _ => panic!("expected audio data"),
    }

    let disabled_config = AudioPipelineConfig {
        source: AudioSourceType::None,
        ..enabled_config.clone()
    };

    mgr.apply_audio_runtime_config(false, &disabled_config, "AudioInput(microphone)", true)
        .expect("disabling audio should keep the existing source registered");

    let samples = mgr.sample_all();
    match &samples[0] {
        InputData::Audio(audio) => assert!((audio.rms_level - 0.0).abs() < f32::EPSILON),
        _ => panic!("expected audio data"),
    }

    mgr.apply_audio_runtime_config(true, &enabled_config, "AudioInput(microphone)", true)
        .expect("re-enabling audio should restore live capture");

    let samples = mgr.sample_all();
    match &samples[0] {
        InputData::Audio(audio) => assert!((audio.rms_level - 0.5).abs() < f32::EPSILON),
        _ => panic!("expected audio data"),
    }
}

#[test]
fn audio_runtime_reconfiguration_binds_each_status_session_to_a_new_graph_generation() {
    let mut manager = InputManager::new();
    manager.add_source(Box::new(StatusReconfigurableAudioSource::new()));
    let registry = manager.source_status_registry();
    let config_a = AudioPipelineConfig {
        source: AudioSourceType::Named("source-a".to_owned()),
        ..AudioPipelineConfig::default()
    };

    manager
        .apply_audio_runtime_config(true, &config_a, "source-a", true)
        .expect("first live audio source should bind");
    let first = registry.snapshot().statuses()[0].clone();
    assert_eq!(first.state, SourceState::Live);
    assert_eq!(
        first.source_graph_generation,
        manager.source_graph_generation()
    );

    manager
        .apply_audio_runtime_config(false, &config_a, "source-a", true)
        .expect("audio disable should fence the active session");
    let disabled = registry.snapshot().statuses()[0].clone();
    assert_eq!(disabled.state, SourceState::Stopped);
    assert!(!disabled.demanded);

    manager
        .apply_audio_runtime_config(true, &config_a, "source-a", true)
        .expect("audio re-enable should start a successor session");
    let reenabled = registry.snapshot().statuses()[0].clone();
    assert!(
        reenabled.source_graph_generation > first.source_graph_generation,
        "disable -> re-enable must not reuse the old graph generation"
    );
    assert!(reenabled.session_generation > first.session_generation);

    let config_b = AudioPipelineConfig {
        source: AudioSourceType::Named("source-b".to_owned()),
        ..config_a
    };
    manager
        .apply_audio_runtime_config(true, &config_b, "source-b", true)
        .expect("active source replacement should start a successor session");
    let replaced = registry.snapshot().statuses()[0].clone();
    assert!(replaced.source_graph_generation > reenabled.source_graph_generation);
    assert!(replaced.session_generation > reenabled.session_generation);
    assert_eq!(
        replaced.source_graph_generation,
        manager.source_graph_generation()
    );
}

#[test]
fn manager_updates_screen_capture_demand_for_screen_sources() {
    let mut mgr = InputManager::new();
    mgr.add_source(Box::new(CaptureTrackingScreenSource::new()));
    mgr.start_all().expect("start_all should succeed");

    let samples = mgr.sample_all();
    assert!(matches!(&samples[0], InputData::None));

    mgr.set_screen_capture_active(true)
        .expect("screen capture demand update should succeed");

    let samples = mgr.sample_all();
    assert!(matches!(&samples[0], InputData::Screen(_)));

    mgr.set_screen_capture_active(false)
        .expect("screen capture demand reset should succeed");

    let samples = mgr.sample_all();
    assert!(matches!(&samples[0], InputData::None));
}

#[test]
fn screen_source_added_while_demanded_is_activated_once_on_next_graph_generation() {
    let activations = Arc::new(AtomicUsize::new(0));
    let mut manager = InputManager::new();
    manager
        .set_screen_capture_active(true)
        .expect("empty graph should accept screen demand");
    let demanded_generation = manager.source_graph_generation();

    manager.add_source(Box::new(CountingScreenDemandSource::new(Arc::clone(
        &activations,
    ))));
    manager
        .start_all()
        .expect("newly registered screen source should start");
    assert!(manager.source_graph_generation() > demanded_generation);
    manager
        .set_screen_capture_active(true)
        .expect("graph change should reconcile existing demand");
    manager
        .set_screen_capture_active(true)
        .expect("stable graph demand should be cached");

    assert_eq!(activations.load(Ordering::Relaxed), 1);
}

#[test]
fn screen_capture_demand_rolls_back_every_source_and_publishes_one_completed_epoch() {
    let states: Vec<_> = (0..3)
        .map(|_| Arc::new(Mutex::new(DemandTransitionState::default())))
        .collect();
    let mut manager = InputManager::new();
    manager.add_source(Box::new(FallibleStatusScreenSource::new(
        "screen-first",
        None,
        Arc::clone(&states[0]),
    )));
    manager.add_source(Box::new(FallibleStatusScreenSource::new(
        "screen-failing",
        Some(true),
        Arc::clone(&states[1]),
    )));
    manager.add_source(Box::new(FallibleStatusScreenSource::new(
        "screen-unattempted",
        None,
        Arc::clone(&states[2]),
    )));
    manager
        .start_all()
        .expect("fallible screen sources start idle");
    let registry = manager.source_status_registry();
    let before = manager.source_graph_generation();

    let error = manager
        .set_screen_capture_active(true)
        .expect_err("second source injects an activation failure");
    assert!(
        error
            .to_string()
            .contains("injected demand transition failure")
    );
    assert_eq!(manager.source_graph_generation(), before + 1);
    assert_eq!(
        registry.snapshot().source_graph_generation(),
        manager.source_graph_generation()
    );
    for state in &states {
        assert!(!state.lock().expect("demand state lock").active);
    }
    assert_eq!(
        states[0].lock().expect("first state lock").transitions,
        vec![true, false]
    );
    assert_eq!(
        states[1].lock().expect("failing state lock").transitions,
        vec![true, false]
    );
    assert_eq!(
        states[2]
            .lock()
            .expect("unattempted state lock")
            .transitions,
        vec![false],
        "rollback covers sources not reached before the failure"
    );
    assert!(
        registry
            .snapshot()
            .statuses()
            .iter()
            .all(|status| !status.demanded && status.state == SourceState::Stopped)
    );

    manager
        .set_screen_capture_active(false)
        .expect("rolled-back demand cache remains false");
    assert_eq!(
        states[0].lock().expect("first state lock").transitions,
        vec![true, false],
        "a false -> false request must be a cache no-op"
    );
}

#[test]
fn screen_capture_deactivation_failure_restores_live_demand_and_cache() {
    let states: Vec<_> = (0..3)
        .map(|_| Arc::new(Mutex::new(DemandTransitionState::default())))
        .collect();
    let mut manager = InputManager::new();
    manager.add_source(Box::new(FallibleStatusScreenSource::new(
        "deactivate-first",
        None,
        Arc::clone(&states[0]),
    )));
    manager.add_source(Box::new(FallibleStatusScreenSource::new(
        "deactivate-failing",
        Some(false),
        Arc::clone(&states[1]),
    )));
    manager.add_source(Box::new(FallibleStatusScreenSource::new(
        "deactivate-unattempted",
        None,
        Arc::clone(&states[2]),
    )));
    manager.start_all().expect("screen sources start idle");
    manager
        .set_screen_capture_active(true)
        .expect("initial activation succeeds");
    let registry = manager.source_status_registry();

    manager
        .set_screen_capture_active(false)
        .expect_err("second source injects a deactivation failure");
    assert!(
        states
            .iter()
            .all(|state| state.lock().expect("demand state lock").active)
    );
    assert_eq!(
        states[0].lock().expect("first state lock").transitions,
        vec![true, false, true]
    );
    assert_eq!(
        states[1].lock().expect("failing state lock").transitions,
        vec![true, false, true]
    );
    assert_eq!(
        states[2]
            .lock()
            .expect("unattempted state lock")
            .transitions,
        vec![true, true]
    );
    assert!(
        registry
            .snapshot()
            .statuses()
            .iter()
            .all(|status| status.demanded)
    );

    manager
        .set_screen_capture_active(true)
        .expect("rolled-back demand cache remains true");
    assert_eq!(
        states[0].lock().expect("first state lock").transitions,
        vec![true, false, true],
        "a true -> true request must be a cache no-op"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn wayland_screen_capture_input_stays_idle_without_capture_demand() {
    let mut src = WaylandScreenCaptureInput::new(CaptureConfig::default());
    assert_eq!(src.name(), "wayland_screen_capture");

    src.start().expect("start should succeed while idle");
    assert!(matches!(
        src.sample().expect("sample should succeed"),
        InputData::None
    ));

    src.stop();
    assert!(!src.is_running());
}

// ── Screen Capture Live Reconfiguration ──────────────────────────────────

#[derive(Default)]
struct ScreenReconfigLog {
    applied_configs: Vec<ScreenCaptureConfig>,
    reselect_count: u32,
}

struct ReconfigurableScreenSource {
    running: bool,
    log: Arc<Mutex<ScreenReconfigLog>>,
}

impl ReconfigurableScreenSource {
    fn new(log: Arc<Mutex<ScreenReconfigLog>>) -> Self {
        Self {
            running: false,
            log,
        }
    }
}

impl InputSource for ReconfigurableScreenSource {
    fn name(&self) -> &'static str {
        "ReconfigurableScreen"
    }

    fn start(&mut self) -> anyhow::Result<()> {
        self.running = true;
        Ok(())
    }

    fn stop(&mut self) {
        self.running = false;
    }

    fn sample(&mut self) -> anyhow::Result<InputData> {
        Ok(InputData::None)
    }

    fn is_running(&self) -> bool {
        self.running
    }

    fn is_screen_source(&self) -> bool {
        true
    }

    fn reconfigure_screen_capture(&mut self, config: &ScreenCaptureConfig) -> anyhow::Result<()> {
        self.log
            .lock()
            .expect("reconfig log lock")
            .applied_configs
            .push(config.clone());
        Ok(())
    }

    fn reselect_screen_source(&mut self) -> anyhow::Result<()> {
        self.log.lock().expect("reconfig log lock").reselect_count += 1;
        Ok(())
    }
}

#[test]
fn input_manager_routes_screen_capture_reconfiguration() {
    let log = Arc::new(Mutex::new(ScreenReconfigLog::default()));
    let mut manager = InputManager::new();
    manager.add_source(Box::new(ReconfigurableScreenSource::new(Arc::clone(&log))));
    manager.add_source(Box::new(MockAudioSource::new(0.5)));
    assert!(manager.has_screen_source());

    let config = ScreenCaptureConfig {
        grid_cols: 16,
        grid_rows: 9,
        ..ScreenCaptureConfig::default()
    };
    manager
        .reconfigure_screen_capture(&config)
        .expect("reconfigure should route to screen sources");
    manager
        .reselect_screen_source()
        .expect("reselect should route to screen sources");

    let observed = log.lock().expect("reconfig log lock");
    assert_eq!(observed.applied_configs.len(), 1);
    assert_eq!(observed.applied_configs[0].grid_cols, 16);
    assert_eq!(observed.applied_configs[0].grid_rows, 9);
    assert_eq!(observed.reselect_count, 1);
}

#[test]
fn screen_restart_operations_start_sessions_under_the_bound_graph_generation() {
    let sessions = Arc::new(Mutex::new(Vec::new()));
    let mut manager = InputManager::new();
    manager.add_source(Box::new(RestartingStatusScreenSource::new(Arc::clone(
        &sessions,
    ))));
    let registry = manager.source_status_registry();
    manager.start_all().expect("status screen source starts");
    let started = registry.snapshot().statuses()[0].clone();
    assert_eq!(sessions.lock().expect("session log lock").len(), 1);

    let analysis_only = ScreenCaptureConfig {
        grid_cols: 8,
        ..ScreenCaptureConfig::default()
    };
    manager
        .reconfigure_screen_capture(&analysis_only)
        .expect("analysis-only settings apply without restart");
    let unchanged_session = registry.snapshot().statuses()[0].clone();
    assert_eq!(
        unchanged_session.session_generation,
        started.session_generation
    );

    let restart = ScreenCaptureConfig {
        target_fps: analysis_only.target_fps + 1,
        ..analysis_only
    };
    manager
        .reconfigure_screen_capture(&restart)
        .expect("FPS change restarts the source session");
    let fps_session = registry.snapshot().statuses()[0].clone();
    assert!(fps_session.session_generation > started.session_generation);
    assert_eq!(
        fps_session.source_graph_generation,
        manager.source_graph_generation()
    );

    manager
        .reselect_screen_source()
        .expect("source reselect starts another session");
    let reselected = registry.snapshot().statuses()[0].clone();
    assert!(reselected.session_generation > fps_session.session_generation);
    assert_eq!(
        reselected.source_graph_generation,
        manager.source_graph_generation()
    );
    assert_eq!(sessions.lock().expect("session log lock").len(), 3);
}

#[test]
fn screen_status_uses_frame_acquisition_time_not_consumer_read_time() {
    let mut input = ScreenCaptureInput::new(ScreenCaptureConfig::default());
    let handle = input
        .source_status_handle()
        .expect("production screen source exposes status");
    input.set_source_graph_generation(1);
    input.start().expect("screen analysis starts");
    let frame = vec![96_u8; 64 * 64 * 4];
    let before_push = Instant::now();
    input.push_frame(&frame, 64, 64);
    let after_push = Instant::now();
    std::thread::sleep(Duration::from_millis(100));

    assert!(matches!(
        input.sample().expect("delayed screen sample succeeds"),
        InputData::Screen(_)
    ));
    let status = handle.snapshot();
    let acquired_at = status
        .last_sample_at
        .expect("screen acquisition timestamp was recorded");
    assert!(acquired_at >= before_push && acquired_at <= after_push);
    assert_eq!(status.freshness, SourceFreshness::Stale);
}

#[test]
fn input_manager_removes_screen_sources() {
    let log = Arc::new(Mutex::new(ScreenReconfigLog::default()));
    let mut manager = InputManager::new();
    manager.add_source(Box::new(ReconfigurableScreenSource::new(log)));
    manager.add_source(Box::new(MockAudioSource::new(0.5)));
    assert!(manager.has_screen_source());

    manager.remove_screen_sources();
    assert!(!manager.has_screen_source());
    assert!(
        manager.source_names().contains(&"MockAudio".to_owned()),
        "non-screen sources must survive removal"
    );
}

// ── Interaction capture demand ─────────────────────────────────────────────

struct CaptureTrackingInteractionSource {
    running: bool,
    capture_active: bool,
    transitions: std::sync::Arc<std::sync::Mutex<Vec<bool>>>,
    status: SourceStatusReporter,
}

impl CaptureTrackingInteractionSource {
    fn new(transitions: std::sync::Arc<std::sync::Mutex<Vec<bool>>>) -> Self {
        Self {
            running: false,
            capture_active: false,
            transitions,
            status: SourceStatusReporter::new(
                "capture_tracking_interaction",
                SourceKind::Interaction,
                "test",
                true,
                true,
                false,
            ),
        }
    }
}

impl InputSource for CaptureTrackingInteractionSource {
    fn name(&self) -> &'static str {
        "CaptureTrackingInteraction"
    }

    fn source_status_handle(&self) -> Option<SourceStatusHandle> {
        Some(self.status.handle())
    }

    fn source_status_reporter(&mut self) -> Option<&mut SourceStatusReporter> {
        Some(&mut self.status)
    }

    fn start(&mut self) -> anyhow::Result<()> {
        self.running = true;
        if self.capture_active {
            self.status.begin_session()?;
        }
        Ok(())
    }

    fn stop(&mut self) {
        self.status.stop();
        self.running = false;
    }

    fn sample(&mut self) -> anyhow::Result<InputData> {
        Ok(InputData::None)
    }

    fn is_running(&self) -> bool {
        self.running
    }

    fn is_interaction_source(&self) -> bool {
        true
    }

    fn is_host_capture_source(&self) -> bool {
        true
    }

    fn set_interaction_capture_active(&mut self, active: bool) -> anyhow::Result<()> {
        if self.capture_active != active {
            self.transitions
                .lock()
                .expect("transitions lock")
                .push(active);
        }
        self.capture_active = active;
        self.status.set_policy(true, true, active)?;
        if active && self.running {
            self.status.begin_session()?;
        }
        Ok(())
    }
}

#[test]
fn manager_routes_interaction_capture_demand_to_interaction_sources_only() {
    let transitions = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut mgr = InputManager::new();
    mgr.add_source(Box::new(CaptureTrackingInteractionSource::new(
        std::sync::Arc::clone(&transitions),
    )));
    mgr.add_source(Box::new(MockAudioSource::new(0.5)));

    mgr.set_interaction_capture_active(true)
        .expect("activate interaction capture");
    mgr.set_interaction_capture_active(true)
        .expect("repeat activation is a no-op");
    mgr.set_interaction_capture_active(false)
        .expect("deactivate interaction capture");

    assert_eq!(
        *transitions.lock().expect("transitions lock"),
        vec![true, false]
    );
}

#[test]
fn interaction_demand_reconciles_false_after_stop_and_restart() {
    let transitions = Arc::new(Mutex::new(Vec::new()));
    let mut manager = InputManager::new();
    manager.add_source(Box::new(CaptureTrackingInteractionSource::new(Arc::clone(
        &transitions,
    ))));
    let registry = manager.source_status_registry();
    manager.start_all().expect("interaction source starts");
    manager
        .set_interaction_capture_active(true)
        .expect("host capture activates");

    manager.stop_all();
    manager.start_all().expect("interaction source restarts");
    manager
        .set_interaction_capture_active(false)
        .expect("unknown demand cache reconciles false");

    assert_eq!(
        *transitions.lock().expect("transitions lock"),
        vec![true, false]
    );
    let status = registry.snapshot().statuses()[0].clone();
    assert!(!status.demanded);
    assert_eq!(status.state, SourceState::Stopped);
}

#[test]
fn production_audio_demand_reconciles_false_after_stop_and_restart() {
    let config = AudioPipelineConfig {
        source: AudioSourceType::Named("__hypercolor_demand_reconciliation_missing__".to_owned()),
        ..AudioPipelineConfig::default()
    };
    let mut manager = InputManager::new();
    manager.add_source(Box::new(AudioInput::new(&config)));
    let registry = manager.source_status_registry();

    manager.start_all().expect("audio source starts idle");
    manager
        .set_audio_capture_active(true)
        .expect("missing capture degrades without rejecting demand");
    assert!(registry.snapshot().statuses()[0].demanded);

    manager.stop_all();
    manager.start_all().expect("audio source restarts idle");
    manager
        .set_audio_capture_active(false)
        .expect("unknown demand cache reconciles false");

    let status = registry.snapshot().statuses()[0].clone();
    assert!(!status.demanded);
    assert_eq!(status.state, SourceState::Stopped);
}

#[test]
fn adding_an_interaction_source_invalidates_and_reapplies_live_demand() {
    let first = Arc::new(Mutex::new(Vec::new()));
    let second = Arc::new(Mutex::new(Vec::new()));
    let mut manager = InputManager::new();
    manager.add_source(Box::new(CaptureTrackingInteractionSource::new(Arc::clone(
        &first,
    ))));
    manager
        .start_all()
        .expect("first interaction source starts");
    manager
        .set_interaction_capture_active(true)
        .expect("first interaction source activates");

    manager.add_source(Box::new(CaptureTrackingInteractionSource::new(Arc::clone(
        &second,
    ))));
    manager
        .set_interaction_capture_active(true)
        .expect("invalidated cache activates the added source");

    assert_eq!(*first.lock().expect("first transitions lock"), vec![true]);
    assert_eq!(*second.lock().expect("second transitions lock"), vec![true]);
}

#[test]
fn manager_tracks_and_removes_host_capture_sources() {
    let transitions = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut mgr = InputManager::new();
    assert!(!mgr.has_host_capture_source());

    mgr.add_source(Box::new(CaptureTrackingInteractionSource::new(transitions)));
    mgr.add_source(Box::new(hypercolor_core::input::BrowserInputSource::new()));
    mgr.add_source(Box::new(MockAudioSource::new(0.5)));
    assert!(mgr.has_host_capture_source());
    assert!(mgr.has_interaction_source());
    assert_eq!(mgr.source_count(), 3);

    mgr.remove_host_capture_sources();
    assert!(!mgr.has_host_capture_source());
    assert!(
        mgr.has_interaction_source(),
        "browser injection survives host removal"
    );
    assert_eq!(mgr.source_count(), 2, "browser + audio survive");
}

#[test]
fn interaction_dirty_check_tracks_generation_and_batch() {
    use hypercolor_core::input::{InteractionBatch, InteractionData};
    use hypercolor_core::types::event::{InputButtonState, TimedInputEvent};

    let mut data = InteractionData::default();
    assert!(data.is_dirty_against(None), "first sight is always dirty");
    assert!(!data.is_dirty_against(Some(0)), "idle frames skip");

    data.generation = 1;
    assert!(data.is_dirty_against(Some(0)), "state change marks dirty");

    data.generation = 0;
    data.batch.events.push(TimedInputEvent {
        event: InputEvent::Key {
            source_id: "test".into(),
            key: "a".into(),
            state: InputButtonState::Pressed,
        },
        at_ms: 1,
        seq: 1,
        physical_code: None,
        repeat_count: 1,
    });
    assert!(data.is_dirty_against(Some(0)), "events mark dirty");

    let mut batch = InteractionBatch::default();
    assert!(batch.is_empty());
    batch.wheel_hi_res = 120;
    assert!(!batch.is_empty(), "wheel travel is renderer-visible");
}

#[test]
fn batch_absorb_prior_preserves_order_and_sums() {
    use hypercolor_core::input::InteractionBatch;
    use hypercolor_core::types::event::TimedInputEvent;

    fn timed(seq: u64) -> TimedInputEvent {
        TimedInputEvent {
            event: InputEvent::Key {
                source_id: "test".into(),
                key: "x".into(),
                state: InputButtonState::Pressed,
            },
            at_ms: seq,
            seq,
            physical_code: None,
            repeat_count: 1,
        }
    }

    let mut current = InteractionBatch {
        events: vec![timed(3), timed(4)],
        wheel_hi_res: 120,
        dropped_events: 1,
        ..Default::default()
    };
    let prior = InteractionBatch {
        events: vec![timed(1), timed(2)],
        wheel_hi_res: -360,
        dropped_events: 2,
        ..Default::default()
    };

    current.absorb_prior(prior);
    let seqs = current.events.iter().map(|e| e.seq).collect::<Vec<_>>();
    assert_eq!(seqs, [1, 2, 3, 4]);
    assert_eq!(current.wheel_hi_res, -240);
    assert_eq!(current.dropped_events, 3);

    let mut overflowing = InteractionBatch::default();
    let prior = InteractionBatch {
        events: (0..300).map(timed).collect(),
        ..Default::default()
    };
    overflowing.absorb_prior(prior);
    assert_eq!(overflowing.events.len(), InteractionBatch::MAX_EVENTS);
    assert_eq!(
        overflowing.dropped_events,
        300 - InteractionBatch::MAX_EVENTS as u32
    );
    assert_eq!(overflowing.events.first().map(|e| e.seq), Some(44));
}

// ── Pointer merge precedence ───────────────────────────────────────────────

/// Build a snapshot carrying only a pointer, at the given position.
fn pointer_snapshot(norm_x: f32, injected: bool) -> hypercolor_core::input::InteractionData {
    use hypercolor_core::input::{InteractionData, PointerMode};
    let mut data = InteractionData::default();
    data.mouse.mode = PointerMode::Absolute;
    data.mouse.norm_x = norm_x;
    data.mouse.injected = injected;
    data
}

#[test]
fn an_injected_pointer_wins_over_a_host_pointer_regardless_of_order() {
    // Host capture registers before the browser source, and first-non-None
    // alone would therefore let the desktop cursor shadow the preview canvas
    // — visibly wrong the moment the host pointer is a real desktop cursor
    // rather than a virtual one nobody cross-checks.
    let mut host_first = pointer_snapshot(0.1, false);
    host_first.merge_from(pointer_snapshot(0.9, true));
    assert!(
        (host_first.mouse.norm_x - 0.9).abs() < 1e-6,
        "the injected pointer must win when it merges second"
    );
    assert!(host_first.mouse.injected);

    let mut browser_first = pointer_snapshot(0.9, true);
    browser_first.merge_from(pointer_snapshot(0.1, false));
    assert!(
        (browser_first.mouse.norm_x - 0.9).abs() < 1e-6,
        "and when it merges first"
    );
}

#[test]
fn two_host_pointers_still_resolve_first_wins() {
    let mut first = pointer_snapshot(0.2, false);
    first.merge_from(pointer_snapshot(0.8, false));
    assert!(
        (first.mouse.norm_x - 0.2).abs() < 1e-6,
        "unchanged behaviour where no source is injected"
    );
}

#[test]
fn an_idle_source_never_blanks_an_active_pointer() {
    use hypercolor_core::input::{InteractionData, PointerMode};
    let mut active = pointer_snapshot(0.4, true);
    active.merge_from(InteractionData::default());
    assert_eq!(active.mouse.mode, PointerMode::Absolute);
    assert!((active.mouse.norm_x - 0.4).abs() < 1e-6);
}

#[test]
fn merging_preserves_recent_key_order_and_unions_held_state() {
    // Registration order is load-bearing for more than the pointer: recent
    // keys concatenate in source order, so fixing pointer precedence by
    // reordering sources would have traded a visible bug for a subtle one.
    use hypercolor_core::input::InteractionData;

    let mut first = InteractionData::default();
    first.keyboard.recent_keys = vec!["a".to_owned(), "b".to_owned()];
    first.keyboard.pressed_keys = vec!["a".to_owned()];
    first.mouse.buttons = vec!["left".to_owned()];

    let mut second = InteractionData::default();
    second.keyboard.recent_keys = vec!["c".to_owned()];
    second.keyboard.pressed_keys = vec!["z".to_owned()];
    second.mouse.buttons = vec!["right".to_owned()];

    first.merge_from(second);

    assert_eq!(
        first.keyboard.recent_keys,
        vec!["a".to_owned(), "b".to_owned(), "c".to_owned()]
    );
    assert_eq!(
        first.keyboard.pressed_keys,
        vec!["a".to_owned(), "z".to_owned()]
    );
    assert_eq!(
        first.mouse.buttons,
        vec!["left".to_owned(), "right".to_owned()]
    );
}

fn test_status_writer() -> (
    SourceStatusWriter,
    hypercolor_core::input::SourceStatusHandle,
) {
    SourceStatusWriter::new("test-screen", SourceKind::Screen, "test", true, true, true)
}

fn test_issue(code: &'static str) -> SourceIssue {
    SourceIssue::new(code, "test issue", true).with_remediation("retry the test source")
}

#[test]
fn source_session_slot_hands_a_long_lived_worker_the_successor_session() {
    let (writer, _handle) = test_status_writer();
    let first = writer
        .begin_session(1)
        .expect("first source session should start");
    let slot = SourceSessionSlot::new();
    slot.store(first.clone());
    assert_eq!(
        slot.load().map(|session| session.session_generation()),
        Some(first.session_generation())
    );

    writer.stop();
    slot.clear();
    assert!(slot.load().is_none());
    let successor = writer
        .begin_session(2)
        .expect("successor source session should start");
    slot.store(successor.clone());
    assert_eq!(
        slot.load().map(|session| session.session_generation()),
        Some(successor.session_generation())
    );
    assert!(successor.session_generation() > first.session_generation());
}

#[test]
fn source_resource_scan_health_maps_access_failure_and_recovery() {
    assert_eq!(
        classify_source_resource_scan(2, 0, 0),
        SourceResourceScanHealth::Live {
            resource_count: 2,
            denied_resource_count: 0,
        }
    );

    let SourceResourceScanHealth::Degraded {
        resource_count,
        denied_resource_count,
        issue: denied_partial,
    } = classify_source_resource_scan(2, 1, 0)
    else {
        panic!("opened plus denied resources should be degraded");
    };
    assert_eq!(resource_count, 2);
    assert_eq!(denied_resource_count, 1);
    assert_eq!(denied_partial.code.as_ref(), "access_denied");
    assert_eq!(
        denied_partial.remediation.as_deref(),
        Some("run `just udev-install` and replug or re-login")
    );

    let SourceResourceScanHealth::Degraded {
        resource_count,
        denied_resource_count,
        issue: failed_partial,
    } = classify_source_resource_scan(2, 0, 1)
    else {
        panic!("opened plus transient failures should be degraded");
    };
    assert_eq!(resource_count, 2);
    assert_eq!(denied_resource_count, 0);
    assert_eq!(
        failed_partial.code.as_ref(),
        "input_resource_scan_partial_failure"
    );

    let mixed = classify_source_resource_scan(2, 1, 1);
    let SourceResourceScanHealth::Degraded {
        resource_count,
        denied_resource_count,
        issue: mixed_partial,
    } = &mixed
    else {
        panic!("mixed usable and failed resources should be degraded");
    };
    assert_eq!(*resource_count, 2);
    assert_eq!(*denied_resource_count, 1);
    assert_eq!(mixed_partial.code.as_ref(), "access_denied");
    assert!(mixed_partial.message.contains("1 failed to open"));
    assert_eq!(
        mixed_partial.remediation.as_deref(),
        Some("run `just udev-install` and replug or re-login")
    );

    let denied_only = classify_source_resource_scan(0, 3, 0);
    let SourceResourceScanHealth::Unavailable {
        resource_count,
        denied_resource_count,
        issue: denied,
    } = &denied_only
    else {
        panic!("denied-only scan should be unavailable");
    };
    assert_eq!(*resource_count, 0);
    assert_eq!(*denied_resource_count, 3);
    assert_eq!(denied.code.as_ref(), "access_denied");
    assert_eq!(
        denied.remediation.as_deref(),
        Some("run `just udev-install` and replug or re-login")
    );

    let failed_only = classify_source_resource_scan(0, 0, 2);
    let SourceResourceScanHealth::Unavailable {
        resource_count,
        denied_resource_count,
        issue: failed,
    } = &failed_only
    else {
        panic!("transient scan failures should be structured");
    };
    assert_eq!(*resource_count, 0);
    assert_eq!(*denied_resource_count, 0);
    assert_eq!(failed.code.as_ref(), "input_resource_scan_failed");
    assert!(failed.retryable);

    let (writer, handle) = test_status_writer();
    let session = writer
        .begin_session(1)
        .expect("resource scan status session should start");
    assert!(session.publish_resource_scan_health(mixed));
    let degraded = handle.snapshot();
    assert_eq!(degraded.state, SourceState::Degraded);
    assert_eq!(degraded.resource_count, 2);
    assert_eq!(degraded.denied_resource_count, 1);
    assert_eq!(
        degraded.issue.as_ref().map(|issue| issue.code.as_ref()),
        Some("access_denied")
    );

    assert!(session.publish_resource_scan_health(denied_only));
    let unavailable = handle.snapshot();
    assert_eq!(unavailable.state, SourceState::Unavailable);
    assert_eq!(unavailable.resource_count, 0);
    assert_eq!(unavailable.denied_resource_count, 3);

    assert!(session.publish_resource_scan_health(failed_only));
    let failed = handle.snapshot();
    assert_eq!(failed.state, SourceState::Unavailable);
    assert_eq!(failed.resource_count, 0);
    assert_eq!(failed.denied_resource_count, 0);

    let healthy = classify_source_resource_scan(3, 0, 0);
    assert!(session.publish_resource_scan_health(healthy.clone()));
    let recovered = handle.snapshot();
    assert_eq!(recovered.state, SourceState::Live);
    assert_eq!(recovered.resource_count, 3);
    assert_eq!(recovered.denied_resource_count, 0);
    assert!(recovered.issue.is_none());

    assert!(session.publish_resource_scan_health(healthy));
    assert!(Arc::ptr_eq(&recovered, &handle.snapshot()));

    writer.stop();
    let stopped = handle.snapshot();
    assert_eq!(stopped.resource_count, 0);
    assert_eq!(stopped.denied_resource_count, 0);
    let successor = writer
        .begin_session(2)
        .expect("successor resource scan session should start");
    let starting = handle.snapshot();
    assert_eq!(starting.resource_count, 0);
    assert_eq!(starting.denied_resource_count, 0);
    assert!(successor.publish_resource_scan_health(classify_source_resource_scan(0, 2, 0)));
    assert_eq!(handle.snapshot().denied_resource_count, 2);
    assert!(successor.failed(test_issue("scan_worker_failed")));
    let failed = handle.snapshot();
    assert_eq!(failed.state, SourceState::Failed);
    assert_eq!(failed.resource_count, 0);
    assert_eq!(failed.denied_resource_count, 0);
}

#[test]
fn source_backend_updates_preserve_lifecycle_and_deduplicate() {
    let (writer, handle) = test_status_writer();
    let before = handle.snapshot();

    writer
        .set_backend("pipewire")
        .expect("backend update should succeed");
    let updated = handle.snapshot();
    assert_eq!(updated.backend.as_ref(), "pipewire");
    assert_eq!(updated.state, before.state);
    assert_eq!(
        updated.source_graph_generation,
        before.source_graph_generation
    );
    assert_eq!(updated.session_generation, before.session_generation);

    writer
        .set_backend("pipewire")
        .expect("same backend should be a no-op");
    assert!(Arc::ptr_eq(&updated, &handle.snapshot()));
}

#[test]
fn terminal_failure_latch_probes_once_per_worker_session() {
    let mut latch = TerminalFailureLatch::default();
    let probes = std::cell::Cell::new(0);

    let first = latch.take(|| {
        probes.set(probes.get() + 1);
        Some(String::from("worker failed"))
    });
    let second = latch.take(|| {
        probes.set(probes.get() + 1);
        Some(String::from("must not allocate again"))
    });

    assert_eq!(first.as_deref(), Some("worker failed"));
    assert!(second.is_none());
    assert_eq!(probes.get(), 1);
    assert!(latch.is_latched());

    latch.reset();
    assert_eq!(
        latch.take(|| Some("successor session")),
        Some("successor session")
    );
}

async fn align_tokio_to_source_clock(
    session: &hypercolor_core::input::SourceSessionWriter,
) -> Instant {
    let runtime_now = tokio::time::Instant::now().into_std();
    if let Some(delta) = session
        .earliest_encodable_timestamp()
        .checked_duration_since(runtime_now)
    {
        tokio::time::advance(delta).await;
    }
    tokio::time::Instant::now().into_std()
}

#[test]
fn source_status_covers_stop_start_failure_and_restart_lifecycle() {
    let (writer, handle) = test_status_writer();
    let initial = handle.snapshot();
    assert_eq!(initial.state, SourceState::Stopped);
    assert_eq!(initial.source_graph_generation, 0);
    assert_eq!(initial.session_generation, 0);

    let session = writer
        .begin_session(7)
        .expect("eligible source session should start");
    assert_eq!(session.session_generation(), 1);
    assert_eq!(handle.snapshot().state, SourceState::Starting);

    let sampled_at = Instant::now();
    let deadline = sampled_at + Duration::from_secs(1);
    assert_eq!(session.record_sample(sampled_at, deadline, 2), Ok(true));
    let live = handle.snapshot_at(sampled_at);
    assert_eq!(live.state, SourceState::Live);
    assert_eq!(live.freshness, SourceFreshness::Fresh);

    assert!(session.degraded(test_issue("degraded")));
    let stale_degraded = handle.snapshot_at(deadline);
    assert_eq!(stale_degraded.state, SourceState::Degraded);
    assert_eq!(stale_degraded.freshness, SourceFreshness::Stale);
    assert_eq!(
        stale_degraded
            .issue
            .as_ref()
            .map(|issue| issue.code.as_ref()),
        Some("degraded")
    );
    assert_eq!(
        stale_degraded
            .freshness_issue
            .as_ref()
            .map(|issue| issue.code.as_ref()),
        Some("stale_data")
    );
    assert!(session.unavailable(test_issue("unavailable")));
    assert_eq!(handle.snapshot().state, SourceState::Unavailable);
    assert!(session.failed(test_issue("failed")));
    let failed = handle.snapshot_at(sampled_at);
    assert_eq!(failed.state, SourceState::Failed);
    assert_eq!(failed.freshness, SourceFreshness::Fresh);
    assert_eq!(failed.resource_count, 0);
    assert_eq!(failed.last_sample_at, Some(sampled_at));
    let expired_failed = handle.snapshot_at(deadline);
    assert_eq!(expired_failed.state, SourceState::Failed);
    assert_eq!(expired_failed.freshness, SourceFreshness::Stale);
    assert_eq!(
        session.record_sample(Instant::now(), deadline, 1),
        Ok(false)
    );
    assert!(!session.degraded(test_issue("late_degraded")));

    writer.stop();
    let stopped = handle.snapshot();
    assert_eq!(stopped.state, SourceState::Stopped);
    assert_eq!(stopped.resource_count, 0);
    assert!(stopped.issue.is_none());

    let restarted = writer
        .begin_session(8)
        .expect("stopped eligible source should restart");
    assert_eq!(restarted.session_generation(), 2);
    let status = handle.snapshot();
    assert_eq!(status.state, SourceState::Starting);
    assert_eq!(status.session_generation, 2);
    assert_eq!(status.source_graph_generation, 8);
}

#[test]
fn source_status_expires_live_data_exactly_at_its_deadline() {
    let (writer, handle) = test_status_writer();
    let session = writer
        .begin_session(1)
        .expect("eligible source session should start");
    let sampled_at = Instant::now();
    let deadline = sampled_at + Duration::from_secs(1);
    assert_eq!(session.record_sample(sampled_at, deadline, 1), Ok(true));

    let just_before_deadline = deadline
        .checked_sub(Duration::from_nanos(1))
        .expect("freshness deadline follows the sample time");
    assert_eq!(
        handle.snapshot_at(just_before_deadline).freshness,
        SourceFreshness::Fresh
    );
    let expired = handle.snapshot_at(deadline);
    assert_eq!(expired.state, SourceState::Live);
    assert_eq!(expired.freshness, SourceFreshness::Stale);
    assert_eq!(
        expired
            .freshness_issue
            .as_ref()
            .map(|issue| issue.code.as_ref()),
        Some("stale_data")
    );
    assert!(expired.issue.is_none());
}

#[test]
fn nonproducing_health_never_invents_sample_freshness_or_resources() {
    let (writer, handle) = test_status_writer();
    let unavailable_session = writer
        .begin_session(1)
        .expect("source session should start");
    assert!(unavailable_session.unavailable(test_issue("not_present")));
    let unavailable = handle.snapshot();
    assert_eq!(unavailable.state, SourceState::Unavailable);
    assert_eq!(unavailable.freshness, SourceFreshness::NotApplicable);
    assert!(unavailable.last_sample_at.is_none());
    assert!(unavailable.freshness_deadline.is_none());
    assert_eq!(unavailable.resource_count, 0);

    writer.stop();
    let failed_session = writer
        .begin_session(2)
        .expect("newer source session should start");
    assert!(failed_session.failed(test_issue("startup_failed")));
    let failed = handle.snapshot();
    assert_eq!(failed.state, SourceState::Failed);
    assert_eq!(failed.freshness, SourceFreshness::NotApplicable);
    assert!(failed.last_sample_at.is_none());
    assert!(failed.freshness_deadline.is_none());
    assert_eq!(failed.resource_count, 0);
}

#[test]
fn stale_source_sessions_cannot_overwrite_successors_or_stops_under_race() {
    let (writer, handle) = test_status_writer();
    let stale = writer
        .begin_session(1)
        .expect("eligible source session should start");
    let stale_worker_writer = stale.clone();
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let stale_worker = std::thread::spawn(move || {
        ready_tx.send(()).expect("root still waits for readiness");
        release_rx.recv().expect("root releases stale publisher");
        let sampled_at = Instant::now();
        stale_worker_writer.record_sample(sampled_at, sampled_at + Duration::from_secs(1), 1)
    });

    ready_rx
        .recv()
        .expect("stale publisher announces readiness");
    let successor = writer
        .begin_session(2)
        .expect("successor session should start");
    release_tx
        .send(())
        .expect("stale publisher still waits for release");
    assert_eq!(
        stale_worker
            .join()
            .expect("stale publisher thread panicked"),
        Ok(false)
    );
    let status = handle.snapshot();
    assert_eq!(status.state, SourceState::Starting);
    assert_eq!(status.session_generation, successor.session_generation());
    let sampled_at = Instant::now();
    assert_eq!(
        stale.record_sample(sampled_at, sampled_at + Duration::from_secs(1), 1),
        Ok(false)
    );
    assert_eq!(stale.record_sample(sampled_at, sampled_at, 1), Ok(false));

    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let successor_worker = std::thread::spawn(move || {
        ready_tx.send(()).expect("root still waits for readiness");
        release_rx
            .recv()
            .expect("root releases successor publisher");
        let sampled_at = Instant::now();
        successor.record_sample(sampled_at, sampled_at + Duration::from_secs(1), 1)
    });
    ready_rx
        .recv()
        .expect("successor publisher announces readiness");
    writer.stop();
    release_tx
        .send(())
        .expect("successor publisher still waits for release");
    assert_eq!(
        successor_worker
            .join()
            .expect("successor publisher thread panicked"),
        Ok(false)
    );
    assert_eq!(handle.snapshot().state, SourceState::Stopped);
}

#[test]
fn removing_source_demand_clears_runtime_state_and_fences_the_session() {
    let (writer, handle) = test_status_writer();
    let session = writer
        .begin_session(1)
        .expect("eligible source session should start");
    let sampled_at = Instant::now();
    let deadline = sampled_at + Duration::from_secs(1);
    assert_eq!(session.record_sample(sampled_at, deadline, 3), Ok(true));
    assert!(session.degraded(test_issue("backend_slow")));

    writer
        .set_policy(true, true, false)
        .expect("active source policy should update");
    let stopped = handle.snapshot();
    assert!(!stopped.demanded);
    assert_eq!(stopped.state, SourceState::Stopped);
    assert!(stopped.last_sample_at.is_none());
    assert!(stopped.freshness_deadline.is_none());
    assert_eq!(stopped.resource_count, 0);
    assert!(stopped.issue.is_none());
    assert!(stopped.freshness_issue.is_none());
    let sampled_at = Instant::now();
    assert_eq!(
        session.record_sample(sampled_at, sampled_at + Duration::from_secs(1), 1),
        Ok(false)
    );
}

#[test]
fn configuration_and_consent_revocation_stop_and_fence_sessions() {
    let (writer, handle) = SourceStatusWriter::new(
        "policy-source",
        SourceKind::Interaction,
        "test",
        false,
        true,
        true,
    );
    assert!(matches!(
        writer.begin_session(1),
        Err(SourceStatusError::Ineligible {
            configured: false,
            consented: true,
            demanded: true,
        })
    ));

    writer
        .set_policy(true, true, true)
        .expect("non-retired source policy should update");
    let configured_session = writer
        .begin_session(1)
        .expect("eligible source session should start");
    let sampled_at = Instant::now();
    assert_eq!(
        configured_session.record_sample(sampled_at, sampled_at + Duration::from_secs(1), 1),
        Ok(true)
    );
    writer
        .set_policy(false, true, true)
        .expect("configuration revocation should publish");
    let stopped = handle.snapshot();
    assert_eq!(stopped.state, SourceState::Stopped);
    assert_eq!(stopped.freshness, SourceFreshness::NotApplicable);
    assert_eq!(stopped.resource_count, 0);
    assert!(stopped.last_sample_at.is_none());
    assert!(stopped.issue.is_none());
    assert!(!configured_session.degraded(test_issue("late")));

    writer
        .set_policy(true, true, true)
        .expect("configuration restore should publish");
    let consented_session = writer
        .begin_session(2)
        .expect("restored source session should start");
    writer
        .set_policy(true, false, true)
        .expect("consent revocation should publish");
    assert_eq!(handle.snapshot().state, SourceState::Stopped);
    assert!(!consented_session.unavailable(test_issue("late")));
    assert!(matches!(
        writer.begin_session(3),
        Err(SourceStatusError::Ineligible {
            configured: true,
            consented: false,
            demanded: true,
        })
    ));
}

#[test]
fn source_graph_generation_never_regresses_across_restarts() {
    let (writer, handle) = test_status_writer();
    let first = writer
        .begin_session(5)
        .expect("initial graph generation should stamp");
    assert_eq!(handle.snapshot().source_graph_generation, 5);
    writer.stop();
    assert!(matches!(
        writer.begin_session(4),
        Err(SourceStatusError::GraphGenerationNotAdvanced {
            current: 5,
            requested: 4,
        })
    ));
    assert!(matches!(
        writer.begin_session(5),
        Err(SourceStatusError::GraphGenerationNotAdvanced {
            current: 5,
            requested: 5,
        })
    ));
    let newer = writer
        .begin_session(6)
        .expect("newer graph generation should stamp");
    assert!(newer.session_generation() > first.session_generation());
    assert_eq!(handle.snapshot().source_graph_generation, 6);
}

#[test]
fn event_driven_live_state_has_no_sample_freshness_contract() {
    let (writer, handle) = test_status_writer();
    let session = writer
        .begin_session(1)
        .expect("eligible event source should start");
    assert!(session.mark_event_driven_live_without_deadline(1));
    let status = handle.snapshot();
    assert_eq!(status.state, SourceState::Live);
    assert_eq!(status.freshness, SourceFreshness::NotApplicable);
    assert!(status.last_sample_at.is_none());
    assert!(status.freshness_deadline.is_none());
}

#[tokio::test(start_paused = true)]
async fn source_status_subscription_wakes_on_publish_and_freshness_expiry() {
    let (writer, handle) = test_status_writer();
    let session = writer
        .begin_session(1)
        .expect("eligible source session should start");
    let mut subscription = handle.subscribe();
    let sampled_at = align_tokio_to_source_clock(&session).await;
    let deadline = sampled_at + Duration::from_secs(5);

    assert_eq!(session.record_sample(sampled_at, deadline, 1), Ok(true));
    let published = subscription
        .changed()
        .await
        .expect("live publication should arrive");
    assert_eq!(published.state, SourceState::Live);

    let expiry = tokio::spawn(async move {
        let status = subscription.changed().await;
        (subscription, status)
    });
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_millis(4_999)).await;
    tokio::task::yield_now().await;
    assert!(!expiry.is_finished());
    tokio::time::advance(Duration::from_millis(1)).await;
    let (mut subscription, expired) = expiry.await.expect("freshness subscriber task panicked");
    let expired = expired.expect("freshness expiry should arrive");
    assert_eq!(expired.state, SourceState::Live);
    assert_eq!(expired.freshness, SourceFreshness::Stale);
    assert_eq!(
        expired
            .freshness_issue
            .as_ref()
            .map(|issue| issue.code.as_ref()),
        Some("stale_data")
    );

    let mut duplicate = Box::pin(subscription.changed());
    tokio::select! {
        biased;
        result = &mut duplicate => panic!("duplicate expiry transition: {result:?}"),
        () = tokio::task::yield_now() => {}
    }
}

#[tokio::test(start_paused = true)]
async fn sample_refresh_reschedules_expiry_without_a_structural_wake() {
    let (writer, handle) = test_status_writer();
    let session = writer
        .begin_session(1)
        .expect("eligible source session should start");
    let initial_sample = align_tokio_to_source_clock(&session).await;
    assert_eq!(
        session.record_sample(initial_sample, initial_sample + Duration::from_secs(5), 1),
        Ok(true)
    );
    let mut subscription = handle.subscribe();
    let expiry = tokio::spawn(async move {
        let status = subscription.changed().await;
        (subscription, status)
    });
    tokio::task::yield_now().await;

    tokio::time::advance(Duration::from_secs(4)).await;
    let refreshed_at = tokio::time::Instant::now().into_std();
    assert_eq!(
        session.record_sample(refreshed_at, refreshed_at + Duration::from_secs(6), 1),
        Ok(true)
    );
    tokio::task::yield_now().await;
    assert!(!expiry.is_finished());

    tokio::time::advance(Duration::from_secs(1)).await;
    tokio::task::yield_now().await;
    assert!(!expiry.is_finished());
    tokio::time::advance(Duration::from_secs(5)).await;
    let (mut subscription, expired) = expiry.await.expect("freshness subscriber task panicked");
    let expired = expired.expect("rescheduled freshness expiry should arrive");
    assert_eq!(expired.freshness, SourceFreshness::Stale);
    assert_eq!(
        expired.freshness_deadline,
        Some(refreshed_at + Duration::from_secs(6))
    );

    let recovered_at = tokio::time::Instant::now().into_std();
    assert_eq!(
        session.record_sample(recovered_at, recovered_at + Duration::from_secs(5), 1),
        Ok(true)
    );
    let recovered = subscription
        .changed()
        .await
        .expect("stale-to-fresh recovery should publish");
    assert_eq!(recovered.freshness, SourceFreshness::Fresh);
}

#[tokio::test]
async fn cancelled_status_wait_can_reenter_without_losing_a_publication() {
    let (writer, handle) = test_status_writer();
    let session = writer
        .begin_session(1)
        .expect("eligible source session should start");
    let mut subscription = handle.subscribe();
    let mut cancelled_wait = Box::pin(subscription.changed());
    tokio::select! {
        biased;
        result = &mut cancelled_wait => panic!("status wait completed unexpectedly: {result:?}"),
        () = tokio::task::yield_now() => {}
    }
    drop(cancelled_wait);

    assert!(session.degraded(test_issue("after_cancel")));
    let status = subscription
        .changed()
        .await
        .expect("publication after cancellation should arrive");
    assert_eq!(status.state, SourceState::Degraded);
}

#[tokio::test]
async fn stopping_an_already_stopped_source_publishes_nothing() {
    let (writer, handle) = test_status_writer();
    let mut subscription = handle.subscribe();
    writer.stop();

    let mut duplicate = Box::pin(subscription.changed());
    tokio::select! {
        biased;
        result = &mut duplicate => panic!("idempotent stop published: {result:?}"),
        () = tokio::task::yield_now() => {}
    }
    drop(duplicate);

    writer
        .begin_session(1)
        .expect("source remains startable after idempotent stop");
    let starting = subscription
        .changed()
        .await
        .expect("real start transition should publish");
    assert_eq!(starting.state, SourceState::Starting);
}

#[tokio::test]
async fn identical_structural_health_does_not_publish_or_wake_twice() {
    let (writer, handle) = test_status_writer();
    let session = writer
        .begin_session(1)
        .expect("source session should start");
    let issue = test_issue("backend_unavailable");
    assert!(session.unavailable(issue.clone()));
    let mut subscription = handle.subscribe();

    assert!(session.unavailable(issue));
    let mut duplicate = Box::pin(subscription.changed());
    tokio::select! {
        biased;
        result = &mut duplicate => panic!("identical health republished: {result:?}"),
        () = tokio::task::yield_now() => {}
    }
}

#[tokio::test]
async fn retiring_source_wakes_and_terminates_subscribers() {
    let (writer, handle) = test_status_writer();
    let _session = writer
        .begin_session(1)
        .expect("eligible source session should start");
    let mut subscription = handle.subscribe();
    assert!(matches!(
        writer.retire(1),
        Err(SourceStatusError::GraphGenerationNotAdvanced {
            current: 1,
            requested: 1,
        })
    ));
    writer
        .retire(2)
        .expect("newer removal generation should retire source");

    let retired = subscription
        .changed()
        .await
        .expect("retirement publication should arrive");
    assert!(retired.retired);
    assert_eq!(retired.state, SourceState::Stopped);
    assert_eq!(retired.source_graph_generation, 2);
    assert!(subscription.changed().await.is_none());
    let mut after_retirement = handle.subscribe();
    assert!(after_retirement.snapshot().retired);
    assert!(after_retirement.changed().await.is_none());
    assert!(matches!(
        writer.begin_session(2),
        Err(SourceStatusError::Retired)
    ));
    assert_eq!(
        writer.set_policy(true, true, true),
        Err(SourceStatusError::Retired)
    );
}

#[test]
fn source_status_rejects_non_advancing_graph_zero_and_invalid_deadlines() {
    let (writer, handle) = test_status_writer();
    assert!(matches!(
        writer.begin_session(0),
        Err(SourceStatusError::GraphGenerationNotAdvanced {
            current: 0,
            requested: 0,
        })
    ));
    let session = writer
        .begin_session(1)
        .expect("positive initial graph generation should start");
    let sampled_at = Instant::now();
    assert_eq!(
        session.record_sample(sampled_at, sampled_at, 1),
        Err(SourceStatusError::InvalidFreshnessDeadline {
            sampled_at,
            freshness_deadline: sampled_at,
        })
    );
    let before_epoch = session
        .earliest_encodable_timestamp()
        .checked_sub(Duration::from_millis(1))
        .expect("source timestamp epoch follows the monotonic clock origin");
    let valid_deadline = session.earliest_encodable_timestamp() + Duration::from_secs(1);
    assert_eq!(
        session.record_sample(before_epoch, valid_deadline, 1),
        Err(SourceStatusError::TimestampOutOfRange {
            field: SourceTimestampField::SampledAt,
            timestamp: before_epoch,
        })
    );
    assert_eq!(handle.snapshot().state, SourceState::Starting);
}

#[test]
fn sample_clock_never_splices_data_across_source_sessions() {
    let (writer, handle) = test_status_writer();
    let first = writer
        .begin_session(1)
        .expect("first source session should start");
    let first_sample = Instant::now();
    let first_deadline = first_sample + Duration::from_secs(10);
    assert_eq!(
        first.record_sample(first_sample, first_deadline, 11),
        Ok(true)
    );
    let first_status = handle.snapshot_at(first_sample);
    assert_eq!(first_status.last_sample_at, Some(first_sample));
    assert_eq!(first_status.resource_count, 11);

    let successor = writer
        .begin_session(2)
        .expect("successor source session should start");
    let starting = handle.snapshot();
    assert_eq!(starting.session_generation, successor.session_generation());
    assert_eq!(starting.freshness, SourceFreshness::AwaitingSample);
    assert!(starting.last_sample_at.is_none());
    assert!(starting.freshness_deadline.is_none());
    assert_eq!(starting.resource_count, 0);

    let second_sample = first_sample + Duration::from_secs(1);
    let second_deadline = second_sample + Duration::from_secs(20);
    assert_eq!(
        successor.record_sample(second_sample, second_deadline, 22),
        Ok(true)
    );
    let second_status = handle.snapshot_at(second_sample);
    assert_eq!(
        second_status.session_generation,
        successor.session_generation()
    );
    assert_eq!(second_status.last_sample_at, Some(second_sample));
    assert_eq!(second_status.freshness_deadline, Some(second_deadline));
    assert_eq!(second_status.resource_count, 22);
}

fn assert_status_sample_coherence(status: &hypercolor_core::input::SourceStatus) {
    if matches!(
        status.freshness,
        SourceFreshness::Fresh | SourceFreshness::Stale
    ) {
        let sampled_at = status
            .last_sample_at
            .expect("fresh status carries its sample timestamp");
        let deadline = status
            .freshness_deadline
            .expect("fresh status carries its deadline");
        assert!(deadline > sampled_at);
        assert_ne!(status.session_generation, 0);
    }
    if status.freshness == SourceFreshness::NotApplicable {
        assert!(status.last_sample_at.is_none());
        assert!(status.freshness_deadline.is_none());
    }
}

fn assert_transition_never_splices_sample_plane(
    transition: impl FnOnce(&SourceStatusWriter, &hypercolor_core::input::SourceSessionWriter),
) {
    let (writer, handle) = test_status_writer();
    let session = writer
        .begin_session(1)
        .expect("source session should start");
    let sampled_at = Instant::now();
    assert_eq!(
        session.record_sample(sampled_at, sampled_at + Duration::from_mins(1), 1),
        Ok(true)
    );
    let done = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let reader_done = Arc::clone(&done);
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let reader = std::thread::spawn(move || {
        ready_tx.send(()).expect("root waits for reader readiness");
        release_rx.recv().expect("root releases coherence reader");
        assert_status_sample_coherence(&handle.snapshot());
        started_tx
            .send(())
            .expect("root waits for active coherence reader");
        let mut tail = 0;
        loop {
            assert_status_sample_coherence(&handle.snapshot());
            if reader_done.load(std::sync::atomic::Ordering::Acquire) {
                tail += 1;
                if tail == 1_024 {
                    break;
                }
            }
        }
    });

    ready_rx
        .recv()
        .expect("coherence reader announces readiness");
    release_tx
        .send(())
        .expect("coherence reader waits for release");
    started_rx.recv().expect("coherence reader is active");
    transition(&writer, &session);
    done.store(true, std::sync::atomic::Ordering::Release);
    reader.join().expect("coherence reader thread panicked");
}

#[test]
fn stop_revoke_retire_and_event_transitions_never_splice_sample_state() {
    assert_transition_never_splices_sample_plane(|writer, _| writer.stop());
    assert_transition_never_splices_sample_plane(|writer, _| {
        writer
            .set_policy(true, false, true)
            .expect("consent revocation should publish");
    });
    assert_transition_never_splices_sample_plane(|writer, _| {
        writer
            .retire(2)
            .expect("newer removal generation should retire");
    });
    assert_transition_never_splices_sample_plane(|_, session| {
        assert!(session.mark_event_driven_live_without_deadline(1));
    });
}

#[tokio::test(start_paused = true)]
async fn shortened_sample_deadline_reschedules_without_false_transition() {
    let (writer, handle) = test_status_writer();
    let session = writer
        .begin_session(1)
        .expect("source session should start");
    let initial = align_tokio_to_source_clock(&session).await;
    assert_eq!(
        session.record_sample(initial, initial + Duration::from_secs(10), 1),
        Ok(true)
    );
    let mut subscription = handle.subscribe();
    let expiry = tokio::spawn(async move { subscription.changed().await });
    tokio::task::yield_now().await;

    tokio::time::advance(Duration::from_secs(1)).await;
    let shortened_sample = tokio::time::Instant::now().into_std();
    let shortened_deadline = shortened_sample + Duration::from_secs(2);
    assert_eq!(
        session.record_sample(shortened_sample, shortened_deadline, 1),
        Ok(true)
    );
    tokio::task::yield_now().await;
    assert!(!expiry.is_finished());
    tokio::time::advance(Duration::from_secs(1)).await;
    tokio::task::yield_now().await;
    assert!(!expiry.is_finished());
    tokio::time::advance(Duration::from_secs(1)).await;
    let expired = expiry
        .await
        .expect("freshness subscriber task panicked")
        .expect("shortened deadline should expire");
    assert_eq!(expired.freshness, SourceFreshness::Stale);
    assert_eq!(expired.freshness_deadline, Some(shortened_deadline));
}

#[tokio::test(start_paused = true)]
async fn observed_stale_data_recovers_even_with_a_delayed_sample_timestamp() {
    let (writer, handle) = test_status_writer();
    let session = writer
        .begin_session(1)
        .expect("source session should start");
    let initial = align_tokio_to_source_clock(&session).await;
    let initial_deadline = initial + Duration::from_secs(4);
    assert_eq!(
        session.record_sample(initial, initial_deadline, 1),
        Ok(true)
    );
    let mut subscription = handle.subscribe();
    tokio::time::advance(Duration::from_secs(4)).await;
    let stale = subscription
        .changed()
        .await
        .expect("freshness expiry should be observed");
    assert_eq!(stale.freshness, SourceFreshness::Stale);

    let mut readers = Vec::new();
    let mut releases = Vec::new();
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    for _ in 0..8 {
        let reader_handle = handle.clone();
        let ready_tx = ready_tx.clone();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        releases.push(release_tx);
        readers.push(std::thread::spawn(move || {
            ready_tx.send(()).expect("root waits for stale reader");
            release_rx.recv().expect("root releases stale reader");
            assert_eq!(
                reader_handle.snapshot_at(initial_deadline).freshness,
                SourceFreshness::Stale
            );
        }));
    }
    for _ in 0..readers.len() {
        ready_rx.recv().expect("stale reader announces readiness");
    }
    for release in releases {
        release.send(()).expect("stale reader waits for release");
    }
    for reader in readers {
        reader.join().expect("stale reader thread panicked");
    }

    let delayed_sample = initial + Duration::from_secs(3);
    let recovered_deadline = initial + Duration::from_secs(8);
    assert_eq!(
        session.record_sample(delayed_sample, recovered_deadline, 1),
        Ok(true)
    );
    let recovered = subscription
        .changed()
        .await
        .expect("observed stale data should signal recovery");
    assert_eq!(recovered.freshness, SourceFreshness::Fresh);
    assert_eq!(recovered.last_sample_at, Some(delayed_sample));
}

#[tokio::test(start_paused = true)]
async fn stale_projection_races_never_park_recovery_subscribers() {
    let (writer, handle) = test_status_writer();
    let session = writer
        .begin_session(1)
        .expect("source session should start");
    let initial = align_tokio_to_source_clock(&session).await;
    assert_eq!(
        session.record_sample(initial, initial + Duration::from_secs(10), 1),
        Ok(true)
    );
    let mut subscription = handle.subscribe();
    let mut resource_count = 1;
    let mut stale_first_races = 0;

    for iteration in 0..32 {
        let now = tokio::time::Instant::now().into_std();
        let expiring_at = now + Duration::from_secs(1);
        assert_eq!(
            session.record_sample(now, expiring_at, resource_count),
            Ok(true)
        );

        let mut wait = tokio::spawn(async move {
            let status = subscription.changed().await;
            (subscription, status)
        });
        tokio::task::yield_now().await;

        let racing_writer = session.clone();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let writer_thread = std::thread::spawn(move || {
            ready_tx.send(()).expect("root waits for racing writer");
            release_rx.recv().expect("root releases racing writer");
            racing_writer.record_sample(
                expiring_at,
                expiring_at + Duration::from_secs(10),
                resource_count,
            )
        });
        ready_rx.recv().expect("racing writer announces readiness");
        if iteration == 0 {
            tokio::time::advance(Duration::from_secs(1)).await;
            let (returned, status) = wait.await.expect("status subscriber task panicked");
            subscription = returned;
            let status = status.expect("active status subscription should remain open");
            assert_eq!(status.freshness, SourceFreshness::Stale);
            stale_first_races += 1;

            release_tx
                .send(())
                .expect("racing writer waits for release");
            assert_eq!(
                writer_thread.join().expect("racing writer panicked"),
                Ok(true)
            );
            let recovered = tokio::time::timeout(Duration::from_millis(1), subscription.changed())
                .await
                .expect("stale recovery signal was lost")
                .expect("active status subscription should remain open");
            assert_eq!(recovered.freshness, SourceFreshness::Fresh);
            continue;
        }

        release_tx
            .send(())
            .expect("racing writer waits for release");
        tokio::time::advance(Duration::from_secs(1)).await;
        assert_eq!(
            writer_thread.join().expect("racing writer panicked"),
            Ok(true)
        );

        if let Ok(joined) = tokio::time::timeout(Duration::from_millis(1), &mut wait).await {
            let (returned, status) = joined.expect("status subscriber task panicked");
            subscription = returned;
            let status = status.expect("active status subscription should remain open");
            if status.freshness == SourceFreshness::Stale {
                stale_first_races += 1;
                let recovered =
                    tokio::time::timeout(Duration::from_millis(1), subscription.changed())
                        .await
                        .expect("stale recovery signal was lost")
                        .expect("active status subscription should remain open");
                assert_eq!(recovered.freshness, SourceFreshness::Fresh);
            } else {
                assert_eq!(status.freshness, SourceFreshness::Fresh);
            }
        } else {
            resource_count = if resource_count == 1 { 2 } else { 1 };
            let refreshed_at = tokio::time::Instant::now().into_std();
            assert_eq!(
                session.record_sample(
                    refreshed_at,
                    refreshed_at + Duration::from_secs(10),
                    resource_count,
                ),
                Ok(true)
            );
            let (returned, status) = wait.await.expect("status subscriber task panicked");
            subscription = returned;
            let status = status.expect("active status subscription should remain open");
            assert_eq!(status.freshness, SourceFreshness::Fresh);
            assert_eq!(status.resource_count, resource_count);
        }
    }
    assert!(
        stale_first_races > 0,
        "bounded race must exercise at least one observed stale recovery"
    );
}

#[tokio::test]
async fn structural_subscription_preserves_live_sample_fields_once() {
    let (writer, handle) = test_status_writer();
    let session = writer
        .begin_session(1)
        .expect("source session should start");
    let sampled_at = Instant::now();
    let deadline = sampled_at + Duration::from_mins(1);
    assert_eq!(session.record_sample(sampled_at, deadline, 3), Ok(true));
    let mut subscription = handle.subscribe();

    assert!(session.degraded(test_issue("degraded_once")));
    let degraded = subscription
        .changed()
        .await
        .expect("degraded publication should arrive");
    assert_eq!(degraded.state, SourceState::Degraded);
    assert_eq!(degraded.freshness, SourceFreshness::Fresh);
    assert_eq!(degraded.last_sample_at, Some(sampled_at));
    assert_eq!(degraded.freshness_deadline, Some(deadline));
    assert_eq!(degraded.resource_count, 3);
    assert_eq!(
        degraded.issue.as_ref().map(|issue| issue.code.as_ref()),
        Some("degraded_once")
    );
    let mut duplicate = Box::pin(subscription.changed());
    tokio::select! {
        biased;
        result = &mut duplicate => panic!("duplicate structural publication: {result:?}"),
        () = tokio::task::yield_now() => {}
    }
    drop(duplicate);

    assert!(session.unavailable(test_issue("unavailable_once")));
    let unavailable = subscription
        .changed()
        .await
        .expect("unavailable publication should arrive");
    assert_eq!(unavailable.state, SourceState::Unavailable);
    assert_eq!(unavailable.freshness, SourceFreshness::Fresh);
    assert_eq!(unavailable.resource_count, 0);
    assert_eq!(unavailable.last_sample_at, Some(sampled_at));
}

#[test]
fn source_status_handles_and_publishers_are_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}

    assert_send_sync::<hypercolor_core::input::SourceStatusHandle>();
    assert_send_sync::<hypercolor_core::input::SourceStatusWriter>();
    assert_send_sync::<hypercolor_core::input::SourceSessionWriter>();
    assert_send_sync::<hypercolor_core::input::SourceStatusSubscription>();
}
