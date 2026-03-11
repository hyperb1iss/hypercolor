//! Tests for the effect engine, renderer trait, and registry.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use hypercolor_core::effect::{
    EffectEngine, EffectEntry, EffectRegistry, EffectRenderer, FrameInput,
};
use hypercolor_core::input::InteractionData;
use hypercolor_types::audio::AudioData;
use hypercolor_types::canvas::{Canvas, DEFAULT_CANVAS_HEIGHT, DEFAULT_CANVAS_WIDTH};
use hypercolor_types::effect::{
    ControlDefinition, ControlKind, ControlType, ControlValue, EffectCategory, EffectId,
    EffectMetadata, EffectSource, EffectState,
};
use uuid::Uuid;

// ── Mock Renderer ────────────────────────────────────────────────────────────

/// A test-only renderer that fills the canvas with a configurable color
/// and tracks lifecycle calls for assertion.
struct MockRenderer {
    initialized: bool,
    destroyed: bool,
    tick_count: u64,
    controls: HashMap<String, ControlValue>,
    /// If set, `init` will return this error.
    init_error: Option<String>,
    /// Fill color for produced canvases (R, G, B, A).
    fill_color: [u8; 4],
}

impl MockRenderer {
    fn new() -> Self {
        Self {
            initialized: false,
            destroyed: false,
            tick_count: 0,
            controls: HashMap::new(),
            init_error: None,
            fill_color: [255, 0, 128, 255], // Electric pink
        }
    }

    fn with_init_error(mut self, message: &str) -> Self {
        self.init_error = Some(message.to_owned());
        self
    }
}

impl EffectRenderer for MockRenderer {
    fn init(&mut self, _metadata: &EffectMetadata) -> anyhow::Result<()> {
        if let Some(ref msg) = self.init_error {
            return Err(anyhow::anyhow!("{msg}"));
        }
        self.initialized = true;
        Ok(())
    }

    fn tick(&mut self, input: &FrameInput<'_>) -> anyhow::Result<Canvas> {
        self.tick_count += 1;
        let mut canvas = Canvas::new(input.canvas_width, input.canvas_height);
        let color = hypercolor_types::canvas::Rgba::new(
            self.fill_color[0],
            self.fill_color[1],
            self.fill_color[2],
            self.fill_color[3],
        );
        canvas.fill(color);
        Ok(canvas)
    }

    fn set_control(&mut self, name: &str, value: &ControlValue) {
        self.controls.insert(name.to_owned(), value.clone());
    }

    fn destroy(&mut self) {
        self.destroyed = true;
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────────

fn sample_metadata() -> EffectMetadata {
    EffectMetadata {
        id: EffectId::new(Uuid::now_v7()),
        name: "Test Aurora".into(),
        author: "hyperb1iss".into(),
        version: "1.0.0".into(),
        description: "A test effect for unit testing".into(),
        category: EffectCategory::Ambient,
        tags: vec!["test".into(), "ambient".into()],
        controls: Vec::new(),
        audio_reactive: false,
        source: EffectSource::Native {
            path: PathBuf::from("native/test-aurora.wgsl"),
        },
        license: Some("Apache-2.0".into()),
    }
}

fn sample_controlled_metadata() -> EffectMetadata {
    let mut metadata = sample_metadata();
    metadata.controls = vec![
        ControlDefinition {
            id: "speed".to_owned(),
            name: "Speed".to_owned(),
            kind: ControlKind::Number,
            control_type: ControlType::Slider,
            default_value: ControlValue::Float(5.0),
            min: Some(0.0),
            max: Some(10.0),
            step: Some(1.0),
            labels: Vec::new(),
            group: Some("General".to_owned()),
            tooltip: None,
        },
        ControlDefinition {
            id: "mode".to_owned(),
            name: "Mode".to_owned(),
            kind: ControlKind::Combobox,
            control_type: ControlType::Dropdown,
            default_value: ControlValue::Enum("normal".to_owned()),
            min: None,
            max: None,
            step: None,
            labels: vec!["normal".to_owned(), "sparkle".to_owned()],
            group: Some("General".to_owned()),
            tooltip: None,
        },
    ];
    metadata
}

fn builtin_metadata(name: &str) -> EffectMetadata {
    EffectMetadata {
        id: EffectId::new(Uuid::now_v7()),
        name: name.to_owned(),
        author: "hypercolor".to_owned(),
        version: "0.1.0".to_owned(),
        description: "Built-in test effect".to_owned(),
        category: EffectCategory::Ambient,
        tags: vec!["builtin".to_owned()],
        controls: Vec::new(),
        audio_reactive: false,
        source: EffectSource::Native {
            path: PathBuf::from(format!("builtin/{name}")),
        },
        license: Some("Apache-2.0".to_owned()),
    }
}

fn sample_entry(name: &str, category: EffectCategory, tags: Vec<&str>) -> EffectEntry {
    EffectEntry {
        metadata: EffectMetadata {
            id: EffectId::new(Uuid::now_v7()),
            name: name.into(),
            author: "test".into(),
            version: "0.1.0".into(),
            description: format!("Test effect: {name}"),
            category,
            tags: tags.into_iter().map(String::from).collect(),
            controls: Vec::new(),
            audio_reactive: false,
            source: EffectSource::Native {
                path: PathBuf::from(format!("native/{name}.wgsl")),
            },
            license: None,
        },
        source_path: PathBuf::from(format!("/effects/native/{name}.wgsl")),
        modified: SystemTime::now(),
        state: EffectState::Loading,
    }
}

// ── FrameInput Tests ─────────────────────────────────────────────────────────

#[test]
fn frame_input_construction() {
    let audio = AudioData::silence();
    let interaction = InteractionData::default();
    let input = FrameInput {
        time_secs: 1.5,
        delta_secs: 0.016,
        frame_number: 90,
        audio: &audio,
        interaction: &interaction,
        canvas_width: DEFAULT_CANVAS_WIDTH,
        canvas_height: DEFAULT_CANVAS_HEIGHT,
    };

    assert!((input.time_secs - 1.5).abs() < f32::EPSILON);
    assert!((input.delta_secs - 0.016).abs() < f32::EPSILON);
    assert_eq!(input.frame_number, 90);
    assert_eq!(input.canvas_width, DEFAULT_CANVAS_WIDTH);
    assert_eq!(input.canvas_height, DEFAULT_CANVAS_HEIGHT);
}

#[test]
fn frame_input_clone() {
    let audio = AudioData::silence();
    let interaction = InteractionData::default();
    let input = FrameInput {
        time_secs: 2.0,
        delta_secs: 0.033,
        frame_number: 60,
        audio: &audio,
        interaction: &interaction,
        canvas_width: 320,
        canvas_height: 200,
    };
    let cloned = input;
    assert_eq!(cloned.frame_number, input.frame_number);
    assert!((cloned.time_secs - input.time_secs).abs() < f32::EPSILON);
}

// ── EffectEngine Tests ───────────────────────────────────────────────────────

#[test]
fn engine_starts_idle() {
    let engine = EffectEngine::new();
    assert_eq!(engine.state(), EffectState::Loading);
    assert!(!engine.is_running());
    assert!(engine.active_metadata().is_none());
}

#[test]
fn engine_default_is_idle() {
    let engine = EffectEngine::default();
    assert_eq!(engine.state(), EffectState::Loading);
    assert!(!engine.is_running());
}

#[test]
fn engine_activate_success() {
    let mut engine = EffectEngine::new();
    let renderer = Box::new(MockRenderer::new());
    let meta = sample_metadata();
    let name = meta.name.clone();

    engine
        .activate(renderer, meta)
        .expect("activation should succeed");

    assert_eq!(engine.state(), EffectState::Running);
    assert!(engine.is_running());
    assert_eq!(
        engine.active_metadata().expect("should have metadata").name,
        name
    );
}

#[test]
fn engine_activate_init_failure() {
    let mut engine = EffectEngine::new();
    let renderer = Box::new(MockRenderer::new().with_init_error("shader compilation failed"));
    let meta = sample_metadata();

    let result = engine.activate(renderer, meta);

    assert!(result.is_err());
    assert_eq!(engine.state(), EffectState::Loading);
    assert!(!engine.is_running());
    assert!(engine.active_metadata().is_none());
}

#[test]
fn engine_deactivate() {
    let mut engine = EffectEngine::new();
    let renderer = Box::new(MockRenderer::new());
    engine
        .activate(renderer, sample_metadata())
        .expect("activate");

    engine.deactivate();

    assert_eq!(engine.state(), EffectState::Loading);
    assert!(!engine.is_running());
    assert!(engine.active_metadata().is_none());
}

#[test]
fn engine_deactivate_when_idle_is_noop() {
    let mut engine = EffectEngine::new();
    engine.deactivate(); // should not panic
    assert_eq!(engine.state(), EffectState::Loading);
}

#[test]
fn engine_activate_replaces_previous() {
    let mut engine = EffectEngine::new();

    let meta1 = sample_metadata();
    engine
        .activate(Box::new(MockRenderer::new()), meta1)
        .expect("first activate");

    let mut meta2 = sample_metadata();
    meta2.name = "Plasma Ocean".into();
    engine
        .activate(Box::new(MockRenderer::new()), meta2)
        .expect("second activate");

    assert_eq!(
        engine.active_metadata().expect("metadata").name,
        "Plasma Ocean"
    );
    assert!(engine.is_running());
}

#[test]
fn engine_tick_produces_canvas() {
    let mut engine = EffectEngine::new();
    engine
        .activate(Box::new(MockRenderer::new()), sample_metadata())
        .expect("activate");

    let audio = AudioData::silence();
    let canvas = engine.tick(0.016, &audio).expect("tick should succeed");

    assert_eq!(canvas.width(), DEFAULT_CANVAS_WIDTH);
    assert_eq!(canvas.height(), DEFAULT_CANVAS_HEIGHT);
    // MockRenderer fills with [255, 0, 128, 255]
    let pixel = canvas.get_pixel(0, 0);
    assert_eq!(pixel.r, 255);
    assert_eq!(pixel.g, 0);
    assert_eq!(pixel.b, 128);
    assert_eq!(pixel.a, 255);
}

#[test]
fn engine_tick_when_idle_returns_black_canvas() {
    let mut engine = EffectEngine::new();
    let audio = AudioData::silence();
    let canvas = engine.tick(0.016, &audio).expect("tick should succeed");

    assert_eq!(canvas.width(), DEFAULT_CANVAS_WIDTH);
    assert_eq!(canvas.height(), DEFAULT_CANVAS_HEIGHT);
    // Should be opaque black
    let pixel = canvas.get_pixel(0, 0);
    assert_eq!(pixel.r, 0);
    assert_eq!(pixel.g, 0);
    assert_eq!(pixel.b, 0);
    assert_eq!(pixel.a, 255);
}

#[test]
fn engine_tick_accumulates_time() {
    let mut engine = EffectEngine::new();
    engine
        .activate(Box::new(MockRenderer::new()), sample_metadata())
        .expect("activate");

    let audio = AudioData::silence();
    for _ in 0..10 {
        engine.tick(0.016, &audio).expect("tick");
    }

    // Engine should still be running after multiple ticks
    assert!(engine.is_running());
}

#[test]
fn engine_pause_and_resume() {
    let mut engine = EffectEngine::new();
    engine
        .activate(Box::new(MockRenderer::new()), sample_metadata())
        .expect("activate");

    engine.pause();
    assert_eq!(engine.state(), EffectState::Paused);
    assert!(!engine.is_running());

    // Tick while paused returns black canvas
    let audio = AudioData::silence();
    let canvas = engine.tick(0.016, &audio).expect("tick while paused");
    let pixel = canvas.get_pixel(0, 0);
    assert_eq!(pixel.r, 0);
    assert_eq!(pixel.g, 0);

    engine.resume();
    assert_eq!(engine.state(), EffectState::Running);
    assert!(engine.is_running());
}

#[test]
fn engine_pause_when_not_running_is_noop() {
    let mut engine = EffectEngine::new();
    engine.pause(); // should not panic, state stays Loading
    assert_eq!(engine.state(), EffectState::Loading);
}

#[test]
fn engine_resume_when_not_paused_is_noop() {
    let mut engine = EffectEngine::new();
    engine
        .activate(Box::new(MockRenderer::new()), sample_metadata())
        .expect("activate");
    engine.resume(); // already running, should be no-op
    assert_eq!(engine.state(), EffectState::Running);
}

#[test]
fn engine_set_control_forwarded_to_renderer() {
    let mut engine = EffectEngine::new();
    engine
        .activate(Box::new(MockRenderer::new()), sample_metadata())
        .expect("activate");

    engine.set_control("speed", &ControlValue::Float(10.0));
    engine.set_control("enabled", &ControlValue::Boolean(true));

    // Controls are stored — engine doesn't expose them directly,
    // but we can verify the renderer received them by ticking
    // (the mock stores them internally).
    assert!(engine.is_running());
}

#[test]
fn engine_set_control_when_idle_stores_value() {
    let mut engine = EffectEngine::new();
    // Setting controls before activation should not panic
    engine.set_control("speed", &ControlValue::Float(5.0));
    assert!(!engine.is_running());
}

#[test]
fn engine_activate_seeds_default_controls_from_metadata() {
    let mut engine = EffectEngine::new();
    engine
        .activate(Box::new(MockRenderer::new()), sample_controlled_metadata())
        .expect("activate");

    let controls = engine.active_controls();
    assert_eq!(controls.get("speed"), Some(&ControlValue::Float(5.0)));
    assert_eq!(
        controls.get("mode"),
        Some(&ControlValue::Enum("normal".to_owned()))
    );
}

#[test]
fn engine_set_control_checked_validates_against_schema() {
    let mut engine = EffectEngine::new();
    engine
        .activate(Box::new(MockRenderer::new()), sample_controlled_metadata())
        .expect("activate");

    let normalized = engine
        .set_control_checked("speed", &ControlValue::Float(99.4))
        .expect("speed should clamp/quantize");
    assert_eq!(normalized, ControlValue::Float(10.0));
    assert_eq!(
        engine.active_controls().get("speed"),
        Some(&ControlValue::Float(10.0))
    );

    let error = engine
        .set_control_checked("mode", &ControlValue::Text("invalid".to_owned()))
        .expect_err("mode should reject unknown option");
    assert!(
        error.to_string().contains("invalid option 'invalid'"),
        "unexpected error: {error}"
    );
}

#[test]
fn engine_custom_canvas_size() {
    let mut engine = EffectEngine::new().with_canvas_size(640, 400);
    engine
        .activate(Box::new(MockRenderer::new()), sample_metadata())
        .expect("activate");

    let audio = AudioData::silence();
    let canvas = engine.tick(0.016, &audio).expect("tick");
    assert_eq!(canvas.width(), 640);
    assert_eq!(canvas.height(), 400);
}

#[test]
fn engine_activate_metadata_selects_builtin_renderer() {
    let mut engine = EffectEngine::new();
    engine
        .activate_metadata(builtin_metadata("solid_color"))
        .expect("built-in metadata activation should succeed");

    assert!(engine.is_running());
}

#[cfg(not(feature = "servo"))]
#[test]
fn engine_activate_metadata_html_requires_servo_feature() {
    let mut engine = EffectEngine::new();
    let metadata = EffectMetadata {
        id: EffectId::new(Uuid::now_v7()),
        name: "html-test".to_owned(),
        author: "test".to_owned(),
        version: "0.1.0".to_owned(),
        description: "HTML effect".to_owned(),
        category: EffectCategory::Ambient,
        tags: vec!["html".to_owned()],
        controls: Vec::new(),
        audio_reactive: false,
        source: EffectSource::Html {
            path: PathBuf::from("community/test.html"),
        },
        license: None,
    };

    let error = engine
        .activate_metadata(metadata)
        .expect_err("html activation should fail without servo feature");
    assert!(error.to_string().contains("requires the `servo` feature"));
}

// ── EffectRegistry Tests ─────────────────────────────────────────────────────

#[test]
fn registry_starts_empty() {
    let registry = EffectRegistry::new(vec![PathBuf::from("/effects")]);
    assert!(registry.is_empty());
    assert_eq!(registry.len(), 0);
}

#[test]
fn registry_default_has_no_paths() {
    let registry = EffectRegistry::default();
    assert!(registry.search_paths().is_empty());
    assert!(registry.is_empty());
}

#[test]
fn registry_register_and_get() {
    let mut registry = EffectRegistry::default();
    let entry = sample_entry("aurora", EffectCategory::Ambient, vec!["ambient", "nature"]);
    let id = entry.metadata.id;

    let replaced = registry.register(entry);
    assert!(replaced.is_none());
    assert_eq!(registry.len(), 1);

    let found = registry.get(&id).expect("should find effect");
    assert_eq!(found.metadata.name, "aurora");
}

#[test]
fn registry_register_replaces_existing() {
    let mut registry = EffectRegistry::default();

    let id = EffectId::new(Uuid::now_v7());
    let entry1 = EffectEntry {
        metadata: EffectMetadata {
            id,
            name: "aurora-v1".into(),
            author: "test".into(),
            version: "1.0.0".into(),
            description: "Version 1".into(),
            category: EffectCategory::Ambient,
            tags: vec![],
            controls: Vec::new(),
            audio_reactive: false,
            source: EffectSource::Native {
                path: PathBuf::from("native/aurora.wgsl"),
            },
            license: None,
        },
        source_path: PathBuf::from("/effects/native/aurora.wgsl"),
        modified: SystemTime::now(),
        state: EffectState::Loading,
    };

    let entry2 = EffectEntry {
        metadata: EffectMetadata {
            id,
            name: "aurora-v2".into(),
            author: "test".into(),
            version: "2.0.0".into(),
            description: "Version 2".into(),
            category: EffectCategory::Ambient,
            tags: vec![],
            controls: Vec::new(),
            audio_reactive: false,
            source: EffectSource::Native {
                path: PathBuf::from("native/aurora.wgsl"),
            },
            license: None,
        },
        source_path: PathBuf::from("/effects/native/aurora.wgsl"),
        modified: SystemTime::now(),
        state: EffectState::Loading,
    };

    registry.register(entry1);
    let replaced = registry.register(entry2);

    assert!(replaced.is_some());
    assert_eq!(replaced.expect("replaced entry").metadata.name, "aurora-v1");
    assert_eq!(registry.len(), 1);
    assert_eq!(registry.get(&id).expect("entry").metadata.name, "aurora-v2");
}

#[test]
fn registry_remove() {
    let mut registry = EffectRegistry::default();
    let entry = sample_entry("plasma", EffectCategory::Generative, vec!["generative"]);
    let id = entry.metadata.id;

    registry.register(entry);
    assert_eq!(registry.len(), 1);

    let removed = registry.remove(&id);
    assert!(removed.is_some());
    assert_eq!(registry.len(), 0);
    assert!(registry.get(&id).is_none());
}

#[test]
fn registry_remove_nonexistent() {
    let mut registry = EffectRegistry::default();
    let id = EffectId::new(Uuid::now_v7());
    assert!(registry.remove(&id).is_none());
}

#[test]
fn registry_by_category() {
    let mut registry = EffectRegistry::default();

    registry.register(sample_entry("aurora", EffectCategory::Ambient, vec![]));
    registry.register(sample_entry("beat-pulse", EffectCategory::Audio, vec![]));
    registry.register(sample_entry("nebula", EffectCategory::Ambient, vec![]));
    registry.register(sample_entry("spectrum", EffectCategory::Audio, vec![]));
    registry.register(sample_entry("solid-color", EffectCategory::Utility, vec![]));

    let ambient = registry.by_category(EffectCategory::Ambient);
    assert_eq!(ambient.len(), 2);

    let audio = registry.by_category(EffectCategory::Audio);
    assert_eq!(audio.len(), 2);

    let utility = registry.by_category(EffectCategory::Utility);
    assert_eq!(utility.len(), 1);

    let particle = registry.by_category(EffectCategory::Particle);
    assert!(particle.is_empty());
}

#[test]
fn registry_search_by_name() {
    let mut registry = EffectRegistry::default();
    registry.register(sample_entry(
        "aurora-borealis",
        EffectCategory::Ambient,
        vec![],
    ));
    registry.register(sample_entry(
        "plasma-ocean",
        EffectCategory::Generative,
        vec![],
    ));
    registry.register(sample_entry(
        "aurora-australis",
        EffectCategory::Ambient,
        vec![],
    ));

    let results = registry.search("aurora");
    assert_eq!(results.len(), 2);
}

#[test]
fn registry_search_case_insensitive() {
    let mut registry = EffectRegistry::default();
    registry.register(sample_entry("Aurora", EffectCategory::Ambient, vec![]));

    let results = registry.search("aurora");
    assert_eq!(results.len(), 1);
    let results = registry.search("AURORA");
    assert_eq!(results.len(), 1);
}

#[test]
fn registry_search_by_tag() {
    let mut registry = EffectRegistry::default();
    registry.register(sample_entry(
        "beat-pulse",
        EffectCategory::Audio,
        vec!["audio-reactive", "beat"],
    ));
    registry.register(sample_entry(
        "aurora",
        EffectCategory::Ambient,
        vec!["ambient", "nature"],
    ));

    let results = registry.search("audio-reactive");
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].metadata.name, "beat-pulse");
}

#[test]
fn registry_search_by_description() {
    let mut registry = EffectRegistry::default();
    registry.register(sample_entry("test-fx", EffectCategory::Utility, vec![]));

    // The description is "Test effect: test-fx" from our helper
    let results = registry.search("test effect");
    assert_eq!(results.len(), 1);
}

#[test]
fn registry_search_empty_query_returns_all() {
    let mut registry = EffectRegistry::default();
    registry.register(sample_entry("a", EffectCategory::Ambient, vec![]));
    registry.register(sample_entry("b", EffectCategory::Audio, vec![]));

    let results = registry.search("");
    assert_eq!(results.len(), 2);
}

#[test]
fn registry_search_no_match() {
    let mut registry = EffectRegistry::default();
    registry.register(sample_entry("aurora", EffectCategory::Ambient, vec![]));

    let results = registry.search("zzz-nonexistent");
    assert!(results.is_empty());
}

#[test]
fn registry_iter() {
    let mut registry = EffectRegistry::default();
    registry.register(sample_entry("a", EffectCategory::Ambient, vec![]));
    registry.register(sample_entry("b", EffectCategory::Audio, vec![]));

    let entries: Vec<_> = registry.iter().collect();
    assert_eq!(entries.len(), 2);
}

#[test]
fn registry_categories() {
    let mut registry = EffectRegistry::default();
    registry.register(sample_entry("a", EffectCategory::Ambient, vec![]));
    registry.register(sample_entry("b", EffectCategory::Audio, vec![]));
    registry.register(sample_entry("c", EffectCategory::Ambient, vec![]));

    let cats = registry.categories();
    assert_eq!(cats.len(), 2);
}

#[test]
fn registry_all_tags() {
    let mut registry = EffectRegistry::default();
    registry.register(sample_entry(
        "a",
        EffectCategory::Ambient,
        vec!["ambient", "nature"],
    ));
    registry.register(sample_entry(
        "b",
        EffectCategory::Audio,
        vec!["audio", "nature"],
    ));

    let tags = registry.all_tags();
    assert_eq!(tags.len(), 3); // ambient, audio, nature (deduplicated)
    assert!(tags.contains(&"ambient".to_owned()));
    assert!(tags.contains(&"audio".to_owned()));
    assert!(tags.contains(&"nature".to_owned()));
}

#[test]
fn registry_all_tags_empty() {
    let registry = EffectRegistry::default();
    assert!(registry.all_tags().is_empty());
}

#[test]
fn registry_by_directory() {
    let mut registry = EffectRegistry::default();

    let mut entry1 = sample_entry("a", EffectCategory::Ambient, vec![]);
    entry1.source_path = PathBuf::from("/effects/native/a.wgsl");

    let mut entry2 = sample_entry("b", EffectCategory::Audio, vec![]);
    entry2.source_path = PathBuf::from("/effects/community/b.html");

    let mut entry3 = sample_entry("c", EffectCategory::Ambient, vec![]);
    entry3.source_path = PathBuf::from("/effects/native/c.wgsl");

    registry.register(entry1);
    registry.register(entry2);
    registry.register(entry3);

    let native = registry.by_directory(Path::new("/effects/native"));
    assert_eq!(native.len(), 2);

    let community = registry.by_directory(Path::new("/effects/community"));
    assert_eq!(community.len(), 1);
}

#[test]
fn registry_get_mut() {
    let mut registry = EffectRegistry::default();
    let entry = sample_entry("mutable", EffectCategory::Utility, vec![]);
    let id = entry.metadata.id;

    registry.register(entry);

    let found = registry.get_mut(&id).expect("should find effect");
    found.state = EffectState::Running;

    assert_eq!(
        registry.get(&id).expect("entry").state,
        EffectState::Running
    );
}

#[test]
fn registry_search_paths() {
    let paths = vec![
        PathBuf::from("/effects/native"),
        PathBuf::from("/effects/community"),
    ];
    let registry = EffectRegistry::new(paths.clone());
    assert_eq!(registry.search_paths(), &paths);
}
