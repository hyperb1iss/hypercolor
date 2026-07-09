#![cfg(all(target_os = "linux", feature = "servo"))]
//! Servo CSS coverage probes for the face SDK layout system (spec 69 W0.5).
//!
//! Each fixture under `tests/fixtures/css-probes/` paints known colors into
//! known regions iff one CSS feature works; the test renders every probe at
//! both canonical display sizes (480x480 round, 960x160 strip) and samples
//! pixels. The asserted matrix below gates which CSS the SDK layout module
//! may rely on — JS layout over the display descriptor stays the baseline.
//!
//! ## Support matrix (Servo 0.2, software GL — verified by this test)
//!
//! | Probe                | 480x480 | 960x160 |
//! |----------------------|---------|---------|
//! | flex-row             | yes     | yes     |
//! | flex-column          | yes     | yes     |
//! | flex-gap             | yes     | yes     |
//! | grid                 | NO      | NO      |
//! | clip-path-circle     | yes     | yes     |
//! | aspect-media-query   | NO      | NO      |
//! | transform-translate  | yes     | yes     |
//!
//! Consequences for the SDK: flexbox (including gap), transforms, and
//! `clip-path: circle()` (the face circular mask) are safe; CSS grid layout and
//! aspect-ratio media queries are NOT rendered by Servo — grid and
//! shape-adaptive layout must stay JS over the display descriptor.
//!
//! Heavy fixture: set `HYPERCOLOR_RUN_SERVO_CSS_PROBES=1` to run. The child
//! process pattern mirrors the Servo GPU parity test — Servo teardown can
//! abort after the probes have already proven their result, so the child
//! writes a marker file the parent trusts over the exit status.

use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::thread;
use std::time::Duration;

use hypercolor_core::effect::{EffectRenderOutput, EffectRenderer, FrameInput, ServoRenderer};
use hypercolor_core::input::InteractionData;
use hypercolor_types::audio::AudioData;
use hypercolor_types::canvas::Canvas;
use hypercolor_types::effect::{EffectCategory, EffectId, EffectMetadata, EffectSource};
use hypercolor_types::sensor::SystemSnapshot;
use tempfile::tempdir;
use uuid::Uuid;

const RUN_ENV: &str = "HYPERCOLOR_RUN_SERVO_CSS_PROBES";
const CHILD_ENV: &str = "HYPERCOLOR_SERVO_CSS_PROBES_CHILD";
const MARKER_ENV: &str = "HYPERCOLOR_SERVO_CSS_PROBES_MARKER";

const SIZES: [(u32, u32); 2] = [(480, 480), (960, 160)];
const FRAME_DT_SECONDS: f32 = 1.0 / 30.0;
const RENDER_ATTEMPTS: u64 = 240;
const COLOR_TOLERANCE: i16 = 12;

/// Expected matrix — every probe at every size. A Servo upgrade that
/// changes coverage fails this test loudly instead of silently shifting
/// what the SDK may rely on.
const EXPECTED_MATRIX: &str = "\
aspect-media 480x480 fail
aspect-media 960x160 fail
clip-path-circle 480x480 pass
clip-path-circle 960x160 pass
flex-column 480x480 pass
flex-column 960x160 pass
flex-gap 480x480 pass
flex-gap 960x160 pass
flex-row 480x480 pass
flex-row 960x160 pass
grid 480x480 fail
grid 960x160 fail
transform-translate 480x480 pass
transform-translate 960x160 pass
";

const RED: [u8; 3] = [255, 0, 0];
const GREEN: [u8; 3] = [0, 255, 0];
const BLUE: [u8; 3] = [0, 0, 255];
const YELLOW: [u8; 3] = [255, 255, 0];
const BLACK: [u8; 3] = [0, 0, 0];

/// One pixel expectation at a fractional position.
struct Check {
    x_frac: f32,
    y_frac: f32,
    expected: [u8; 3],
}

const fn check(x_frac: f32, y_frac: f32, expected: [u8; 3]) -> Check {
    Check {
        x_frac,
        y_frac,
        expected,
    }
}

fn probe_checks(probe: &str, width: u32, height: u32) -> Vec<Check> {
    match probe {
        "flex-row" => vec![check(0.25, 0.5, RED), check(0.75, 0.5, GREEN)],
        // Gap working pushes the green child past the halfway point and
        // leaves background in the gap; ignored gap puts green at 0.375.
        "flex-gap" => vec![
            check(0.125, 0.5, RED),
            check(0.375, 0.5, BLACK),
            check(0.625, 0.5, GREEN),
        ],
        "flex-column" => vec![check(0.5, 0.25, RED), check(0.5, 0.75, GREEN)],
        "grid" => vec![
            check(0.25, 0.25, RED),
            check(0.75, 0.25, GREEN),
            check(0.25, 0.75, BLUE),
            check(0.75, 0.75, YELLOW),
        ],
        // Center sits inside the circle; the far corners are clipped away
        // on every aspect ratio because the radius is the closest side.
        "clip-path-circle" => vec![
            check(0.5, 0.5, GREEN),
            check(0.02, 0.05, RED),
            check(0.98, 0.05, RED),
        ],
        // Red is the no-media-query fallback; a pass requires one of the
        // two aspect-gated queries to actually fire.
        "aspect-media" => {
            #[allow(clippy::cast_precision_loss)]
            let wide = width as f32 / height as f32 >= 2.0;
            vec![check(0.5, 0.5, if wide { GREEN } else { BLUE })]
        }
        "transform-translate" => vec![check(0.75, 0.75, RED), check(0.25, 0.25, BLACK)],
        other => panic!("unknown probe '{other}'"),
    }
}

const PROBES: [&str; 7] = [
    "aspect-media",
    "clip-path-circle",
    "flex-column",
    "flex-gap",
    "flex-row",
    "grid",
    "transform-translate",
];

#[test]
fn servo_css_probe_matrix_matches_documented_support() {
    if std::env::var_os(RUN_ENV).is_none() {
        eprintln!("set {RUN_ENV}=1 to run the Servo CSS probe fixture");
        return;
    }
    if std::env::var_os(CHILD_ENV).is_none() {
        let marker_dir = tempdir().expect("probe marker temp dir should be created");
        let marker_path = marker_dir.path().join("servo-css-probes.txt");
        let output = Command::new(std::env::current_exe().expect("test binary path"))
            .arg("--exact")
            .arg("servo_css_probe_matrix_matches_documented_support")
            .arg("--nocapture")
            .env(RUN_ENV, "1")
            .env(CHILD_ENV, "1")
            .env(MARKER_ENV, &marker_path)
            .output()
            .expect("Servo CSS probe child process should run");
        print!("{}", String::from_utf8_lossy(&output.stdout));
        let matrix = fs::read_to_string(&marker_path).unwrap_or_else(|_| {
            panic!(
                "probe child produced no matrix; status={}; stderr={}",
                output.status,
                String::from_utf8_lossy(&output.stderr)
            )
        });
        assert_eq!(
            matrix, EXPECTED_MATRIX,
            "Servo CSS support matrix changed — update the documented matrix \
             and re-check what the SDK layout module relies on"
        );
        return;
    }

    run_probe_child();
    unreachable!("probe child exits after writing its matrix");
}

fn run_probe_child() {
    let mut lines = Vec::new();
    for probe in PROBES {
        for (width, height) in SIZES {
            let passed = run_probe(probe, width, height);
            let verdict = if passed { "pass" } else { "fail" };
            println!("css-probe {probe} {width}x{height}: {verdict}");
            lines.push(format!("{probe} {width}x{height} {verdict}"));
        }
    }

    let marker_path = std::env::var_os(MARKER_ENV).expect("probe child should get a marker path");
    fs::write(marker_path, format!("{}\n", lines.join("\n")))
        .expect("probe child should write its matrix");
    // Servo teardown can abort after results are proven; exit cleanly first.
    std::process::exit(0);
}

fn run_probe(probe: &str, width: u32, height: u32) -> bool {
    let html_path = fixture_path(probe);
    let metadata = probe_metadata(probe, html_path);
    let checks = probe_checks(probe, width, height);

    let mut renderer = ServoRenderer::new();
    if renderer
        .init_with_canvas_size(&metadata, width, height)
        .is_err()
    {
        std::mem::forget(renderer);
        return false;
    }

    let mut passed = false;
    for frame_number in 0..RENDER_ATTEMPTS {
        let input = frame_input(frame_number, width, height);
        let Ok(output) = renderer.render_output(&input) else {
            break;
        };
        if let EffectRenderOutput::Cpu(canvas) = output
            && checks_pass(&canvas, &checks)
        {
            passed = true;
            break;
        }
        thread::sleep(Duration::from_millis(16));
    }

    std::mem::forget(renderer);
    passed
}

fn checks_pass(canvas: &Canvas, checks: &[Check]) -> bool {
    checks.iter().all(|check| {
        let pixel = sample(canvas, check.x_frac, check.y_frac);
        pixel
            .iter()
            .zip(check.expected.iter())
            .all(|(actual, expected)| {
                (i16::from(*actual) - i16::from(*expected)).abs() <= { COLOR_TOLERANCE }
            })
    })
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    clippy::as_conversions
)]
fn sample(canvas: &Canvas, x_frac: f32, y_frac: f32) -> [u8; 3] {
    let x = ((canvas.width() as f32 * x_frac) as u32).min(canvas.width() - 1);
    let y = ((canvas.height() as f32 * y_frac) as u32).min(canvas.height() - 1);
    let index = ((y * canvas.width() + x) * 4) as usize;
    let bytes = canvas.as_rgba_bytes();
    [bytes[index], bytes[index + 1], bytes[index + 2]]
}

fn fixture_path(probe: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/css-probes")
        .join(format!("{probe}.html"))
}

fn probe_metadata(probe: &str, path: PathBuf) -> EffectMetadata {
    EffectMetadata {
        id: EffectId::new(Uuid::now_v7()),
        name: format!("css-probe-{probe}"),
        author: "hypercolor-tests".to_owned(),
        version: "0.1.0".to_owned(),
        description: "Servo CSS coverage probe".to_owned(),
        category: EffectCategory::Ambient,
        tags: vec!["servo".to_owned(), "css-probe".to_owned()],
        controls: Vec::new(),
        presets: Vec::new(),
        audio_reactive: false,
        screen_reactive: false,
        source: EffectSource::Html { path },
        license: None,
    }
}

fn frame_input(frame_number: u64, canvas_width: u32, canvas_height: u32) -> FrameInput<'static> {
    static AUDIO: std::sync::LazyLock<AudioData> = std::sync::LazyLock::new(AudioData::silence);
    static INTERACTION: std::sync::LazyLock<InteractionData> =
        std::sync::LazyLock::new(InteractionData::default);
    static SENSORS: std::sync::LazyLock<SystemSnapshot> =
        std::sync::LazyLock::new(SystemSnapshot::empty);

    #[allow(clippy::cast_precision_loss)]
    FrameInput {
        time_secs: frame_number as f64 * f64::from(FRAME_DT_SECONDS),
        delta_secs: FRAME_DT_SECONDS,
        frame_number,
        audio: &AUDIO,
        interaction: &INTERACTION,
        screen: None,
        sensors: &SENSORS,
        sources: hypercolor_core::effect::FrameDataSources::default(),
        canvas_width,
        canvas_height,
    }
}
