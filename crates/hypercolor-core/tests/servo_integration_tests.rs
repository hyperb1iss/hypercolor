#![cfg(feature = "servo")]

use std::fs;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use hypercolor_core::effect::{EffectEngine, bundled_effects_root, parse_html_effect_metadata};
use hypercolor_types::audio::AudioData;
use hypercolor_types::canvas::Canvas;
use hypercolor_types::effect::{EffectCategory, EffectId, EffectMetadata, EffectSource};
use tempfile::tempdir;
use uuid::Uuid;

const FRAME_DT_SECONDS: f32 = 1.0 / 60.0;
const BUILTIN_EFFECTS: &[&str] = &[
    "builtin/Rainbow.html",
    "builtin/Solid Color.html",
    "builtin/Side To Side.html",
    "builtin/Neon Shift.html",
    "builtin/Screen Ambience.html",
];
const COMMUNITY_SAMPLE_SIZE: usize = 20;

fn html_metadata(path: PathBuf) -> EffectMetadata {
    let name = path
        .file_stem()
        .and_then(|value| value.to_str())
        .map_or_else(|| "servo-smoke".to_owned(), ToOwned::to_owned);

    EffectMetadata {
        id: EffectId::new(Uuid::now_v7()),
        name,
        author: "hypercolor-tests".to_owned(),
        version: "0.1.0".to_owned(),
        description: "servo smoke test".to_owned(),
        category: EffectCategory::Ambient,
        tags: vec!["servo".to_owned(), "smoke".to_owned()],
        controls: Vec::new(),
        presets: Vec::new(),
        audio_reactive: false,
        screen_reactive: false,
        source: EffectSource::Html { path },
        license: None,
    }
}

fn render_frames(path: &Path, frame_count: usize) -> Vec<Canvas> {
    let mut engine = EffectEngine::new();
    engine
        .activate_metadata(html_metadata(path.to_path_buf()))
        .unwrap_or_else(|error| {
            panic!(
                "servo activation should succeed for {}: {error}",
                path.display()
            )
        });

    (0..frame_count)
        .map(|_| {
            let frame = engine
                .tick(FRAME_DT_SECONDS, &AudioData::silence())
                .expect("servo tick should produce a frame");
            thread::sleep(Duration::from_millis(16));
            frame
        })
        .collect()
}

fn assert_dimensions(canvas: &Canvas) {
    assert_eq!(canvas.width(), 320);
    assert_eq!(canvas.height(), 200);
}

fn frame_contains_red_pixel(canvas: &Canvas) -> bool {
    canvas
        .pixels()
        .any(|[r, g, b, _]| r >= 200 && g <= 80 && b <= 80)
}

fn frame_contains_green_pixel(canvas: &Canvas) -> bool {
    canvas
        .pixels()
        .any(|[r, g, b, _]| g >= 200 && r <= 80 && b <= 80)
}

fn frame_has_spatial_variance(canvas: &Canvas) -> bool {
    let top_left = canvas.get_pixel(0, 0);
    let top_right = canvas.get_pixel(canvas.width() - 1, 0);
    let bottom_left = canvas.get_pixel(0, canvas.height() - 1);
    let bottom_right = canvas.get_pixel(canvas.width() - 1, canvas.height() - 1);
    top_left != top_right || top_left != bottom_left || top_left != bottom_right
}

fn effect_paths_in(bucket: &str) -> Vec<PathBuf> {
    let root = bundled_effects_root().join(bucket);
    let mut paths: Vec<PathBuf> = fs::read_dir(&root)
        .unwrap_or_else(|error| {
            panic!(
                "failed to read effects directory {}: {error}",
                root.display()
            )
        })
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("html"))
        })
        .collect();
    paths.sort();
    paths
        .into_iter()
        .map(|path| {
            path.strip_prefix(bundled_effects_root())
                .map_or(path.clone(), Path::to_path_buf)
        })
        .collect()
}

fn sampled_paths(paths: &[PathBuf], count: usize) -> Vec<PathBuf> {
    if paths.len() <= count {
        return paths.to_vec();
    }

    let stride = (paths.len() / count).max(1);
    paths.iter().step_by(stride).take(count).cloned().collect()
}

fn webgl_effect_paths(paths: &[PathBuf]) -> Vec<PathBuf> {
    let root = bundled_effects_root();
    paths
        .iter()
        .filter_map(|relative_path| {
            let absolute_path = root.join(relative_path);
            let html = fs::read_to_string(&absolute_path).ok()?;
            let parsed = parse_html_effect_metadata(&html);
            if parsed.uses_webgl {
                Some(relative_path.clone())
            } else {
                None
            }
        })
        .collect()
}

#[test]
#[ignore = "requires full Servo runtime and is expensive in CI/dev loops"]
fn servo_renderer_smoke_renders_temp_html_effect() {
    let tmp = tempdir().expect("tempdir should create");
    let html_path = tmp.path().join("smoke.html");
    let html = r#"<!doctype html>
<html>
<body style="margin:0;background:black;">
<canvas id="fx" width="320" height="200"></canvas>
<script>
const canvas = document.getElementById('fx');
const ctx = canvas.getContext('2d');
ctx.fillStyle = 'rgb(255,0,0)';
ctx.fillRect(0, 0, canvas.width, canvas.height);
</script>
</body>
</html>"#;
    std::fs::write(&html_path, html).expect("html write should work");

    let frames = render_frames(&html_path, 5);
    assert!(
        frames
            .iter()
            .all(|canvas| canvas.width() == 320 && canvas.height() == 200)
    );
    assert!(
        frames.iter().any(frame_contains_red_pixel),
        "expected at least one frame to contain strong red output from smoke effect"
    );
}

#[test]
#[ignore = "requires full Servo runtime and is expensive in CI/dev loops"]
fn servo_renderer_smoke_renders_temp_webgl_effect() {
    let tmp = tempdir().expect("tempdir should create");
    let html_path = tmp.path().join("smoke-webgl.html");
    let html = r#"<!doctype html>
<html>
<body style="margin:0;background:black;">
<canvas id="fx" width="320" height="200"></canvas>
<script>
const canvas = document.getElementById('fx');
const gl = canvas.getContext('webgl2', { preserveDrawingBuffer: true });
if (!gl) {
  throw new Error('WebGL2 not supported');
}
gl.viewport(0, 0, canvas.width, canvas.height);
gl.clearColor(1, 0, 0, 1);
gl.clear(gl.COLOR_BUFFER_BIT);
</script>
</body>
</html>"#;
    std::fs::write(&html_path, html).expect("html write should work");

    let frames = render_frames(&html_path, 8);
    assert!(
        frames
            .iter()
            .all(|canvas| canvas.width() == 320 && canvas.height() == 200)
    );
    assert!(
        frames.iter().any(frame_contains_red_pixel),
        "expected at least one frame to contain strong red output from temp WebGL effect"
    );
}

#[test]
#[ignore = "requires full Servo runtime and is expensive in CI/dev loops"]
fn servo_renderer_smoke_activates_generated_webgl_effect_sample() {
    let frames = render_frames(Path::new("hypercolor/arc-storm.html"), 4);
    for frame in frames {
        assert_dimensions(&frame);
    }
}

#[test]
#[ignore = "manual macOS recovery check for fatal WebGL worker retirement"]
fn servo_renderer_recovers_after_fatal_webgl_activation_failure() {
    let mut engine = EffectEngine::new();
    let first = engine.activate_metadata(html_metadata(PathBuf::from(
        "custom/cellular-automaton.html",
    )));

    let Err(first_error) = first else {
        return;
    };
    let first_message = first_error.to_string();
    if !first_message.contains("Disconnected")
        && !first_message.contains("timed out waiting for Servo page load completion")
    {
        return;
    }

    engine
        .activate_metadata(html_metadata(PathBuf::from("hypercolor/arc-storm.html")))
        .expect("Arc Storm should activate with a fresh Servo worker after retirement");
}

#[test]
#[ignore = "requires full Servo runtime and is expensive in CI/dev loops"]
fn servo_renderer_smoke_switches_between_html_effects() {
    let tmp = tempdir().expect("tempdir should create");
    let first_path = tmp.path().join("first.html");
    let second_path = tmp.path().join("second.html");

    std::fs::write(
        &first_path,
        r#"<!doctype html>
<html>
<body style="margin:0;background:black;">
<canvas id="fx" width="320" height="200"></canvas>
<script>
const canvas = document.getElementById('fx');
const ctx = canvas.getContext('2d');
ctx.fillStyle = 'rgb(255,0,0)';
ctx.fillRect(0, 0, canvas.width, canvas.height);
</script>
</body>
</html>"#,
    )
    .expect("first html write should work");

    std::fs::write(
        &second_path,
        r#"<!doctype html>
<html>
<body style="margin:0;background:black;">
<canvas id="fx" width="320" height="200"></canvas>
<script>
const canvas = document.getElementById('fx');
const ctx = canvas.getContext('2d');
ctx.fillStyle = 'rgb(0,255,0)';
ctx.fillRect(0, 0, canvas.width, canvas.height);
</script>
</body>
</html>"#,
    )
    .expect("second html write should work");

    let mut engine = EffectEngine::new();
    engine
        .activate_metadata(html_metadata(first_path))
        .expect("first servo activation should succeed");

    let first_frame = engine
        .tick(FRAME_DT_SECONDS, &AudioData::silence())
        .expect("first servo tick should produce a frame");
    assert_dimensions(&first_frame);
    assert!(
        frame_contains_red_pixel(&first_frame),
        "expected the first effect to render a red frame"
    );

    engine
        .activate_metadata(html_metadata(second_path))
        .expect("second servo activation should succeed after reusing the worker");

    let second_frame = engine
        .tick(FRAME_DT_SECONDS, &AudioData::silence())
        .expect("second servo tick should produce a frame");
    assert_dimensions(&second_frame);
    assert!(
        frame_contains_green_pixel(&second_frame),
        "expected the second effect to render a green frame after effect switching"
    );
}

#[test]
#[ignore = "requires full Servo runtime and is expensive in CI/dev loops"]
fn servo_renderer_smoke_renders_builtin_catalog_sample() {
    let rainbow_frames = render_frames(Path::new("builtin/Rainbow.html"), 3);
    assert!(
        rainbow_frames.iter().any(frame_has_spatial_variance),
        "rainbow effect should produce spatially varying pixels"
    );

    for relative in BUILTIN_EFFECTS {
        let frames = render_frames(Path::new(relative), 3);
        assert!(
            frames
                .iter()
                .all(|canvas| canvas.width() == 320 && canvas.height() == 200)
        );
    }
}

#[test]
#[ignore = "requires full Servo runtime and is expensive in CI/dev loops"]
fn servo_renderer_smoke_renders_sampled_community_effects() {
    let community_paths = effect_paths_in("community");
    let sampled = sampled_paths(&community_paths, COMMUNITY_SAMPLE_SIZE);
    assert_eq!(sampled.len(), COMMUNITY_SAMPLE_SIZE);

    for relative in sampled {
        let frames = render_frames(&relative, 2);
        assert!(
            frames
                .iter()
                .all(|canvas| canvas.width() == 320 && canvas.height() == 200)
        );
    }
}

#[test]
#[ignore = "requires full Servo runtime and is expensive in CI/dev loops"]
fn servo_renderer_smoke_renders_webgl_effects() {
    let custom_paths = effect_paths_in("custom");
    let webgl_paths = webgl_effect_paths(&custom_paths);
    assert!(
        !webgl_paths.is_empty(),
        "expected at least one custom effect tagged as WebGL"
    );
    for relative in webgl_paths {
        let frames = render_frames(&relative, 2);
        for frame in frames {
            assert_dimensions(&frame);
        }
    }
}

#[test]
fn webgl_catalog_selection_finds_entries() {
    let custom_paths = effect_paths_in("custom");
    let webgl_paths = webgl_effect_paths(&custom_paths);
    assert!(
        !webgl_paths.is_empty(),
        "custom catalog should include at least one WebGL effect"
    );
}
