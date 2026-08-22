//! Tests for the renderer trait and effect registry.

use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::SystemTime;

use hypercolor_core::effect::{
    EffectEntry, EffectRegistry, EffectRenderer, FrameInput, create_renderer_for_metadata,
};
use hypercolor_core::input::InteractionData;
use hypercolor_types::audio::AudioData;
use hypercolor_types::canvas::{Canvas, DEFAULT_CANVAS_HEIGHT, DEFAULT_CANVAS_WIDTH};
use hypercolor_types::control::ControlDeltaBatch;
use hypercolor_types::effect::{
    EffectCategory, EffectId, EffectMetadata, EffectSource, EffectState,
};
use hypercolor_types::sensor::SystemSnapshot;
use uuid::Uuid;

static EMPTY_SENSORS: LazyLock<SystemSnapshot> = LazyLock::new(SystemSnapshot::empty);

// ── Mock Renderer ────────────────────────────────────────────────────────────

/// A test-only renderer that fills the canvas with a configurable color
/// and tracks lifecycle calls for assertion.
struct MockRenderer {
    initialized: bool,
    destroyed: bool,
    tick_count: u64,
    init_error: Option<String>,
    fill_color: [u8; 4],
}

impl MockRenderer {
    fn new() -> Self {
        Self {
            initialized: false,
            destroyed: false,
            tick_count: 0,
            init_error: None,
            fill_color: [255, 0, 128, 255],
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

    fn render_into(&mut self, input: &FrameInput<'_>, canvas: &mut Canvas) -> anyhow::Result<()> {
        self.tick_count += 1;
        if canvas.width() != input.canvas_width || canvas.height() != input.canvas_height {
            *canvas = Canvas::new(input.canvas_width, input.canvas_height);
        }
        let color = hypercolor_types::canvas::Rgba::new(
            self.fill_color[0],
            self.fill_color[1],
            self.fill_color[2],
            self.fill_color[3],
        );
        canvas.fill(color);
        Ok(())
    }

    fn apply_controls(&mut self, _batch: &ControlDeltaBatch<'_>) {}

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
        presets: Vec::new(),
        audio_reactive: false,
        screen_reactive: false,
        input_reactive: false,
        source: EffectSource::Native {
            path: PathBuf::from("native/test-aurora.wgsl"),
        },
        license: Some("Apache-2.0".into()),
    }
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
        presets: Vec::new(),
        audio_reactive: false,
        screen_reactive: false,
        input_reactive: false,
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
            presets: Vec::new(),
            audio_reactive: false,
            screen_reactive: false,
            input_reactive: false,
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
        screen: None,
        sensors: &EMPTY_SENSORS,
        sources: hypercolor_core::effect::FrameDataSources::default(),
        canvas_width: DEFAULT_CANVAS_WIDTH,
        canvas_height: DEFAULT_CANVAS_HEIGHT,
    };

    assert!((input.time_secs - 1.5).abs() < f64::EPSILON);
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
        screen: None,
        sensors: &EMPTY_SENSORS,
        sources: hypercolor_core::effect::FrameDataSources::default(),
        canvas_width: 320,
        canvas_height: 200,
    };
    let cloned = input;
    assert_eq!(cloned.frame_number, input.frame_number);
    assert!((cloned.time_secs - input.time_secs).abs() < f64::EPSILON);
}

// ── Renderer Lifecycle Tests ─────────────────────────────────────────────────

#[test]
fn renderer_initializes_renders_and_destroys_through_the_public_trait() {
    let metadata = sample_metadata();
    let mut renderer = MockRenderer::new();
    renderer
        .init_with_canvas_size(&metadata, 640, 400)
        .expect("renderer initialization should succeed");

    let audio = AudioData::silence();
    let interaction = InteractionData::default();
    let input = FrameInput {
        time_secs: 0.016,
        delta_secs: 0.016,
        frame_number: 0,
        audio: &audio,
        interaction: &interaction,
        screen: None,
        sensors: &EMPTY_SENSORS,
        sources: hypercolor_core::effect::FrameDataSources::default(),
        canvas_width: 640,
        canvas_height: 400,
    };
    let mut canvas = Canvas::new(640, 400);
    let allocation = canvas.as_rgba_bytes().as_ptr();
    renderer
        .render_into(&input, &mut canvas)
        .expect("renderer should produce a frame");

    assert!(renderer.initialized);
    assert_eq!(renderer.tick_count, 1);
    assert_eq!(canvas.as_rgba_bytes().as_ptr(), allocation);
    assert_eq!(canvas.get_pixel(0, 0).r, 255);
    assert_eq!(canvas.get_pixel(0, 0).b, 128);

    renderer.destroy();
    assert!(renderer.destroyed);
}

#[test]
fn renderer_initialization_failure_is_reported_by_the_public_trait() {
    let mut renderer = MockRenderer::new().with_init_error("shader compilation failed");
    let error = renderer
        .init_with_canvas_size(&sample_metadata(), 320, 200)
        .expect_err("renderer initialization should fail");

    assert!(error.to_string().contains("shader compilation failed"));
    assert!(!renderer.initialized);
}

#[test]
fn renderer_factory_selects_builtin_renderer() {
    let metadata = builtin_metadata("solid_color");
    let mut renderer =
        create_renderer_for_metadata(&metadata).expect("built-in renderer should be selected");

    renderer
        .init_with_canvas_size(&metadata, 320, 200)
        .expect("built-in renderer should initialize");
}

#[cfg(not(feature = "servo"))]
#[test]
fn renderer_factory_rejects_html_without_servo_feature() {
    let metadata = EffectMetadata {
        id: EffectId::new(Uuid::now_v7()),
        name: "html-test".to_owned(),
        author: "test".to_owned(),
        version: "0.1.0".to_owned(),
        description: "HTML effect".to_owned(),
        category: EffectCategory::Ambient,
        tags: vec!["html".to_owned()],
        controls: Vec::new(),
        presets: Vec::new(),
        audio_reactive: false,
        screen_reactive: false,
        input_reactive: false,
        source: EffectSource::Html {
            path: PathBuf::from("community/test.html"),
        },
        license: None,
    };

    let error = create_renderer_for_metadata(&metadata)
        .err()
        .expect("html renderer selection should fail without servo");
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
            presets: Vec::new(),
            audio_reactive: false,
            screen_reactive: false,
            input_reactive: false,
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
            presets: Vec::new(),
            audio_reactive: false,
            screen_reactive: false,
            input_reactive: false,
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
fn registry_update_applies_nonsemantic_mutation_without_invalidation() {
    let mut registry = EffectRegistry::default();
    let entry = sample_entry("mutable", EffectCategory::Utility, vec![]);
    let id = entry.metadata.id;

    registry.register(entry);
    let generation_before_mut = registry.generation();

    let changed = registry
        .update(&id, |entry| {
            entry.state = EffectState::Running;
        })
        .expect("should find effect");

    assert!(!changed);
    assert_eq!(
        registry.get(&id).expect("entry").state,
        EffectState::Running
    );
    assert_eq!(
        registry.generation(),
        generation_before_mut,
        "raw mutable access should not invalidate semantic caches on its own"
    );
}

#[test]
fn registry_generation_advances_on_semantic_mutation() {
    let mut registry = EffectRegistry::default();
    assert_eq!(registry.generation(), 0);

    let entry = sample_entry("mutable", EffectCategory::Utility, vec![]);
    let id = entry.metadata.id;
    registry.register(entry);
    let after_register = registry.generation();

    assert!(after_register > 0);

    let changed = registry
        .update(&id, |entry| {
            entry.metadata.audio_reactive = true;
        })
        .expect("semantic update should bump generation");
    assert!(changed);
    let after_update = registry.generation();

    assert!(after_update > after_register);

    let removed = registry.remove(&id);
    assert!(removed.is_some());
    assert!(registry.generation() > after_update);
}

#[test]
fn registry_generation_ignores_noop_semantic_writes() {
    let mut registry = EffectRegistry::default();
    let entry = sample_entry("stable", EffectCategory::Utility, vec![]);
    let id = entry.metadata.id;

    registry.register(entry.clone());
    let after_register = registry.generation();

    let replaced = registry.register(entry);
    assert!(replaced.is_some());
    assert_eq!(
        registry.generation(),
        after_register,
        "re-registering an identical entry should not invalidate active-scene caches"
    );

    let changed = registry
        .update(&id, |stored| {
            stored.metadata.audio_reactive = false;
        })
        .expect("entry should exist");
    assert!(!changed);
    assert_eq!(
        registry.generation(),
        after_register,
        "writing the same semantic metadata should not invalidate active-scene caches"
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
