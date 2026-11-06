//! Real-Servo rendering smoke tests.
//!
//! Every case drives a real [`ServoRenderer`] through the shared Servo worker
//! and asserts actual pixels, so a Servo bump or an embedder change that
//! blanks every frame fails here instead of shipping.
//!
//! The renderer is asynchronous: `init` only queues a page load, and each
//! render call collects whatever frame the worker finished since the last
//! call. Cases therefore tick in real time until the worker has presented
//! frames for the page and the frame they expect arrives. Each page gets
//! one hard deadline, generous enough for software GL on a shared CI
//! runner, that covers its load, retries, and pixel checks.
//!
//! Only frames the worker presented count. A renderer that loses its
//! session or fails its load stops warming and returns placeholders, so
//! every check reads [`ServoRenderer::presented_frames`] instead of trusting
//! the canvas, and a poisoned runtime fails the case and the run.
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

use std::collections::HashSet;
use std::env;
use std::fmt::Write as _;
use std::fs;
use std::panic::{self, UnwindSafe};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

#[path = "support/frame_state.rs"]
mod frame_state;

use hypercolor_core::effect::console::EffectConsoleJournal;
use hypercolor_core::effect::{
    EffectRenderer, ServoRenderer, ServoTelemetrySnapshot, load_html_effect_file,
    parse_html_effect_metadata, servo_runtime_poison_reason, servo_telemetry_snapshot,
    shutdown_servo_runtime,
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
/// Hard deadline for one page: its load, degraded-render retries, and pixel
/// checks together. Local runs need well under a second; the margin is for
/// software GL with a cold shader cache on a loaded CI runner.
const RENDER_DEADLINE: Duration = Duration::from_secs(150);
/// How long a deliberately broken page gets to fail once it is presenting.
const PROBE_WINDOW: Duration = Duration::from_secs(2);
/// Per-channel slack for solid-color pages.
const COLOR_TOLERANCE: u8 = 12;
/// Share of pixels a solid-color page must cover.
const SOLID_COVERAGE: f32 = 0.98;
/// Distinct values each animation clock must show after the page is ready.
const ANIMATION_DISTINCT_VALUES: usize = 8;
/// Distinct presented frames a catalog effect must show to count as animating.
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
/// Fixed green and blue channels that mark each animation clock's half.
const PAGE_CLOCK_MARK: (u8, u8) = (40, 200);
const HOST_CLOCK_MARK: (u8, u8) = (200, 40);

/// Distinct presented Shockwave frames each audio run measures.
const SHOCKWAVE_FRAMES: usize = 60;
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
        name: "servo_rejects_a_page_that_never_draws",
        run: rejects_a_page_that_never_draws,
        ignored: None,
    },
    Case {
        name: "servo_rejects_a_page_that_never_presents",
        run: rejects_a_page_that_never_presents,
        ignored: None,
    },
    Case {
        name: "servo_rejects_clocks_split_across_frames",
        run: rejects_clocks_split_across_frames,
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

    /// An empty selection is a mistake unless a substring filter aimed at
    /// another test binary: `cargo test <filter>` hands the same filter to
    /// every binary, and libtest reports those runs green.
    fn empty_selection_is_error(&self) -> bool {
        self.filters.is_empty() || self.exact
    }
}

/// Set while a case runs a probe that must panic, so the expected failure
/// stays out of the output.
static EXPECTING_PANIC: AtomicBool = AtomicBool::new(false);

fn install_panic_hook() {
    let default_hook = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        if !EXPECTING_PANIC.load(Ordering::SeqCst) {
            default_hook(info);
        }
    }));
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
    if selected.is_empty() && args.empty_selection_is_error() {
        println!("\nerror: no Servo smoke case matched the given arguments\n");
        return ExitCode::from(101);
    }

    install_panic_hook();
    println!("\nrunning {} tests", selected.len());
    let suite_started = Instant::now();
    let mut passed = 0_usize;
    let mut skipped = 0_usize;
    let mut failed = Vec::new();
    let mut poisoned = None;
    for case in &selected {
        if !args.runs(case) {
            skipped += 1;
            let reason = case.ignored.unwrap_or_default();
            println!("test {} ... ignored, {reason}", case.name);
            continue;
        }
        let started = Instant::now();
        let outcome = panic::catch_unwind(case.run);
        // A poisoned runtime fails the case that left it behind, whatever
        // the case itself concluded, and every later case would fail anyway.
        if let Some(reason) = servo_runtime_poison_reason() {
            println!(
                "test {} ... FAILED ({:.2?}), Servo runtime poisoned: {reason}",
                case.name,
                started.elapsed()
            );
            failed.push(case.name);
            poisoned = Some(reason);
            break;
        }
        match outcome {
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

    if let Some(reason) = &poisoned {
        println!("\nremaining cases skipped: the Servo runtime is poisoned ({reason})");
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
    let ok = failed.is_empty() && poisoned.is_none() && shutdown.is_ok();
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

/// Run `probe`, which must panic, and return its message.
fn expect_failure(what: &str, probe: impl FnOnce() + UnwindSafe) -> String {
    EXPECTING_PANIC.store(true, Ordering::SeqCst);
    let result = panic::catch_unwind(probe);
    EXPECTING_PANIC.store(false, Ordering::SeqCst);
    let Err(payload) = result else {
        panic!("{what}: the harness accepted a deliberately broken page");
    };
    payload
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| {
            payload
                .downcast_ref::<&str>()
                .map(|text| (*text).to_owned())
        })
        .unwrap_or_default()
}

// ── Rendering driver ───────────────────────────────────────────

/// One render call's result.
struct Tick {
    frame: Canvas,
    /// Whether the worker presented a new frame for this page since the
    /// previous tick.
    fresh: bool,
}

/// One effect loaded into a real Servo renderer and ticked in real time
/// against a single hard deadline.
struct ServoRun {
    name: String,
    renderer: ServoRenderer,
    frame_state: TestFrameState,
    deadline: Instant,
    presented_seen: u64,
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
        // The same choice the effect factory makes for HTML effects; the
        // concrete type exposes the presented-frame count.
        let mut renderer = if metadata.category == EffectCategory::Display {
            ServoRenderer::new_display_face()
        } else {
            ServoRenderer::new()
        };
        frame_state
            .initialize(&mut renderer, metadata)
            .expect("Servo renderer should queue its page load");
        Self {
            name: metadata.name.clone(),
            renderer,
            frame_state,
            deadline: Instant::now() + RENDER_DEADLINE,
            presented_seen: 0,
            console_cursor,
            telemetry_before,
            degraded_renders: 0,
            last_degraded: None,
        }
    }

    /// Pull the deadline in, so a deliberately broken page fails quickly.
    fn shorten_deadline(&mut self, window: Duration) {
        self.deadline = self.deadline.min(Instant::now() + window);
    }

    /// Fail once the page's deadline has passed.
    fn check_deadline(&self, failure: &str) {
        assert!(
            Instant::now() < self.deadline,
            "{}: {failure} before the page deadline{}",
            self.name,
            self.diagnostics()
        );
    }

    /// Fail on a poisoned runtime or any failed session or page load since
    /// this page started.
    fn assert_healthy(&self) {
        if let Some(reason) = servo_runtime_poison_reason() {
            panic!(
                "{}: the Servo runtime is poisoned: {reason}{}",
                self.name,
                self.diagnostics()
            );
        }
        let now = servo_telemetry_snapshot();
        let before = &self.telemetry_before;
        let failures = (now.session_create_failures_total - before.session_create_failures_total)
            + (now.page_load_failures_total - before.page_load_failures_total)
            + (now.renderer_load_failures_total - before.renderer_load_failures_total);
        assert_eq!(
            failures,
            0,
            "{}: a Servo session or page load failed{}",
            self.name,
            self.diagnostics()
        );
    }

    /// Render one frame under silence, then wait out the frame interval.
    fn tick(&mut self) -> Tick {
        self.tick_with_audio(&AudioData::silence())
    }

    /// A degraded render (a hard stall or a saturated queue) is retried the
    /// way the render loop retains the last frame and keeps ticking: a slow
    /// software-GL frame fails the page only when it misses the deadline.
    fn tick_with_audio(&mut self, audio: &AudioData) -> Tick {
        loop {
            self.assert_healthy();
            match self
                .frame_state
                .render(&mut self.renderer, FRAME_DT_SECONDS, audio)
            {
                Ok(frame) => {
                    assert_eq!(
                        (frame.width(), frame.height()),
                        (CANVAS_WIDTH, CANVAS_HEIGHT),
                        "{}: frame size drifted from the canvas size",
                        self.name
                    );
                    let presented = self.renderer.presented_frames();
                    let fresh = presented > self.presented_seen;
                    self.presented_seen = presented;
                    thread::sleep(FRAME_INTERVAL);
                    return Tick { frame, fresh };
                }
                Err(error) => {
                    self.degraded_renders += 1;
                    self.last_degraded = Some(format!("{error:#}"));
                    self.check_deadline(&format!("rendering stayed degraded ({error:#})"));
                    thread::sleep(FRAME_INTERVAL);
                }
            }
        }
    }

    /// Tick until the worker has presented a frame for this page.
    fn wait_until_ready(&mut self) {
        while self.renderer.presented_frames() == 0 {
            self.tick();
            // A render only succeeds without a stall, so a renderer that
            // stopped warming here failed its load or lost its session and
            // is drawing placeholders.
            assert!(
                self.renderer.presented_frames() > 0 || self.renderer.is_warming(),
                "{}: Servo stopped warming without presenting a frame{}",
                self.name,
                self.diagnostics()
            );
            self.check_deadline("Servo presented no frame");
        }
    }

    /// Tick until a presented frame satisfies `accept`, and return it.
    fn render_until(&mut self, expectation: &str, mut accept: impl FnMut(&Tick) -> bool) -> Canvas {
        self.wait_until_ready();
        loop {
            let tick = self.tick();
            if accept(&tick) {
                return tick.frame;
            }
            self.check_deadline(&format!(
                "no frame showed {expectation}; last frame: {}",
                describe_frame(&tick.frame)
            ));
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
            "\n  servo: presented frames {}, warming {}, page loads {} (failed {}), renderer load failures {}, session create failures {}, cpu frames {}, cached frames {}, soft stalls {}",
            self.renderer.presented_frames(),
            self.renderer.is_warming(),
            now.page_loads_total - before.page_loads_total,
            now.page_load_failures_total - before.page_load_failures_total,
            now.renderer_load_failures_total - before.renderer_load_failures_total,
            now.session_create_failures_total - before.session_create_failures_total,
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

    /// Destroy the page and hand back the frame clock for the next one.
    fn finish(mut self) -> TestFrameState {
        std::mem::replace(
            &mut self.frame_state,
            TestFrameState::new(CANVAS_WIDTH, CANVAS_HEIGHT),
        )
    }
}

impl Drop for ServoRun {
    /// Destroys the page even when a check panicked, so a failed case leaves
    /// no session behind on the shared worker.
    fn drop(&mut self) {
        if let Some(reason) = &self.last_degraded
            && !thread::panicking()
        {
            println!(
                "    {}: recovered after {} degraded renders; last: {reason}",
                self.name, self.degraded_renders
            );
        }
        self.renderer.destroy();
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
    run.render_until("a solid red canvas", |tick| is_solid(&tick.frame, RED));
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
    run.render_until("a solid azure WebGL2 clear", |tick| {
        is_solid(&tick.frame, AZURE)
    });
    run.finish();
    Outcome::Passed
}

/// Read both animation clocks from one frame: the page clock from the left
/// half and the host clock from the right. `None` unless both halves carry
/// their marks, so frames that each show only one clock prove nothing.
fn read_animation_clocks(frame: &Canvas) -> Option<(u8, u8)> {
    let y = CANVAS_HEIGHT / 2;
    let left = frame.get_pixel(CANVAS_WIDTH / 4, y);
    let right = frame.get_pixel(CANVAS_WIDTH * 3 / 4, y);
    let left_marked =
        channel_close(left.g, PAGE_CLOCK_MARK.0) && channel_close(left.b, PAGE_CLOCK_MARK.1);
    let right_marked =
        channel_close(right.g, HOST_CLOCK_MARK.0) && channel_close(right.b, HOST_CLOCK_MARK.1);
    (left_marked && right_marked).then_some((left.r, right.r))
}

/// Tick until both clocks have shown enough distinct values in presented
/// frames that carry both of them.
fn watch_animation_clocks(run: &mut ServoRun) {
    run.wait_until_ready();
    let mut page_clock = HashSet::new();
    let mut host_clock = HashSet::new();
    while page_clock.len() < ANIMATION_DISTINCT_VALUES
        || host_clock.len() < ANIMATION_DISTINCT_VALUES
    {
        let tick = run.tick();
        if tick.fresh
            && let Some((page, host)) = read_animation_clocks(&tick.frame)
        {
            page_clock.insert(page);
            host_clock.insert(host);
        }
        run.check_deadline(&format!(
            "animation latched; the page clock showed {} distinct values and the host clock {}; last frame: {}",
            page_clock.len(),
            host_clock.len(),
            describe_frame(&tick.frame)
        ));
    }
}

/// The left half counts the page's own animation frames; the right half
/// shows the frame number the host injects into `window.engine` each tick.
/// Both clocks must keep advancing, so neither Servo's animation pipeline
/// nor the host frame payload can latch one frame and pass.
fn keeps_presenting_page_and_host_animation() -> Outcome {
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
    watch_animation_clocks(&mut run);
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
    first.render_until("the first effect's red", |tick| is_solid(&tick.frame, RED));
    let frame_state = first.finish();

    let mut second = ServoRun::start_with_state(&green, frame_state);
    second.render_until("the second effect's green", |tick| {
        is_solid(&tick.frame, GREEN)
    });
    for _ in 0..SETTLE_TICKS {
        let tick = second.tick();
        assert!(
            is_solid(&tick.frame, GREEN),
            "{}: output left green after the switch; frame: {}{}",
            second.name,
            describe_frame(&tick.frame),
            second.diagnostics()
        );
    }
    second.finish();
    Outcome::Passed
}

// ── Broken pages the harness must reject ───────────────────────

/// A page that throws before drawing still presents frames (its black
/// background), and the pixel check must refuse them.
fn rejects_a_page_that_never_draws() -> Outcome {
    let dir = tempdir().expect("tempdir should create");
    let metadata = write_page(
        &dir,
        "never-draws",
        "throw new Error('deliberately broken before drawing');",
    );
    let message = expect_failure("a page that never draws", || {
        let mut run = ServoRun::start(&metadata);
        run.wait_until_ready();
        run.shorten_deadline(PROBE_WINDOW);
        run.render_until("a solid red canvas", |tick| is_solid(&tick.frame, RED));
    });
    assert!(
        message.contains("no frame showed a solid red canvas"),
        "a page that never draws failed for the wrong reason: {message}"
    );
    Outcome::Passed
}

/// A page whose load fails stops warming and renders placeholders, which
/// change every frame and clear the lit threshold. Readiness must fail on
/// the missing presentation instead of waiting for the deadline.
fn rejects_a_page_that_never_presents() -> Outcome {
    let dir = tempdir().expect("tempdir should create");
    let metadata = page_metadata(dir.path().join("missing-page.html"));
    let started = Instant::now();
    let message = expect_failure("a page that never presents", || {
        let mut run = ServoRun::start(&metadata);
        run.wait_until_ready();
    });
    assert!(
        message.contains("stopped warming without presenting a frame"),
        "a page that never presents failed for the wrong reason: {message}"
    );
    assert!(
        started.elapsed() < RENDER_DEADLINE / 2,
        "a failed load took {:.2?} to surface",
        started.elapsed()
    );
    Outcome::Passed
}

/// Full-canvas fills that alternate between the two clock colors put each
/// mark in its own frame. Neither clock is proven, so the check must fail.
fn rejects_clocks_split_across_frames() -> Outcome {
    let dir = tempdir().expect("tempdir should create");
    let metadata = write_page(
        &dir,
        "split-clocks",
        "const ctx = canvas.getContext('2d');
let frames = 0;
function draw() {
  frames += 1;
  ctx.fillStyle = frames % 2
    ? `rgb(${(frames * 9) % 256},40,200)`
    : `rgb(${(frames * 9) % 256},200,40)`;
  ctx.fillRect(0, 0, canvas.width, canvas.height);
  requestAnimationFrame(draw);
}
requestAnimationFrame(draw);",
    );
    let message = expect_failure("clocks split across frames", || {
        let mut run = ServoRun::start(&metadata);
        run.wait_until_ready();
        run.shorten_deadline(PROBE_WINDOW);
        watch_animation_clocks(&mut run);
    });
    assert!(
        message.contains("animation latched"),
        "clocks split across frames failed for the wrong reason: {message}"
    );
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

/// Load one generated effect and prove it renders as expected. Readiness
/// alone already requires a presented frame.
fn prove_catalog_effect(root: &Path, file_name: &str, expectation: CatalogExpectation) {
    let metadata = catalog_metadata(root, file_name);
    let mut run = ServoRun::start(&metadata);
    run.wait_until_ready();
    if expectation.drawn {
        run.render_until("drawn pixels", |tick| is_drawn(&tick.frame));
    }
    if expectation.animated {
        let mut frames = HashSet::new();
        run.render_until("animation across presented frames", |tick| {
            if tick.fresh && (!expectation.drawn || is_drawn(&tick.frame)) {
                frames.insert(tick.frame.as_rgba_bytes().to_vec());
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

/// What one Shockwave run measured.
struct AudioRun {
    frames: Vec<Canvas>,
    ticks: usize,
}

/// Feed `audio_for(tick)` at the host's 60 fps cadence and keep only the
/// distinct frames the worker presented, so render latency cannot pad the
/// sequence with repeats of one frame.
fn render_audio_sequence(
    metadata: &EffectMetadata,
    audio_for: impl Fn(usize) -> AudioData,
) -> AudioRun {
    let mut run = ServoRun::start(metadata);
    run.wait_until_ready();
    let mut frames: Vec<Canvas> = Vec::with_capacity(SHOCKWAVE_FRAMES);
    let mut ticks = 0;
    while frames.len() < SHOCKWAVE_FRAMES {
        let tick = run.tick_with_audio(&audio_for(ticks));
        ticks += 1;
        let repeats_last = frames
            .last()
            .is_some_and(|last| last.as_rgba_bytes() == tick.frame.as_rgba_bytes());
        if tick.fresh && !repeats_last {
            frames.push(tick.frame);
        }
        run.check_deadline(&format!(
            "presented only {} distinct audio frames in {ticks} ticks",
            frames.len()
        ));
    }
    run.assert_no_console_errors();
    run.finish();
    AudioRun { frames, ticks }
}

/// Live audio must reach the page: Shockwave surges harder on a music-like
/// signal than on its silent fallback pulse.
fn shockwave_prefers_live_music_over_fallback_pulse() -> Outcome {
    let root = match generated_catalog() {
        Ok(root) => root,
        Err(skipped) => return skipped,
    };
    let metadata = catalog_metadata(&root, "shockwave.html");

    let active_run = render_audio_sequence(&metadata, music_like_audio_frame);
    let silence_run = render_audio_sequence(&metadata, |_| AudioData::silence());
    let active = sequence_activity_metrics(&active_run.frames);
    let silence = sequence_activity_metrics(&silence_run.frames);
    println!(
        "    shockwave active over {} frames in {} ticks: {active:?}\n    shockwave silence over {} frames in {} ticks: {silence:?}",
        active_run.frames.len(),
        active_run.ticks,
        silence_run.frames.len(),
        silence_run.ticks
    );

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
