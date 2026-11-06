//! Real-Servo rendering smoke tests.
//!
//! Every case drives a real [`ServoRenderer`] through the shared Servo worker
//! and asserts actual pixels, so a Servo bump or an embedder change that
//! blanks every frame fails here instead of shipping.
//!
//! The renderer is asynchronous: `init` only queues a page load, and each
//! render call collects whatever frame the worker finished since the last
//! call. Cases therefore tick in real time until the renderer stops warming
//! and the frame they expect arrives, under a deadline generous enough for
//! software GL on a shared CI runner.
//!
//! This binary runs its own harness (`harness = false`). Servo sets up
//! process-global state once and cannot start again in the same process, and
//! a process that exits while the runtime is alive aborts in SpiderMonkey's
//! static destructors. The harness runs every case serially on one runtime,
//! then shuts it down before exiting, the way the daemon does.
//!
//! Self-contained cases write their pages to temp dirs and run anywhere the
//! `servo` feature builds. Catalog cases render the SDK effects that `just
//! effects-build` writes to `effects/hypercolor`, or whatever
//! `$HYPERCOLOR_EFFECTS_DIR` names. They are skipped when that catalog is
//! missing unless `HYPERCOLOR_REQUIRE_GENERATED_EFFECTS=1`, which CI sets.
//!
//! The ignored sweep renders every generated effect; run it after a Servo
//! bump with `-- --include-ignored`.
//!
//! [`ServoRenderer`]: hypercolor_core::effect::ServoRenderer

use std::collections::HashSet;
use std::env;
use std::fmt::Write as _;
use std::fs;
use std::panic;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::thread;
use std::time::{Duration, Instant};

#[path = "support/frame_state.rs"]
mod frame_state;

use hypercolor_core::effect::console::EffectConsoleJournal;
use hypercolor_core::effect::{
    EffectRenderer, ServoTelemetrySnapshot, create_renderer_for_metadata, load_html_effect_file,
    parse_html_effect_metadata, servo_telemetry_snapshot, shutdown_servo_runtime,
};
use hypercolor_types::audio::AudioData;
use hypercolor_types::canvas::Canvas;
use hypercolor_types::effect::{EffectCategory, EffectId, EffectMetadata, EffectSource};
use tempfile::{TempDir, tempdir};
use uuid::Uuid;

use frame_state::TestFrameState;

const CANVAS_WIDTH: u32 = 320;
const CANVAS_HEIGHT: u32 = 200;
const FRAME_DT_SECONDS: f32 = 1.0 / 60.0;
/// Real time between ticks, matching the frame clock's 60 fps step.
const FRAME_INTERVAL: Duration = Duration::from_millis(16);
/// Longest a case waits for a page to load or to present the frame it
/// expects. Local runs need well under a second; the margin is for software
/// GL on a loaded CI runner.
const RENDER_DEADLINE: Duration = Duration::from_secs(30);
/// Per-channel slack for solid-color pages.
const COLOR_TOLERANCE: u8 = 12;
/// Share of pixels a solid-color page must cover.
const SOLID_COVERAGE: f32 = 0.98;
/// Distinct values each animation clock must show after the page is ready.
const ANIMATION_DISTINCT_VALUES: usize = 8;
/// Distinct drawn frames a catalog effect must present to count as animating.
const CATALOG_DISTINCT_FRAMES: usize = 3;
/// A channel at or above this level counts the pixel as lit.
const LIT_CHANNEL_THRESHOLD: u8 = 24;
/// Share of lit pixels that separates a drawn frame from a blank one.
const LIT_COVERAGE: f32 = 0.01;
/// Ticks sampled after a switch to prove the old page never comes back.
const SETTLE_TICKS: usize = 12;

const REQUIRE_CATALOG_ENV: &str = "HYPERCOLOR_REQUIRE_GENERATED_EFFECTS";
const EFFECTS_DIR_ENV: &str = "HYPERCOLOR_EFFECTS_DIR";

/// Generated effects every run renders: two WebGL shaders, two Canvas 2D
/// effects, and an audio-reactive effect that still draws under silence.
const CATALOG_SAMPLE: &[&str] = &[
    "arc-storm.html",
    "plasma-engine.html",
    "rainbow.html",
    "color-wave.html",
    "audio-pulse.html",
];
/// Generated effects that hold one frame by design.
const STATIC_CATALOG_EFFECTS: &[&str] = &["color-zones.html", "solid-color.html"];

const RED: [u8; 3] = [255, 0, 0];
const GREEN: [u8; 3] = [0, 255, 0];
/// Asymmetric so a red/blue channel swap in readback cannot pass.
const AZURE: [u8; 3] = [0, 128, 255];

const AUDIO_TEST_FRAMES: usize = 120;
const BASS_END: usize = 40;
const MID_END: usize = 130;
const SHOCKWAVE_PIXEL_THRESHOLD: u8 = 80;
const SHOCKWAVE_DELTA_THRESHOLD: u8 = 56;
const SHOCKWAVE_SURGE_PIXEL_COUNT: usize = 180;

// ── Harness ────────────────────────────────────────────────────

enum Outcome {
    Passed,
    Skipped(String),
}

struct Case {
    name: &'static str,
    run: fn() -> Outcome,
    ignored: Option<&'static str>,
}

const CASES: &[Case] = &[
    Case {
        name: "servo_renders_canvas2d_page_pixels",
        run: renders_canvas2d_page_pixels,
        ignored: None,
    },
    Case {
        name: "servo_renders_webgl2_clear_color",
        run: renders_webgl2_clear_color,
        ignored: None,
    },
    Case {
        name: "servo_keeps_presenting_page_and_host_animation",
        run: keeps_presenting_page_and_host_animation,
        ignored: None,
    },
    Case {
        name: "servo_switches_effects_on_the_shared_worker",
        run: switches_effects_on_the_shared_worker,
        ignored: None,
    },
    Case {
        name: "servo_renders_generated_catalog_sample",
        run: renders_generated_catalog_sample,
        ignored: None,
    },
    Case {
        name: "servo_shockwave_prefers_live_music_over_fallback_pulse",
        run: shockwave_prefers_live_music_over_fallback_pulse,
        ignored: None,
    },
    Case {
        name: "servo_renders_every_generated_effect",
        run: renders_every_generated_effect,
        ignored: Some("renders the whole generated catalog; run after a Servo bump"),
    },
];

/// The libtest arguments this harness honors: positional name filters,
/// `--exact`, `--skip`, `--list`, `--ignored`, and `--include-ignored`.
/// Other flags (`--nocapture`, `--test-threads`, `--color`, ...) are accepted
/// and ignored, since cases always run serially on one runtime.
struct HarnessArgs {
    filters: Vec<String>,
    skips: Vec<String>,
    exact: bool,
    list: bool,
    ignored_only: bool,
    include_ignored: bool,
}

impl HarnessArgs {
    fn parse() -> Self {
        const VALUE_FLAGS: &[&str] = &[
            "--test-threads",
            "--color",
            "--format",
            "--logfile",
            "--report-time",
            "-Z",
        ];
        let mut parsed = Self {
            filters: Vec::new(),
            skips: Vec::new(),
            exact: false,
            list: false,
            ignored_only: false,
            include_ignored: false,
        };
        let mut args = env::args().skip(1);
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--exact" => parsed.exact = true,
                "--list" => parsed.list = true,
                "--ignored" => parsed.ignored_only = true,
                "--include-ignored" => parsed.include_ignored = true,
                "--skip" => parsed.skips.extend(args.next()),
                flag if VALUE_FLAGS.contains(&flag) => {
                    args.next();
                }
                flag if flag.starts_with('-') => {}
                filter => parsed.filters.push(filter.to_owned()),
            }
        }
        parsed
    }

    fn selects(&self, case: &Case) -> bool {
        if self.ignored_only && case.ignored.is_none() {
            return false;
        }
        if self
            .skips
            .iter()
            .any(|skip| case.name.contains(skip.as_str()))
        {
            return false;
        }
        self.filters.is_empty()
            || self.filters.iter().any(|filter| {
                if self.exact {
                    case.name == filter
                } else {
                    case.name.contains(filter.as_str())
                }
            })
    }

    fn runs(&self, case: &Case) -> bool {
        case.ignored.is_none() || self.ignored_only || self.include_ignored
    }
}

fn main() -> ExitCode {
    let args = HarnessArgs::parse();
    let selected: Vec<&Case> = CASES.iter().filter(|case| args.selects(case)).collect();

    if args.list {
        for case in &selected {
            println!("{}: test", case.name);
        }
        return ExitCode::SUCCESS;
    }

    println!("\nrunning {} tests", selected.len());
    let suite_started = Instant::now();
    let mut passed = 0_usize;
    let mut skipped = 0_usize;
    let mut failed = Vec::new();
    for case in &selected {
        if !args.runs(case) {
            skipped += 1;
            let reason = case.ignored.unwrap_or_default();
            println!("test {} ... ignored, {reason}", case.name);
            continue;
        }
        let started = Instant::now();
        match panic::catch_unwind(case.run) {
            Ok(Outcome::Passed) => {
                passed += 1;
                println!("test {} ... ok ({:.2?})", case.name, started.elapsed());
            }
            Ok(Outcome::Skipped(reason)) => {
                skipped += 1;
                println!("test {} ... ignored, {reason}", case.name);
            }
            Err(_) => {
                failed.push(case.name);
                println!("test {} ... FAILED ({:.2?})", case.name, started.elapsed());
            }
        }
    }

    let shutdown = shutdown_servo_runtime();
    if let Err(error) = &shutdown {
        println!("\nServo runtime shutdown failed: {error:#}");
    }
    if !failed.is_empty() {
        println!("\nfailures:");
        for name in &failed {
            println!("    {name}");
        }
    }
    let ok = failed.is_empty() && shutdown.is_ok();
    println!(
        "\ntest result: {}. {passed} passed; {} failed; {skipped} ignored; finished in {:.2?}\n",
        if ok { "ok" } else { "FAILED" },
        failed.len(),
        suite_started.elapsed()
    );

    if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(101)
    }
}

// ── Rendering driver ───────────────────────────────────────────

/// One effect loaded into a real Servo renderer and ticked in real time.
struct ServoRun {
    name: String,
    renderer: Box<dyn EffectRenderer>,
    frame_state: TestFrameState,
    console_cursor: u64,
    telemetry_before: ServoTelemetrySnapshot,
    degraded_renders: usize,
    last_degraded: Option<String>,
}

impl ServoRun {
    fn start(metadata: &EffectMetadata) -> Self {
        Self::start_with_state(metadata, TestFrameState::new(CANVAS_WIDTH, CANVAS_HEIGHT))
    }

    fn start_with_state(metadata: &EffectMetadata, frame_state: TestFrameState) -> Self {
        let console_cursor = EffectConsoleJournal::global().latest_seq();
        let telemetry_before = servo_telemetry_snapshot();
        let mut renderer = create_renderer_for_metadata(metadata)
            .expect("HTML metadata should select the Servo renderer");
        frame_state
            .initialize(renderer.as_mut(), metadata)
            .expect("Servo renderer should queue its page load");
        Self {
            name: metadata.name.clone(),
            renderer,
            frame_state,
            console_cursor,
            telemetry_before,
            degraded_renders: 0,
            last_degraded: None,
        }
    }

    /// Render one frame under silence, then wait out the frame interval.
    fn tick(&mut self) -> Canvas {
        self.tick_with_audio(&AudioData::silence())
    }

    /// A degraded render (a hard stall or a saturated queue) is retried the
    /// way the render loop retains the last frame and keeps ticking: a slow
    /// software-GL frame fails the case only when it never lands.
    fn tick_with_audio(&mut self, audio: &AudioData) -> Canvas {
        let mut degraded_since: Option<Instant> = None;
        loop {
            match self
                .frame_state
                .render(self.renderer.as_mut(), FRAME_DT_SECONDS, audio)
            {
                Ok(frame) => {
                    assert_eq!(
                        (frame.width(), frame.height()),
                        (CANVAS_WIDTH, CANVAS_HEIGHT),
                        "{}: frame size drifted from the canvas size",
                        self.name
                    );
                    thread::sleep(FRAME_INTERVAL);
                    return frame;
                }
                Err(error) => {
                    let since = *degraded_since.get_or_insert_with(Instant::now);
                    self.degraded_renders += 1;
                    self.last_degraded = Some(format!("{error:#}"));
                    assert!(
                        since.elapsed() < RENDER_DEADLINE,
                        "{}: Servo rendering stayed degraded for {RENDER_DEADLINE:?}: {error:#}{}",
                        self.name,
                        self.diagnostics()
                    );
                    thread::sleep(FRAME_INTERVAL);
                }
            }
        }
    }

    /// Tick until the page has loaded and presented its first frame.
    fn wait_until_ready(&mut self) {
        let started = Instant::now();
        while self.renderer.is_warming() {
            assert!(
                started.elapsed() < RENDER_DEADLINE,
                "{}: Servo presented no first frame within {RENDER_DEADLINE:?}{}",
                self.name,
                self.diagnostics()
            );
            self.tick();
        }
        // A failed load also stops warming, then renders placeholders.
        assert_eq!(
            servo_telemetry_snapshot().renderer_load_failures_total,
            self.telemetry_before.renderer_load_failures_total,
            "{}: the Servo page load failed{}",
            self.name,
            self.diagnostics()
        );
    }

    /// Tick until a frame satisfies `accept`, and return that frame.
    fn render_until(
        &mut self,
        expectation: &str,
        mut accept: impl FnMut(&Canvas) -> bool,
    ) -> Canvas {
        self.wait_until_ready();
        let started = Instant::now();
        loop {
            let frame = self.tick();
            if accept(&frame) {
                return frame;
            }
            assert!(
                started.elapsed() < RENDER_DEADLINE,
                "{}: no frame showed {expectation} within {RENDER_DEADLINE:?}; last frame: {}{}",
                self.name,
                describe_frame(&frame),
                self.diagnostics()
            );
        }
    }

    /// Fail if the page logged a console error since it started.
    fn assert_no_console_errors(&self) {
        let errors = EffectConsoleJournal::global().read(self.console_cursor, 16, |entry| {
            entry.effect_name == self.name && entry.level == "error"
        });
        assert!(
            errors.entries.is_empty(),
            "{}: page logged console errors{}",
            self.name,
            self.diagnostics()
        );
    }

    /// Recent console output and Servo counters, for failure messages.
    fn diagnostics(&self) -> String {
        let now = servo_telemetry_snapshot();
        let before = &self.telemetry_before;
        let mut text = format!(
            "\n  servo: page loads {} (failed {}), renderer load failures {}, cpu frames {}, cached frames {}, soft stalls {}",
            now.page_loads_total - before.page_loads_total,
            now.page_load_failures_total - before.page_load_failures_total,
            now.renderer_load_failures_total - before.renderer_load_failures_total,
            now.render_cpu_frames_total - before.render_cpu_frames_total,
            now.render_cached_frames_total - before.render_cached_frames_total,
            now.soft_stalls_total - before.soft_stalls_total,
        );
        if let Some(reason) = &self.last_degraded {
            let _ = write!(
                text,
                "\n  degraded renders {}, last: {reason}",
                self.degraded_renders
            );
        }
        let console = EffectConsoleJournal::global().read(self.console_cursor, 16, |entry| {
            entry.effect_name == self.name
        });
        for entry in &console.entries {
            let _ = write!(text, "\n  console {}: {}", entry.level, entry.message);
        }
        text
    }

    fn finish(mut self) -> TestFrameState {
        if let Some(reason) = &self.last_degraded {
            println!(
                "    {}: recovered after {} degraded renders; last: {reason}",
                self.name, self.degraded_renders
            );
        }
        self.renderer.destroy();
        self.frame_state
    }
}

// ── Frame predicates ───────────────────────────────────────────

fn channel_close(actual: u8, expected: u8) -> bool {
    actual.abs_diff(expected) <= COLOR_TOLERANCE
}

fn pixel_matches(pixel: [u8; 4], expected: [u8; 3]) -> bool {
    channel_close(pixel[0], expected[0])
        && channel_close(pixel[1], expected[1])
        && channel_close(pixel[2], expected[2])
}

fn coverage(canvas: &Canvas, accept: impl Fn([u8; 4]) -> bool) -> f32 {
    let total = canvas.pixels().len().max(1);
    let matching = canvas.pixels().filter(|pixel| accept(*pixel)).count();
    matching as f32 / total as f32
}

fn is_solid(canvas: &Canvas, expected: [u8; 3]) -> bool {
    coverage(canvas, |pixel| pixel_matches(pixel, expected)) >= SOLID_COVERAGE
}

fn is_lit(pixel: [u8; 4]) -> bool {
    pixel[0].max(pixel[1]).max(pixel[2]) >= LIT_CHANNEL_THRESHOLD
}

fn is_drawn(canvas: &Canvas) -> bool {
    coverage(canvas, is_lit) >= LIT_COVERAGE
}

fn describe_frame(canvas: &Canvas) -> String {
    let mut sum = [0_u64; 3];
    for pixel in canvas.pixels() {
        for (total, channel) in sum.iter_mut().zip(pixel) {
            *total += u64::from(channel);
        }
    }
    let count = canvas.pixels().len().max(1) as u64;
    let center = canvas.get_pixel(canvas.width() / 2, canvas.height() / 2);
    format!(
        "mean rgb ({}, {}, {}), center rgba ({}, {}, {}, {}), lit {:.1}%",
        sum[0] / count,
        sum[1] / count,
        sum[2] / count,
        center.r,
        center.g,
        center.b,
        center.a,
        coverage(canvas, is_lit) * 100.0
    )
}

// ── Pages ──────────────────────────────────────────────────────

fn page_metadata(path: PathBuf) -> EffectMetadata {
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
        input_reactive: false,
        source: EffectSource::Html { path },
        license: None,
    }
}

/// Write a page whose canvas fills the viewport and runs `script`.
fn write_page(dir: &TempDir, name: &str, script: &str) -> EffectMetadata {
    let path = dir.path().join(format!("{name}.html"));
    let html = format!(
        r#"<!doctype html>
<html>
<body style="margin:0;background:black;overflow:hidden;">
<canvas id="fx" width="{CANVAS_WIDTH}" height="{CANVAS_HEIGHT}" style="display:block;"></canvas>
<script>
const canvas = document.getElementById('fx');
{script}
</script>
</body>
</html>"#
    );
    fs::write(&path, html).expect("smoke page should be written");
    page_metadata(path)
}

fn fill_page(dir: &TempDir, name: &str, [r, g, b]: [u8; 3]) -> EffectMetadata {
    write_page(
        dir,
        name,
        &format!(
            "const ctx = canvas.getContext('2d');
ctx.fillStyle = 'rgb({r},{g},{b})';
ctx.fillRect(0, 0, canvas.width, canvas.height);"
        ),
    )
}

// ── Self-contained cases ───────────────────────────────────────

fn renders_canvas2d_page_pixels() -> Outcome {
    let dir = tempdir().expect("tempdir should create");
    let mut run = ServoRun::start(&fill_page(&dir, "canvas2d-red", RED));
    run.render_until("a solid red canvas", |frame| is_solid(frame, RED));
    run.finish();
    Outcome::Passed
}

fn renders_webgl2_clear_color() -> Outcome {
    let dir = tempdir().expect("tempdir should create");
    let metadata = write_page(
        &dir,
        "webgl2-azure",
        "const gl = canvas.getContext('webgl2', { preserveDrawingBuffer: true });
if (!gl) { throw new Error('WebGL2 is unavailable'); }
gl.viewport(0, 0, canvas.width, canvas.height);
gl.clearColor(0, 128 / 255, 1, 1);
gl.clear(gl.COLOR_BUFFER_BIT);",
    );
    let mut run = ServoRun::start(&metadata);
    run.render_until("a solid azure WebGL2 clear", |frame| is_solid(frame, AZURE));
    run.finish();
    Outcome::Passed
}

/// The left half counts the page's own animation frames; the right half
/// shows the frame number the host injects into `window.engine` each tick.
/// Both clocks must keep advancing, so neither Servo's animation pipeline
/// nor the host frame payload can latch one frame and pass.
fn keeps_presenting_page_and_host_animation() -> Outcome {
    // Fixed green and blue channels mark each half as page content.
    const LEFT_MARK: (u8, u8) = (40, 200);
    const RIGHT_MARK: (u8, u8) = (200, 40);

    let dir = tempdir().expect("tempdir should create");
    let metadata = write_page(
        &dir,
        "animated-halves",
        "const ctx = canvas.getContext('2d');
let pageFrames = 0;
function draw() {
  pageFrames += 1;
  const hostFrame = (window.engine && Number(window.engine.frame)) || 0;
  ctx.fillStyle = `rgb(${(pageFrames * 9) % 256},40,200)`;
  ctx.fillRect(0, 0, canvas.width / 2, canvas.height);
  ctx.fillStyle = `rgb(${(hostFrame * 9) % 256},200,40)`;
  ctx.fillRect(canvas.width / 2, 0, canvas.width / 2, canvas.height);
  requestAnimationFrame(draw);
}
requestAnimationFrame(draw);",
    );

    let mut run = ServoRun::start(&metadata);
    run.wait_until_ready();
    let y = CANVAS_HEIGHT / 2;
    let mut page_clock = HashSet::new();
    let mut host_clock = HashSet::new();
    let started = Instant::now();
    while page_clock.len() < ANIMATION_DISTINCT_VALUES
        || host_clock.len() < ANIMATION_DISTINCT_VALUES
    {
        let frame = run.tick();
        let left = frame.get_pixel(CANVAS_WIDTH / 4, y);
        let right = frame.get_pixel(CANVAS_WIDTH * 3 / 4, y);
        if channel_close(left.g, LEFT_MARK.0) && channel_close(left.b, LEFT_MARK.1) {
            page_clock.insert(left.r);
        }
        if channel_close(right.g, RIGHT_MARK.0) && channel_close(right.b, RIGHT_MARK.1) {
            host_clock.insert(right.r);
        }
        assert!(
            started.elapsed() < RENDER_DEADLINE,
            "{}: animation latched; the page clock showed {} distinct values and the host clock {} within {RENDER_DEADLINE:?}; last frame: {}{}",
            run.name,
            page_clock.len(),
            host_clock.len(),
            describe_frame(&frame),
            run.diagnostics()
        );
    }
    run.finish();
    Outcome::Passed
}

/// Mirrors the effect pool's switch: destroy the old renderer, then create
/// one for the next page on the same shared worker and frame clock.
fn switches_effects_on_the_shared_worker() -> Outcome {
    let dir = tempdir().expect("tempdir should create");
    let red = fill_page(&dir, "switch-red", RED);
    let green = fill_page(&dir, "switch-green", GREEN);

    let mut first = ServoRun::start(&red);
    first.render_until("the first effect's red", |frame| is_solid(frame, RED));
    let frame_state = first.finish();

    let mut second = ServoRun::start_with_state(&green, frame_state);
    second.render_until("the second effect's green", |frame| is_solid(frame, GREEN));
    for _ in 0..SETTLE_TICKS {
        let frame = second.tick();
        assert!(
            is_solid(&frame, GREEN),
            "{}: output left green after the switch; frame: {}{}",
            second.name,
            describe_frame(&frame),
            second.diagnostics()
        );
    }
    second.finish();
    Outcome::Passed
}

// ── Generated catalog cases ────────────────────────────────────

fn generated_catalog() -> Result<PathBuf, Outcome> {
    let root = env::var_os(EFFECTS_DIR_ENV).map_or_else(
        || Path::new(env!("CARGO_MANIFEST_DIR")).join("../../effects/hypercolor"),
        PathBuf::from,
    );
    if !catalog_effect_files(&root).is_empty() {
        return Ok(root);
    }
    let required = env::var(REQUIRE_CATALOG_ENV).is_ok_and(|value| value == "1");
    assert!(
        !required,
        "{REQUIRE_CATALOG_ENV}=1 but no generated effects were found at {}; run `just effects-build`",
        root.display()
    );
    Err(Outcome::Skipped(format!(
        "no generated effects at {}; run `just effects-build`",
        root.display()
    )))
}

fn catalog_effect_files(root: &Path) -> Vec<String> {
    let Ok(entries) = fs::read_dir(root) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .flatten()
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| {
            Path::new(name)
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("html"))
        })
        .collect();
    names.sort();
    names
}

fn catalog_metadata(root: &Path, file_name: &str) -> EffectMetadata {
    let path = root.join(file_name);
    assert!(
        path.exists(),
        "generated effect {} is missing; run `just effects-build`",
        path.display()
    );
    load_html_effect_file(&path)
        .unwrap_or_else(|error| panic!("failed to load {}: {}", path.display(), error.message))
        .unwrap_or_else(|| panic!("{} did not load as an HTML effect", path.display()))
        .metadata
}

fn catalog_uses_webgl(root: &Path, file_name: &str) -> bool {
    let path = root.join(file_name);
    let html = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
    parse_html_effect_metadata(&html).uses_webgl
}

/// What a catalog effect must show under silence with no screen or input.
#[derive(Clone, Copy)]
struct CatalogExpectation {
    drawn: bool,
    animated: bool,
}

/// Load one generated effect and prove it renders as expected.
fn prove_catalog_effect(root: &Path, file_name: &str, expectation: CatalogExpectation) {
    let metadata = catalog_metadata(root, file_name);
    let mut run = ServoRun::start(&metadata);
    run.wait_until_ready();
    if expectation.drawn {
        run.render_until("drawn pixels", is_drawn);
    }
    if expectation.animated {
        let mut frames = HashSet::new();
        run.render_until("animation", |frame| {
            if !expectation.drawn || is_drawn(frame) {
                frames.insert(frame.as_rgba_bytes().to_vec());
            }
            frames.len() >= CATALOG_DISTINCT_FRAMES
        });
    }
    run.assert_no_console_errors();
    run.finish();
}

fn renders_generated_catalog_sample() -> Outcome {
    let root = match generated_catalog() {
        Ok(root) => root,
        Err(skipped) => return skipped,
    };
    assert!(
        CATALOG_SAMPLE
            .iter()
            .any(|file_name| catalog_uses_webgl(&root, file_name)),
        "the catalog sample no longer includes a WebGL effect"
    );
    for file_name in CATALOG_SAMPLE {
        prove_catalog_effect(
            &root,
            file_name,
            CatalogExpectation {
                drawn: true,
                animated: true,
            },
        );
    }
    Outcome::Passed
}

/// Effects that need a screen or input source may stay dark here, and the
/// static ones hold one frame; everything else must draw and animate.
fn renders_every_generated_effect() -> Outcome {
    let root = match generated_catalog() {
        Ok(root) => root,
        Err(skipped) => return skipped,
    };
    for file_name in catalog_effect_files(&root) {
        let metadata = catalog_metadata(&root, &file_name);
        let needs_input = metadata.screen_reactive || metadata.input_reactive;
        let started = Instant::now();
        prove_catalog_effect(
            &root,
            &file_name,
            CatalogExpectation {
                drawn: !needs_input,
                animated: !needs_input && !STATIC_CATALOG_EFFECTS.contains(&file_name.as_str()),
            },
        );
        println!("    {file_name} ... ok ({:.2?})", started.elapsed());
    }
    Outcome::Passed
}

// ── Audio reactivity ───────────────────────────────────────────

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

fn near_shockwave_emitter(emitters: &[(f32, f32); 3], index: usize, width: usize) -> bool {
    let exclusion_radius_sq = 20.0_f32 * 20.0_f32;
    let x = (index % width) as f32;
    let y = (index / width) as f32;
    emitters.iter().any(|(cx, cy)| {
        let dx = x - cx;
        let dy = y - cy;
        dx * dx + dy * dy <= exclusion_radius_sq
    })
}

fn shockwave_dynamic_pixels(canvas: &Canvas) -> usize {
    let emitters = shockwave_emitters(canvas);
    let width = canvas.width() as usize;
    canvas
        .pixels()
        .enumerate()
        .filter(|(index, [r, g, b, _])| {
            (*r).max(*g).max(*b) >= SHOCKWAVE_PIXEL_THRESHOLD
                && !near_shockwave_emitter(&emitters, *index, width)
        })
        .count()
}

fn shockwave_delta_pixels(previous: &Canvas, current: &Canvas) -> usize {
    let emitters = shockwave_emitters(current);
    let width = current.width() as usize;
    previous
        .pixels()
        .zip(current.pixels())
        .enumerate()
        .filter(|(index, (before, after))| {
            let max_delta = before[0]
                .abs_diff(after[0])
                .max(before[1].abs_diff(after[1]))
                .max(before[2].abs_diff(after[2]));
            max_delta >= SHOCKWAVE_DELTA_THRESHOLD
                && !near_shockwave_emitter(&emitters, *index, width)
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
        dynamic_pixel_total += shockwave_dynamic_pixels(frame);

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

fn render_audio_sequence(metadata: &EffectMetadata, sequence: &[AudioData]) -> Vec<Canvas> {
    let mut run = ServoRun::start(metadata);
    run.wait_until_ready();
    let frames = sequence
        .iter()
        .map(|audio| run.tick_with_audio(audio))
        .collect();
    run.assert_no_console_errors();
    run.finish();
    frames
}

/// Live audio must reach the page: Shockwave surges harder on a music-like
/// signal than on its silent fallback pulse.
fn shockwave_prefers_live_music_over_fallback_pulse() -> Outcome {
    let root = match generated_catalog() {
        Ok(root) => root,
        Err(skipped) => return skipped,
    };
    let metadata = catalog_metadata(&root, "shockwave.html");
    let active_audio: Vec<_> = (0..AUDIO_TEST_FRAMES).map(music_like_audio_frame).collect();
    let silence_audio = vec![AudioData::silence(); AUDIO_TEST_FRAMES];

    let active = sequence_activity_metrics(&render_audio_sequence(&metadata, &active_audio));
    let silence = sequence_activity_metrics(&render_audio_sequence(&metadata, &silence_audio));
    println!("    shockwave active={active:?}\n    shockwave silence={silence:?}");

    assert!(
        active.average_delta_pixels > silence.average_delta_pixels * 1.35,
        "expected live audio to create stronger frame-to-frame shockwave surges than fallback mode: active={active:?} silence={silence:?}"
    );
    assert!(
        active.average_delta_pixels > 6_000.0,
        "expected live audio to produce a meaningful amount of Shockwave surge motion: active={active:?} silence={silence:?}"
    );
    Outcome::Passed
}
