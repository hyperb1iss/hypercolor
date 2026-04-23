#![cfg(feature = "servo")]

use std::fs;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

#[path = "support/effect_engine.rs"]
mod effect_engine;

use hypercolor_core::effect::{
    bundled_effects_root, load_html_effect_file, parse_html_effect_metadata,
};
use hypercolor_types::audio::AudioData;
use hypercolor_types::canvas::Canvas;
use hypercolor_types::effect::{EffectCategory, EffectId, EffectMetadata, EffectSource};
use tempfile::tempdir;
use uuid::Uuid;

use effect_engine::EffectEngine;

const FRAME_DT_SECONDS: f32 = 1.0 / 60.0;
const AUDIO_TEST_FRAMES: usize = 120;
const BASS_END: usize = 40;
const MID_END: usize = 130;
const SHOCKWAVE_PIXEL_THRESHOLD: u8 = 80;
const SHOCKWAVE_DELTA_THRESHOLD: u8 = 56;
const SHOCKWAVE_SURGE_PIXEL_COUNT: usize = 180;
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

fn bundled_html_metadata(relative: &str) -> EffectMetadata {
    let path = bundled_effects_root().join(relative);
    assert!(
        path.exists(),
        "expected generated HTML effect at {}; run `just effects-build` first",
        path.display()
    );

    let entry = load_html_effect_file(&path)
        .unwrap_or_else(|error| panic!("failed to load {}: {}", path.display(), error.message))
        .unwrap_or_else(|| panic!("expected {} to load as an HTML effect", path.display()));

    entry.metadata
}

fn render_audio_sequence(metadata: EffectMetadata, sequence: &[AudioData]) -> Vec<Canvas> {
    let mut engine = EffectEngine::new();
    engine
        .activate_metadata(metadata)
        .expect("servo activation should succeed for audio-reactive effect");

    sequence
        .iter()
        .map(|audio| {
            let frame = engine
                .tick(FRAME_DT_SECONDS, audio)
                .expect("servo tick should produce a frame");
            thread::sleep(Duration::from_millis(16));
            frame
        })
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct SequenceActivityMetrics {
    average_dynamic_pixels: f32,
    average_delta_pixels: f32,
    max_surge_gap_frames: usize,
    surge_frames: usize,
}

fn shockwave_emitters(canvas: &Canvas) -> [(f32, f32); 3] {
    let width = canvas.width() as f32;
    let height = canvas.height() as f32;
    [
        (width * 0.5, height * 0.16),
        (width * 0.23, height * 0.58),
        (width * 0.77, height * 0.58),
    ]
}

fn shockwave_dynamic_pixels(canvas: &Canvas) -> usize {
    let emitters = shockwave_emitters(canvas);
    let exclusion_radius_sq = 20.0_f32 * 20.0_f32;
    let width = canvas.width() as usize;

    canvas
        .pixels()
        .enumerate()
        .filter(|(index, [r, g, b, _])| {
            if (*r).max(*g).max(*b) < SHOCKWAVE_PIXEL_THRESHOLD {
                return false;
            }

            let x = (*index % width) as f32;
            let y = (*index / width) as f32;

            emitters.iter().all(|(cx, cy)| {
                let dx = x - cx;
                let dy = y - cy;
                dx * dx + dy * dy > exclusion_radius_sq
            })
        })
        .count()
}

fn shockwave_delta_pixels(previous: &Canvas, current: &Canvas) -> usize {
    let emitters = shockwave_emitters(current);
    let exclusion_radius_sq = 20.0_f32 * 20.0_f32;
    let width = current.width() as usize;

    previous
        .pixels()
        .zip(current.pixels())
        .enumerate()
        .filter(|(index, (before, after))| {
            let x = (*index % width) as f32;
            let y = (*index / width) as f32;
            if emitters.iter().any(|(cx, cy)| {
                let dx = x - cx;
                let dy = y - cy;
                dx * dx + dy * dy <= exclusion_radius_sq
            }) {
                return false;
            }

            let max_delta = before[0]
                .abs_diff(after[0])
                .max(before[1].abs_diff(after[1]))
                .max(before[2].abs_diff(after[2]));
            max_delta >= SHOCKWAVE_DELTA_THRESHOLD
        })
        .count()
}

fn sequence_activity_metrics(frames: &[Canvas]) -> SequenceActivityMetrics {
    let mut dynamic_pixel_total = 0_usize;
    let mut delta_pixel_total = 0_usize;
    let mut surge_frames = 0_usize;
    let mut surge_gap = 0_usize;
    let mut max_surge_gap_frames = 0_usize;

    for (index, frame) in frames.iter().enumerate() {
        let dynamic_pixels = shockwave_dynamic_pixels(frame);
        dynamic_pixel_total += dynamic_pixels;

        if let Some(previous) = index.checked_sub(1).and_then(|prev| frames.get(prev)) {
            let delta_pixels = shockwave_delta_pixels(previous, frame);
            delta_pixel_total += delta_pixels;

            if delta_pixels >= SHOCKWAVE_SURGE_PIXEL_COUNT {
                surge_frames += 1;
                surge_gap = 0;
            } else {
                surge_gap += 1;
                max_surge_gap_frames = max_surge_gap_frames.max(surge_gap);
            }
        }
    }

    let frame_count = frames.len().max(1) as f32;
    SequenceActivityMetrics {
        average_dynamic_pixels: dynamic_pixel_total as f32 / frame_count,
        average_delta_pixels: delta_pixel_total as f32 / frame_count,
        max_surge_gap_frames,
        surge_frames,
    }
}

fn music_like_audio_frame(frame_index: usize) -> AudioData {
    let mut audio = AudioData::silence();
    let beat_period = 12;
    let phase = frame_index % beat_period;
    let beat = phase == 0;

    let transient = ((beat_period - phase) as f32 / beat_period as f32).powf(1.6);
    let sway = ((frame_index as f32) * 0.19).sin() * 0.5 + 0.5;

    let bass = (0.34 + transient * 0.52).clamp(0.0, 1.0);
    let mid = (0.16 + transient * 0.18 + sway * 0.08).clamp(0.0, 1.0);
    let treble = (0.05 + transient * 0.08 + (1.0 - sway) * 0.05).clamp(0.0, 1.0);

    for value in &mut audio.spectrum[..BASS_END] {
        *value = bass;
    }
    for value in &mut audio.spectrum[BASS_END..MID_END] {
        *value = mid;
    }
    for value in &mut audio.spectrum[MID_END..] {
        *value = treble;
    }

    audio.rms_level = (0.16 + transient * 0.28).clamp(0.0, 1.0);
    audio.peak_level = (audio.rms_level * 1.45).clamp(0.0, 1.0);
    audio.spectral_centroid = (0.24 + treble * 0.5).clamp(0.0, 1.0);
    audio.spectral_flux = (0.14 + transient * 0.68).clamp(0.0, 1.0);
    audio.beat_detected = beat;
    audio.beat_confidence = 0.86;
    audio.beat_phase = phase as f32 / beat_period as f32;
    audio.beat_pulse = match phase {
        0 => 1.0,
        1 => 0.72,
        2 => 0.5,
        3 => 0.38,
        _ => 0.0,
    };
    audio.bpm = 120.0;
    audio.onset_detected = phase <= 1;
    audio.onset_pulse = match phase {
        0 => 1.0,
        1 => 0.55,
        _ => 0.0,
    };

    audio
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
    let entries = match fs::read_dir(&root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(error) => {
            panic!(
                "failed to read effects directory {}: {error}",
                root.display()
            )
        }
    };
    let mut paths: Vec<PathBuf> = entries
        .map(|entry| {
            entry
                .unwrap_or_else(|error| {
                    panic!(
                        "failed to read effects directory {}: {error}",
                        root.display()
                    )
                })
                .path()
        })
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
    let generated_paths = effect_paths_in("hypercolor");
    let webgl_paths = webgl_effect_paths(&generated_paths);
    assert!(
        !webgl_paths.is_empty(),
        "generated catalog should include at least one WebGL effect"
    );
}

#[test]
#[ignore = "manual Servo regression; run with --features servo after just effects-build"]
fn servo_renderer_audio_reactive_shockwave_prefers_live_music_over_fallback_pulse() {
    let metadata = bundled_html_metadata("hypercolor/shockwave.html");
    let active_audio: Vec<_> = (0..AUDIO_TEST_FRAMES).map(music_like_audio_frame).collect();
    let silence_audio = vec![AudioData::silence(); AUDIO_TEST_FRAMES];

    let active_frames = render_audio_sequence(metadata.clone(), &active_audio);
    let silence_frames = render_audio_sequence(metadata, &silence_audio);

    let active = sequence_activity_metrics(&active_frames);
    let silence = sequence_activity_metrics(&silence_frames);

    assert!(
        active.average_delta_pixels > silence.average_delta_pixels * 1.35,
        "expected live audio to create stronger frame-to-frame shockwave surges than fallback mode: active={active:?} silence={silence:?}"
    );
    assert!(
        active.average_delta_pixels > 6_000.0,
        "expected live audio to produce a meaningful amount of Shockwave surge motion: active={active:?} silence={silence:?}"
    );
}
