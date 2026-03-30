//! Tests for the input source abstraction layer.

#[cfg(target_os = "linux")]
use hypercolor_core::input::screen::{CaptureConfig, WaylandScreenCaptureInput};
use hypercolor_core::input::{InputData, InputManager, InputSource, ScreenData};
use hypercolor_core::types::audio::{AudioData, AudioPipelineConfig, AudioSourceType};
use hypercolor_core::types::event::{InputButtonState, InputEvent, ZoneColors};

// ── Mock Sources ───────────────────────────────────────────────────────────

/// A mock audio input source that produces a known `AudioData` snapshot.
struct MockAudioSource {
    running: bool,
    rms_level: f32,
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
    events: Vec<InputEvent>,
}

impl EventfulSource {
    fn new(events: Vec<InputEvent>) -> Self {
        Self {
            running: false,
            events,
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

    fn drain_events(&mut self) -> Vec<InputEvent> {
        std::mem::take(&mut self.events)
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

#[test]
fn manager_start_all_succeeds() {
    let mut mgr = InputManager::new();
    mgr.add_source(Box::new(MockAudioSource::new(0.1)));
    mgr.add_source(Box::new(MockScreenSource::new(1)));

    mgr.start_all().expect("start_all should succeed");
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

    let first = mgr.drain_events();
    assert_eq!(first.len(), 2);

    let second = mgr.drain_events();
    assert!(second.is_empty(), "events should drain exactly once");
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
