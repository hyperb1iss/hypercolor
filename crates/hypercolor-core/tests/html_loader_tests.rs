//! Tests for HTML effect discovery/loader integration.

use std::fs;
use std::path::Path;

use tempfile::TempDir;

use hypercolor_core::effect::{
    EffectRegistry, default_effect_search_paths, parse_html_effect_metadata, register_html_effects,
};
use hypercolor_types::canvas::srgb_to_linear;
use hypercolor_types::effect::{EffectCategory, EffectSource};

fn write_html(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("failed to create parent dirs");
    }
    fs::write(path, contents).expect("failed to write html file");
}

#[test]
fn register_html_effects_loads_effects_from_directory_tree() {
    let temp = TempDir::new().expect("failed to create tempdir");
    let root = temp.path().join("effects");

    write_html(
        &root.join("community/aurora.html"),
        r#"
<head>
  <title>Aurora</title>
  <meta description="Northern lights" />
  <meta publisher="Hypercolor" />
  <meta property="speed" label="Speed" type="number" default="50" min="0" max="100" />
</head>
<script>
  const freqs = new Uint8Array(engine.audio.freq);
</script>
"#,
    );

    write_html(
        &root.join("custom/broken-cube.html"),
        r#"
<head>
  <meta name="name" content="Broken Cube" />
  <meta name="description" content="Three.js visualizer" />
  <meta name="author" content="Nova" />
</head>
<script>
  console.log("THREE.WebGLRenderer");
</script>
"#,
    );

    let mut registry = EffectRegistry::new(vec![root.clone()]);
    let report = register_html_effects(&mut registry, std::slice::from_ref(&root));

    assert_eq!(report.scanned_files, 2);
    assert_eq!(report.loaded_effects, 2);
    assert_eq!(report.failed_files(), 0);
    assert_eq!(registry.len(), 2);

    let aurora = registry
        .search("Aurora")
        .into_iter()
        .next()
        .expect("aurora metadata should be loaded");
    assert_eq!(aurora.metadata.category, EffectCategory::Audio);
    assert!(aurora.metadata.tags.contains(&"audio-reactive".to_owned()));

    match &aurora.metadata.source {
        EffectSource::Html { path } => {
            let expected_path = fs::canonicalize(root.join("community/aurora.html"))
                .expect("aurora effect path should canonicalize");
            assert_eq!(path, &expected_path);
        }
        source => panic!("expected html source, got {source:?}"),
    }

    let broken_cube = registry
        .search("Broken Cube")
        .into_iter()
        .next()
        .expect("broken cube metadata should be loaded");
    assert_eq!(broken_cube.metadata.author, "Nova");
}

#[test]
fn register_html_effects_skips_duplicates_from_overlapping_roots() {
    let temp = TempDir::new().expect("failed to create tempdir");
    let root = temp.path().join("effects");

    write_html(
        &root.join("community/single.html"),
        r#"
<head>
  <title>Single</title>
  <meta description="single file" />
  <meta publisher="Hypercolor" />
</head>
"#,
    );

    let mut registry = EffectRegistry::new(vec![root.clone(), root.join("community")]);
    let report = register_html_effects(&mut registry, &[root.clone(), root.join("community")]);

    assert_eq!(report.scanned_files, 2);
    assert_eq!(report.loaded_effects, 1);
    assert_eq!(report.skipped_files, 1);
    assert_eq!(registry.len(), 1);
}

#[test]
fn register_html_effects_reports_unreadable_files() {
    let temp = TempDir::new().expect("failed to create tempdir");
    let root = temp.path().join("effects");

    write_html(
        &root.join("community/good.html"),
        r#"
<head>
  <title>Good</title>
  <meta description="good" />
  <meta publisher="Hypercolor" />
</head>
"#,
    );

    let bad_path = root.join("community/bad.html");
    fs::create_dir_all(bad_path.parent().expect("bad file should have parent"))
        .expect("failed to create parent dir");
    fs::write(&bad_path, [0xff_u8, 0xfe_u8]).expect("failed to write invalid UTF-8 html");

    let mut registry = EffectRegistry::new(vec![root.clone()]);
    let report = register_html_effects(&mut registry, &[root]);

    assert_eq!(report.scanned_files, 2);
    assert_eq!(report.loaded_effects, 1);
    assert_eq!(report.failed_files(), 1);
    assert_eq!(registry.len(), 1);
}

#[test]
fn default_effect_search_paths_deduplicates_extra_roots() {
    let temp = TempDir::new().expect("failed to create tempdir");
    let extra_root = temp.path().join("extra-effects");

    let paths =
        default_effect_search_paths(&[extra_root.clone(), extra_root.clone(), extra_root.clone()]);

    let matches = paths.iter().filter(|path| *path == &extra_root).count();
    assert_eq!(matches, 1);
    assert!(!paths.is_empty());
}

#[test]
fn register_html_effects_decodes_color_defaults_to_linear_rgba() {
    let temp = TempDir::new().expect("failed to create tempdir");
    let root = temp.path().join("effects");

    write_html(
        &root.join("community/color-check.html"),
        r##"
<head>
  <title>Color Check</title>
  <meta description="color defaults" />
  <meta publisher="Hypercolor" />
  <meta property="accent" label="Accent" type="color" default="#808080" />
</head>
"##,
    );

    let mut registry = EffectRegistry::new(vec![root.clone()]);
    let report = register_html_effects(&mut registry, &[root]);

    assert_eq!(report.failed_files(), 0);

    let effect = registry
        .search("Color Check")
        .into_iter()
        .next()
        .expect("color check effect should be loaded");
    let control = effect
        .metadata
        .controls
        .iter()
        .find(|control| control.control_id() == "accent")
        .expect("accent control should exist");

    let hypercolor_types::effect::ControlValue::Color([r, g, b, a]) = control.default_value else {
        panic!("accent control should decode to a color default");
    };

    let expected = srgb_to_linear(128.0 / 255.0);
    assert!((r - expected).abs() < 0.0001);
    assert!((g - expected).abs() < 0.0001);
    assert!((b - expected).abs() < 0.0001);
    assert!((a - 1.0).abs() < f32::EPSILON);
}

#[test]
fn parse_html_effect_metadata_prefers_webgl_when_shared_runtime_contains_2d_fallback() {
    let html = r#"
<head>
  <title>Arc Storm</title>
  <meta description="Shader effect" />
</head>
<script>
  const gl = canvas.getContext('webgl2', { preserveDrawingBuffer: true });
  const errorCtx = canvas.getContext('2d');
</script>
"#;

    let parsed = parse_html_effect_metadata(html);

    assert!(parsed.uses_webgl);
    assert!(!parsed.uses_canvas2d);
    assert!(parsed.tags.contains(&"webgl".to_owned()));
    assert!(!parsed.tags.contains(&"canvas2d".to_owned()));
}

#[test]
fn parse_html_effect_metadata_respects_explicit_renderer_meta() {
    let html = r#"
<head>
  <title>Canvas Override</title>
  <meta renderer="canvas2d" />
</head>
<script>
  const gl = canvas.getContext('webgl2', { preserveDrawingBuffer: true });
</script>
"#;

    let parsed = parse_html_effect_metadata(html);

    assert!(!parsed.uses_webgl);
    assert!(parsed.uses_canvas2d);
    assert!(!parsed.tags.contains(&"webgl".to_owned()));
    assert!(parsed.tags.contains(&"canvas2d".to_owned()));
}
