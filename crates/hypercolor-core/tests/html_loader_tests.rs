//! Tests for HTML effect discovery/loader integration.

use std::fs;
use std::path::Path;

use tempfile::TempDir;

use hypercolor_core::effect::{
    EffectRegistry, builtin::register_builtin_effects, bundled_effects_root,
    default_effect_search_paths, html_path_effect_id_for_testing, load_html_effect_file,
    parse_html_effect_metadata, register_html_effects,
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
  <meta audio-reactive="true" />
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
fn bundled_html_effect_ids_are_stable_across_build_roots() {
    let temp = TempDir::new().expect("failed to create tempdir");
    let root = temp.path().join("effects");
    let html = r#"
<head>
  <title>Poisonous</title>
  <meta description="Stable generated effect identity" />
  <meta publisher="Hypercolor" />
</head>
"#;
    let source_path = root.join("hypercolor/poisonous.html");
    let installed_path = root.join("bundled/poisonous.html");
    write_html(&source_path, html);
    write_html(&installed_path, html);

    let source_entry = load_html_effect_file(&source_path)
        .expect("source effect should load")
        .expect("source effect should register");
    let installed_entry = load_html_effect_file(&installed_path)
        .expect("installed effect should load")
        .expect("installed effect should register");

    assert_eq!(source_entry.metadata.id, installed_entry.metadata.id);
}

#[test]
fn registry_resolves_legacy_bundled_html_path_aliases() {
    let temp = TempDir::new().expect("failed to create tempdir");
    let root = temp.path().join("effects");
    let html = r#"
<head>
  <title>Poisonous</title>
  <meta description="Stable generated effect identity" />
  <meta publisher="Hypercolor" />
</head>
"#;
    let source_path = root.join("hypercolor/poisonous.html");
    let installed_path = root.join("bundled/poisonous.html");
    write_html(&source_path, html);
    write_html(&installed_path, html);

    let installed_entry = load_html_effect_file(&installed_path)
        .expect("installed effect should load")
        .expect("installed effect should register");
    let canonical_id = installed_entry.metadata.id;
    let legacy_id = html_path_effect_id_for_testing(&source_path);
    assert_ne!(legacy_id, canonical_id);

    let mut registry = EffectRegistry::new(vec![root]);
    registry.register(installed_entry);

    assert_eq!(registry.resolve_id(&legacy_id), Some(canonical_id));
    let effect = registry
        .get(&legacy_id)
        .expect("legacy path id should resolve to the installed effect");
    assert_eq!(effect.metadata.name, "Poisonous");
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
fn register_html_effects_respects_explicit_display_category() {
    let temp = TempDir::new().expect("failed to create tempdir");
    let root = temp.path().join("effects");

    write_html(
        &root.join("faces/system-monitor.html"),
        r#"
<head>
  <title>System Monitor Face</title>
  <meta description="Display dashboard" />
  <meta publisher="Hypercolor" />
  <meta category="display" />
</head>
"#,
    );

    let mut registry = EffectRegistry::new(vec![root.clone()]);
    let report = register_html_effects(&mut registry, &[root]);

    assert_eq!(report.failed_files(), 0);

    let effect = registry
        .search("System Monitor Face")
        .into_iter()
        .next()
        .expect("display face should be loaded");
    assert_eq!(effect.metadata.category, EffectCategory::Display);
}

#[test]
fn registry_rescan_preserves_builtin_native_effects() {
    let temp = TempDir::new().expect("failed to create tempdir");
    let root = temp.path().join("effects");

    write_html(
        &root.join("community/aurora.html"),
        r#"
<head>
  <title>Aurora</title>
  <meta description="Northern lights" />
  <meta publisher="Hypercolor" />
</head>
"#,
    );

    let mut registry = EffectRegistry::new(vec![root.clone()]);
    register_builtin_effects(&mut registry);
    let builtin_count_before = registry
        .iter()
        .filter(|(_, entry)| matches!(entry.metadata.source, EffectSource::Native { .. }))
        .count();

    let initial_report = register_html_effects(&mut registry, &[root]);
    assert_eq!(initial_report.loaded_effects, 1);

    let rescan_report = registry.rescan();
    assert_eq!(rescan_report.removed, 0);

    let builtin_count_after = registry
        .iter()
        .filter(|(_, entry)| matches!(entry.metadata.source, EffectSource::Native { .. }))
        .count();

    assert_eq!(builtin_count_after, builtin_count_before);
    assert!(
        registry
            .iter()
            .any(|(_, entry)| entry.metadata.name == "Audio Pulse"),
        "builtin audio pulse should survive a manual rescan"
    );
    assert!(
        registry
            .iter()
            .any(|(_, entry)| entry.metadata.name == "Aurora"),
        "html effects should still be present after a manual rescan"
    );
}

#[test]
fn registry_reload_single_does_not_rescan_sibling_effects() {
    let temp = TempDir::new().expect("failed to create tempdir");
    let root = temp.path().join("effects");
    let aurora_path = root.join("aurora.html");
    let nebula_path = root.join("nebula.html");

    write_html(
        &aurora_path,
        r#"
<head>
  <title>Aurora</title>
  <meta description="Northern lights" />
  <meta publisher="Hypercolor" />
</head>
"#,
    );
    write_html(
        &nebula_path,
        r#"
<head>
  <title>Nebula</title>
  <meta description="Space haze" />
  <meta publisher="Hypercolor" />
</head>
"#,
    );

    let mut registry = EffectRegistry::new(vec![root.clone()]);
    let initial_report = register_html_effects(&mut registry, &[root]);
    assert_eq!(initial_report.loaded_effects, 2);

    write_html(
        &nebula_path,
        r#"
<head>
  <title>Nebula Reloaded</title>
  <meta description="Space haze" />
  <meta publisher="Hypercolor" />
</head>
"#,
    );

    let report = registry.reload_single(&aurora_path);

    assert_eq!(report.added, 0);
    assert_eq!(report.removed, 0);
    assert_eq!(report.updated, 1);
    assert!(
        registry.search("Nebula Reloaded").is_empty(),
        "single-file reload should not refresh sibling HTML effects"
    );
    assert!(
        registry
            .iter()
            .any(|(_, entry)| entry.metadata.name == "Nebula"),
        "unchanged sibling registry entry should keep its previous metadata"
    );
}

#[test]
fn registry_reload_single_removes_deleted_effect_from_noncanonical_path() {
    let temp = TempDir::new().expect("failed to create tempdir");
    let root = temp.path().join("effects");
    let aurora_path = root.join("aurora.html");

    write_html(
        &aurora_path,
        r#"
<head>
  <title>Aurora</title>
  <meta description="Northern lights" />
  <meta publisher="Hypercolor" />
</head>
"#,
    );

    let mut registry = EffectRegistry::new(vec![root.clone()]);
    let initial_report = register_html_effects(&mut registry, std::slice::from_ref(&root));
    assert_eq!(initial_report.loaded_effects, 1);
    assert_eq!(registry.len(), 1);

    fs::remove_file(&aurora_path).expect("effect file should be deleted");

    let watcher_style_path = root.join("nested").join("..").join("aurora.html");
    let report = registry.reload_single(&watcher_style_path);

    assert_eq!(report.removed, 1);
    assert_eq!(registry.len(), 0);
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

#[test]
fn parse_html_effect_metadata_reads_builtin_and_screen_reactive_meta() {
    let html = r#"
<head>
  <title>Screen Cast</title>
  <meta builtin-id="screen_cast" />
  <meta screen-reactive="true" />
</head>
"#;

    let parsed = parse_html_effect_metadata(html);

    assert_eq!(parsed.builtin_id.as_deref(), Some("screen_cast"));
    assert!(parsed.screen_reactive);
    assert_eq!(parsed.category, EffectCategory::Utility);
    assert!(parsed.tags.contains(&"screen".to_owned()));
    assert!(parsed.tags.contains(&"screen-reactive".to_owned()));
}

#[cfg(not(feature = "servo"))]
#[test]
fn register_html_effects_skips_builtin_html_ports_without_servo() {
    let temp = TempDir::new().expect("failed to create tempdir");
    let root = temp.path().join("effects");

    write_html(
        &root.join("hypercolor/screen-cast.html"),
        r#"
<head>
  <title>Screen Cast</title>
  <meta description="HTML builtin port" />
  <meta publisher="Hypercolor" />
  <meta builtin-id="screen_cast" />
  <meta screen-reactive="true" />
</head>
"#,
    );

    let mut registry = EffectRegistry::new(vec![root.clone()]);
    let report = register_html_effects(&mut registry, &[root]);

    assert_eq!(report.scanned_files, 1);
    assert_eq!(report.loaded_effects, 0);
    assert_eq!(report.skipped_files, 1);
    assert_eq!(registry.len(), 0);
}

#[test]
fn generated_audio_effects_keep_audio_reactive_metadata() {
    let root = bundled_effects_root();
    for file_name in [
        "audio-pulse.html",
        "frequency-cascade.html",
        "iris.html",
        "shockwave.html",
    ] {
        let path = [
            root.join(file_name),
            root.join("hypercolor").join(file_name),
        ]
        .into_iter()
        .find(|path| path.exists())
        .unwrap_or_else(|| root.join(file_name));
        assert!(
            path.exists(),
            "expected generated HTML effect at {}; run `just effects-build` first",
            path.display()
        );

        let html = fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
        let parsed = parse_html_effect_metadata(&html);

        assert!(
            parsed.audio_reactive,
            "expected {} to remain audio-reactive in generated metadata",
            file_name
        );
        assert!(
            parsed
                .tags
                .iter()
                .any(|tag| tag.eq_ignore_ascii_case("audio-reactive")),
            "expected {} to retain the audio-reactive tag",
            file_name
        );
    }
}
