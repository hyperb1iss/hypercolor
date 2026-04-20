//! Shared Servo worker thread lifecycle and runtime.
//!
//! Servo initializes process-global options exactly once; recreating the
//! runtime after a shutdown panics inside servo. Hypercolor therefore
//! keeps one worker alive for the entire daemon lifetime and reuses it
//! across HTML effect switches. The [`ServoCircuitBreaker`] gates retries
//! for soft failures so a flaky effect load can't permanently knock HTML
//! effects offline, while the legacy "poison forever" path still applies
//! to fatal conditions (channel disconnect, thread exit).

use std::any::Any;
use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use dpi::PhysicalSize;
use hypercolor_types::canvas::Canvas;
use hypercolor_types::effect::{ControlValue, EffectCategory, EffectMetadata};
use reqwest::Url;
use servo::{
    JSValue, JavaScriptEvaluationError, Preferences, RenderingContext, Servo, ServoBuilder,
    WebView, WebViewBuilder,
};
use tracing::{debug, trace, warn};

use super::circuit_breaker::ServoCircuitBreaker;
use super::delegate::{ConsoleMessage, HypercolorWebViewDelegate};
use super::worker_client::{
    ServoSessionId, ServoWorkerClient, ServoWorkerClientSharedState, UNLOAD_TIMEOUT,
    WORKER_READY_TIMEOUT, WorkerCommand,
};
use crate::effect::servo_bootstrap::bootstrap_software_rendering_context;

pub(super) const LOAD_TIMEOUT: Duration = Duration::from_secs(5);
const URL_LOAD_TIMEOUT: Duration = Duration::from_secs(15);
const SCRIPT_TIMEOUT: Duration = Duration::from_millis(250);
pub(super) const RENDER_RESPONSE_TIMEOUT: Duration = Duration::from_millis(500);
pub(super) const RECENT_CONSOLE_SAMPLE_SIZE: usize = 6;
const CONSOLE_SNIPPET_RADIUS: usize = 1;
const CONSOLE_SNIPPET_LINE_MAX_CHARS: usize = 180;
const JS_TIMER_MIN_DURATION_MS: i64 = 4;

#[derive(Debug, Clone, Copy, Default)]
struct ServoRenderStageTimings {
    evaluate_scripts_us: u64,
    event_loop_us: u64,
    paint_us: u64,
    readback_us: u64,
    total_us: u64,
}

// The shared worker. Servo can only exist once per process; this OnceLock
// keeps the single instance alive across effect switches for the entire
// daemon lifetime.
static SERVO_WORKER: OnceLock<Mutex<SharedServoWorkerState>> = OnceLock::new();
static SERVO_CIRCUIT_BREAKER: ServoCircuitBreaker = ServoCircuitBreaker::new();

/// Acquire a client handle to the shared Servo worker, spawning it on first use.
pub(super) fn acquire_servo_worker() -> Result<ServoWorkerClient> {
    if !SERVO_CIRCUIT_BREAKER.can_attempt() {
        let cooldown = SERVO_CIRCUIT_BREAKER
            .cooldown_remaining()
            .unwrap_or(Duration::ZERO);
        bail!(
            "Servo worker is cooling down after repeated failures; retry in {}s",
            cooldown.as_secs().max(1)
        );
    }

    let slot = SERVO_WORKER.get_or_init(|| Mutex::new(SharedServoWorkerState::Vacant));
    let mut guard = match slot.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };

    match &mut *guard {
        SharedServoWorkerState::Running(worker) => {
            let client = worker.client();
            if client.is_ok() {
                SERVO_CIRCUIT_BREAKER.record_success();
            } else {
                SERVO_CIRCUIT_BREAKER.record_failure();
            }
            return client;
        }
        SharedServoWorkerState::Poisoned { reason } => {
            bail!("Servo runtime is unrecoverable until the daemon restarts: {reason}");
        }
        SharedServoWorkerState::Vacant => {}
    }

    match ServoWorker::spawn() {
        Ok(worker) => match worker.client() {
            Ok(client) => {
                *guard = SharedServoWorkerState::Running(worker);
                SERVO_CIRCUIT_BREAKER.record_success();
                Ok(client)
            }
            Err(error) => {
                SERVO_CIRCUIT_BREAKER.record_failure();
                Err(error)
            }
        },
        Err(error) => {
            SERVO_CIRCUIT_BREAKER.record_failure();
            Err(error)
        }
    }
}

/// Returns true if an error is fatal enough to retire the shared worker.
pub(super) fn servo_worker_is_fatal_error(error: &anyhow::Error) -> bool {
    let message = error.to_string().to_ascii_lowercase();
    message.contains("disconnected")
        || message.contains("timed out waiting for servo page load")
        || message.contains("timed out waiting for servo page unload")
        || message.contains("timed out waiting for servo webview close")
        || message.contains("timed out waiting for servo frame response")
        || message.contains("timed out waiting for javascript callback")
        || message.contains("failed to send load command to servo worker")
        || message.contains("failed to send render command to servo worker")
        || message.contains("failed to send unload command to servo worker")
}

/// Marks the shared worker poisoned if and only if the error is fatal.
pub(super) fn poison_shared_servo_worker_if_fatal(context: &str, error: &anyhow::Error) {
    if !servo_worker_is_fatal_error(error) {
        SERVO_CIRCUIT_BREAKER.record_failure();
        return;
    }
    let message = format!("{context}: {error}");
    poison_shared_servo_worker(&message);
}

/// Permanently marks the shared worker as unrecoverable.
pub(super) fn poison_shared_servo_worker(reason: &str) {
    SERVO_CIRCUIT_BREAKER.record_failure();

    let slot = SERVO_WORKER.get_or_init(|| Mutex::new(SharedServoWorkerState::Vacant));
    let mut guard = match slot.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };

    let previous = std::mem::replace(
        &mut *guard,
        SharedServoWorkerState::Poisoned {
            reason: reason.to_owned(),
        },
    );
    drop(guard);

    match previous {
        SharedServoWorkerState::Vacant => {
            warn!(
                reason = reason,
                "Marked shared Servo worker unrecoverable; restart the daemon to use HTML effects again"
            );
        }
        SharedServoWorkerState::Running(mut worker) => {
            let had_command_tx = worker.command_tx.take().is_some();
            let had_thread_handle = worker.thread_handle.take().is_some();
            warn!(
                reason = reason,
                had_command_tx,
                had_thread_handle,
                "Marked shared Servo worker unrecoverable; restart the daemon to use HTML effects again"
            );
        }
        SharedServoWorkerState::Poisoned { .. } => {}
    }
}

#[cfg(test)]
pub(super) fn shutdown_shared_servo_worker() -> Result<()> {
    let slot = SERVO_WORKER.get_or_init(|| Mutex::new(SharedServoWorkerState::Vacant));
    let mut guard = match slot.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };

    let previous = std::mem::replace(&mut *guard, SharedServoWorkerState::Vacant);
    drop(guard);

    match previous {
        SharedServoWorkerState::Vacant | SharedServoWorkerState::Poisoned { .. } => Ok(()),
        SharedServoWorkerState::Running(mut worker) => worker.shutdown(),
    }
}

/// Whether an effect should receive `engine.audio.*` updates each frame.
pub(super) fn effect_is_audio_reactive(metadata: &EffectMetadata) -> bool {
    if metadata.audio_reactive {
        return true;
    }

    if matches!(metadata.category, EffectCategory::Audio) {
        return true;
    }

    metadata
        .tags
        .iter()
        .any(|tag| tag.eq_ignore_ascii_case("audio") || tag.eq_ignore_ascii_case("audio-reactive"))
}

/// Prepare an HTML file with a runtime control preamble injected into `<head>`.
pub(super) fn prepare_runtime_html_source(
    original_path: &Path,
    controls: &HashMap<String, ControlValue>,
) -> Result<(PathBuf, Option<PathBuf>)> {
    let html = std::fs::read_to_string(original_path).with_context(|| {
        format!(
            "failed to read HTML effect file while preparing runtime source: {}",
            original_path.display()
        )
    })?;

    let preamble = build_control_preamble_script(controls);
    let base_tag = original_path
        .parent()
        .and_then(|parent| Url::from_directory_path(parent).ok())
        .map_or_else(String::new, |url| format!("<base href=\"{url}\">\n"));
    let injected_block = format!("{base_tag}<script>\n{preamble}\n</script>\n");
    let runtime_html = inject_runtime_head_block(&html, &injected_block);

    let cache_root = dirs::cache_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("hypercolor")
        .join("servo-runtime");
    std::fs::create_dir_all(&cache_root).with_context(|| {
        format!(
            "failed to create Servo runtime cache directory: {}",
            cache_root.display()
        )
    })?;

    let runtime_path = cache_root.join(format!("effect-{}.html", uuid::Uuid::now_v7()));
    std::fs::write(&runtime_path, runtime_html).with_context(|| {
        format!(
            "failed to write runtime HTML source '{}'",
            runtime_path.display()
        )
    })?;

    Ok((runtime_path.clone(), Some(runtime_path)))
}

fn build_control_preamble_script(controls: &HashMap<String, ControlValue>) -> String {
    let mut sorted_controls: Vec<_> = controls.iter().collect();
    sorted_controls.sort_by(|(left, _), (right, _)| left.cmp(right));

    let mut script = String::from("(function(){\n");
    script.push_str("  window.__hypercolorCaptureMode = true;\n");
    script.push_str("  window.__hypercolorPreserveDrawingBuffer = true;\n");
    script.push_str("  if (typeof globalThis === 'object' && globalThis !== null) {\n");
    script.push_str("    globalThis.__hypercolorCaptureMode = true;\n");
    script.push_str("    globalThis.__hypercolorPreserveDrawingBuffer = true;\n");
    script.push_str("  }\n");
    for (name, value) in sorted_controls {
        let key_literal = serde_json::to_string(name).unwrap_or_else(|_| "\"invalid\"".to_owned());
        let _ = writeln!(
            script,
            "  if (typeof globalThis[{key_literal}] === 'undefined') globalThis[{key_literal}] = {};",
            value.to_js_literal()
        );
    }
    script.push_str("})();");
    script
}

fn inject_runtime_head_block(html: &str, block: &str) -> String {
    let lowered = html.to_ascii_lowercase();

    if let Some(head_start) = lowered.find("<head")
        && let Some(head_close_offset) = lowered[head_start..].find('>')
    {
        let insert_at = head_start + head_close_offset + 1;
        let (before, after) = html.split_at(insert_at);
        return format!("{before}\n{block}{after}");
    }

    if let Some(script_start) = lowered.find("<script") {
        let (before, after) = html.split_at(script_start);
        return format!("{before}\n{block}{after}");
    }

    format!("{block}{html}")
}

/// Render the list of console messages as human-readable summaries.
pub(super) fn summarize_console_messages(
    messages: &[ConsoleMessage],
    fallback_source: Option<&Path>,
) -> Vec<String> {
    messages
        .iter()
        .map(|entry| format_console_message(entry, fallback_source))
        .collect()
}

/// Format a single console message, annotating it with source context when
/// the message mentions a `file://` URL we can locate on disk.
pub(super) fn format_console_message(
    entry: &ConsoleMessage,
    fallback_source: Option<&Path>,
) -> String {
    let mut formatted = format!("{}: {}", entry.level, entry.message);
    if let Some(location) = parse_console_source_location(&entry.message, fallback_source) {
        let _ = write!(
            formatted,
            " [{}:{}]",
            location.path.display(),
            location.line_number
        );
        if let Some(snippet) =
            load_source_snippet(&location.path, location.line_number, CONSOLE_SNIPPET_RADIUS)
        {
            let _ = write!(formatted, " {snippet}");
        }
    }
    formatted
}

/// Scan a batch of recent console messages for an initialization-failure
/// marker emitted by the effect runtime.
pub(super) fn find_initialization_failure_message(
    messages: &[ConsoleMessage],
) -> Option<&ConsoleMessage> {
    messages.iter().rev().find(|entry| {
        let is_error_level = matches!(entry.level.as_str(), "error" | "warn");
        let lower = entry.message.to_ascii_lowercase();
        is_error_level
            && (lower.contains("initialization failed") || lower.contains("failed to initialize"))
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ConsoleSourceLocation {
    path: PathBuf,
    line_number: usize,
}

fn parse_console_source_location(
    message: &str,
    fallback_source: Option<&Path>,
) -> Option<ConsoleSourceLocation> {
    let url_token = extract_file_url_token(message);
    let line_number = url_token
        .as_deref()
        .and_then(extract_line_from_file_url_token)
        .or_else(|| extract_line_after_file_url_token(message, url_token.as_deref()))
        .or_else(|| extract_line_word_pattern(message))?;
    if line_number == 0 {
        return None;
    }

    let candidate_path = url_token.as_deref().and_then(|token| {
        let path = file_url_token_to_path(token)?;
        if file_url_token_points_to_directory(token) || path.is_dir() {
            return None;
        }
        Some(path)
    });
    let path = candidate_path.or_else(|| fallback_source.map(Path::to_path_buf))?;
    Some(ConsoleSourceLocation { path, line_number })
}

fn extract_file_url_token(message: &str) -> Option<String> {
    let start = message.find("file://")?;
    let tail = &message[start..];
    let end = tail
        .find(|ch: char| ['"', '\'', ')', '(', ',', ' ', '\t', '\n', '\r'].contains(&ch))
        .unwrap_or(tail.len());
    Some(tail[..end].to_owned())
}

fn extract_line_from_file_url_token(token: &str) -> Option<usize> {
    let mut remaining = token;
    let mut trailing_numbers: Vec<usize> = Vec::new();

    while let Some((prefix, value)) = split_trailing_colon_number(remaining) {
        trailing_numbers.push(value);
        remaining = prefix;
    }

    if trailing_numbers.is_empty() {
        return None;
    }

    // file:///path.js:123:45 -> trailing_numbers == [45, 123]
    if trailing_numbers.len() >= 2 {
        return Some(trailing_numbers[1]);
    }
    Some(trailing_numbers[0])
}

fn split_trailing_colon_number(input: &str) -> Option<(&str, usize)> {
    let (prefix, suffix) = input.rsplit_once(':')?;
    if suffix.is_empty() || !suffix.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let value = suffix.parse::<usize>().ok()?;
    Some((prefix, value))
}

fn extract_line_after_file_url_token(message: &str, token: Option<&str>) -> Option<usize> {
    let token = token?;
    let start = message.find(token)?;
    let rest = &message[start + token.len()..];
    let comma = rest.find(',')?;
    parse_first_number(&rest[comma + 1..])
}

fn parse_first_number(input: &str) -> Option<usize> {
    let start = input.find(|ch: char| ch.is_ascii_digit())?;
    let digits = input[start..]
        .chars()
        .take_while(char::is_ascii_digit)
        .collect::<String>();
    digits.parse::<usize>().ok()
}

fn extract_line_word_pattern(message: &str) -> Option<usize> {
    let lower = message.to_ascii_lowercase();
    let marker = "line ";
    let start = lower.find(marker)?;
    parse_first_number(&lower[start + marker.len()..])
}

fn file_url_token_to_path(token: &str) -> Option<PathBuf> {
    let trimmed = strip_file_url_line_suffix(token);
    let parsed = Url::parse(trimmed).ok()?;
    if parsed.scheme() != "file" {
        return None;
    }
    parsed.to_file_path().ok()
}

fn strip_file_url_line_suffix(token: &str) -> &str {
    let mut trimmed = token;
    while let Some((prefix, _)) = split_trailing_colon_number(trimmed) {
        trimmed = prefix;
    }
    trimmed
}

fn file_url_token_points_to_directory(token: &str) -> bool {
    strip_file_url_line_suffix(token).ends_with('/')
}

fn load_source_snippet(path: &Path, line_number: usize, radius: usize) -> Option<String> {
    if line_number == 0 {
        return None;
    }

    let contents = std::fs::read_to_string(path).ok()?;
    let lines: Vec<&str> = contents.lines().collect();
    if line_number > lines.len() {
        return None;
    }

    let start = line_number.saturating_sub(radius).max(1);
    let end = line_number.saturating_add(radius).min(lines.len());
    let mut window = Vec::with_capacity(end.saturating_sub(start) + 1);
    for idx in start..=end {
        let marker = if idx == line_number { ">" } else { "-" };
        let content = truncate_for_log(lines[idx - 1].trim(), CONSOLE_SNIPPET_LINE_MAX_CHARS);
        window.push(format!("{marker}{idx}: {content}"));
    }
    Some(window.join(" || "))
}

fn truncate_for_log(input: &str, max_chars: usize) -> String {
    let mut chars = input.chars();
    let truncated: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        return format!("{truncated}...");
    }
    truncated
}

fn panic_payload_message(payload: &(dyn Any + Send + 'static)) -> String {
    if let Some(message) = payload.downcast_ref::<String>() {
        return message.clone();
    }
    if let Some(message) = payload.downcast_ref::<&'static str>() {
        return (*message).to_owned();
    }
    "unknown panic payload".to_owned()
}

fn install_rustls_provider() {
    if let Err(error) = rustls::crypto::aws_lc_rs::default_provider().install_default() {
        trace!(?error, "Rustls provider already initialized or unavailable");
    }
}

fn file_url_for_path(path: &Path) -> Result<Url> {
    let canonical_path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    Url::from_file_path(&canonical_path).map_err(|()| {
        anyhow!(
            "failed to convert '{}' to file:// URL",
            canonical_path.display()
        )
    })
}

/// Read Servo's composited framebuffer into a fresh `Canvas`.
///
/// Bypasses `servo-paint-api::Framebuffer::read_framebuffer_to_image`, which
/// allocates, calls `glReadPixels`, clones the whole `Vec<u8>` so it can
/// flip rows into the original via `clone_from_slice`. On a 640×480×4 frame
/// that's two extra full-buffer passes over ≈1.2 MB beyond the unavoidable
/// `glReadPixels` DMA. Profile attributed that pair to ~45% of the Servo
/// worker thread as raw libc memmove.
///
/// Here we read directly into an owned `Vec<u8>` and swap rows in place —
/// each byte moves at most once, so the flip is half the cost of Servo's
/// default and there's no separate clone. The `bind_vertex_array(0)` call
/// is the OSMesa workaround that Servo's upstream implementation keeps for
/// its own reasons; preserve it so the headless adapter stays honest.
fn read_framebuffer_into_canvas(session: &ServoSession, width: i32, height: i32) -> Result<Canvas> {
    use gleam::gl;

    if width <= 0 || height <= 0 {
        bail!("Servo readback rectangle has non-positive dimensions ({width}×{height})");
    }

    let width_u32 = u32::try_from(width).context("servo readback width overflow")?;
    let height_u32 = u32::try_from(height).context("servo readback height overflow")?;

    session.rendering_context.prepare_for_rendering();
    let gl = session.rendering_context.gleam_gl_api();
    gl.bind_vertex_array(0);

    let mut pixels = gl.read_pixels(0, 0, width, height, gl::RGBA, gl::UNSIGNED_BYTE);
    let gl_error = gl.get_error();
    if gl_error != gl::NO_ERROR {
        warn!("GL error 0x{gl_error:x} raised during Servo framebuffer readback");
    }

    let stride = usize::try_from(width)
        .ok()
        .and_then(|w| w.checked_mul(4))
        .context("servo readback row stride overflow")?;
    let expected_len = stride
        .checked_mul(usize::try_from(height).context("servo readback height overflow")?)
        .context("servo readback buffer length overflow")?;
    if pixels.len() != expected_len {
        bail!(
            "Servo readback returned {} bytes; expected {} ({}×{}×4)",
            pixels.len(),
            expected_len,
            width,
            height
        );
    }

    flip_rows_in_place(&mut pixels, stride);

    Ok(Canvas::from_vec(pixels, width_u32, height_u32))
}

/// Swap pairs of rows in a row-major RGBA buffer to flip it vertically.
///
/// OpenGL's `glReadPixels` places (0,0) at the bottom-left of the source
/// framebuffer, but `Canvas` expects top-left origin. Walking from both
/// ends with `swap_with_slice` lets each byte move exactly once — no
/// scratch buffer, no per-row clone.
fn flip_rows_in_place(pixels: &mut [u8], stride: usize) {
    if stride == 0 {
        return;
    }
    let row_count = pixels.len() / stride;
    if row_count < 2 {
        return;
    }
    let mut top = 0;
    let mut bottom = row_count - 1;
    while top < bottom {
        let top_start = top * stride;
        let bottom_start = bottom * stride;
        let (upper, lower) = pixels.split_at_mut(bottom_start);
        upper[top_start..top_start + stride].swap_with_slice(&mut lower[..stride]);
        top += 1;
        bottom -= 1;
    }
}

fn trimmed_servo_preferences() -> Preferences {
    Preferences {
        js_timers_minimum_duration: JS_TIMER_MIN_DURATION_MS,
        // Workshop effects require WebGL + OffscreenCanvas composition.
        dom_webgl2_enabled: true,
        dom_offscreen_canvas_enabled: true,
        // Hypercolor effects render a single offscreen document; parallel CSS
        // parsing and extra style workers are wasted overhead here.
        dom_parallel_css_parsing_enabled: false,
        layout_style_sharing_cache_enabled: false,
        layout_threads: 1,
        // Keep Servo's task pools small for the single-effect embedder case.
        threadpools_async_runtime_workers_max: 1,
        threadpools_image_cache_workers_max: 1,
        threadpools_indexeddb_workers_max: 1,
        threadpools_webstorage_workers_max: 1,
        threadpools_webrender_workers_max: 1,
        network_http_cache_disabled: true,
        // Disable subsystems Hypercolor does not use.
        devtools_server_enabled: false,
        dom_gamepad_enabled: false,
        dom_indexeddb_enabled: false,
        dom_serviceworker_enabled: false,
        dom_webgpu_enabled: false,
        dom_webrtc_enabled: false,
        dom_webrtc_transceiver_enabled: false,
        dom_webxr_enabled: false,
        dom_webxr_glwindow_enabled: false,
        dom_webxr_hands_enabled: false,
        dom_webxr_openxr_enabled: false,
        dom_worklet_enabled: false,
        // JIT buys little for short-lived effect scripts and spins up extra
        // SpiderMonkey helper capacity.
        js_disable_jit: true,
        js_baseline_jit_enabled: false,
        js_ion_enabled: false,
        js_offthread_compilation_enabled: false,
        js_ion_unsafe_eager_compilation_enabled: false,
        media_glvideo_enabled: false,
        shell_background_color_rgba: [0.0, 0.0, 0.0, 0.0],
        ..Preferences::default()
    }
}

fn script_preview(script: &str) -> String {
    let single_line = script.split_whitespace().collect::<Vec<_>>().join(" ");
    truncate_for_log(&single_line, 120)
}

fn batched_script_preview(scripts: &[String]) -> String {
    if scripts.len() == 1 {
        return script_preview(&scripts[0]);
    }

    let previews = scripts
        .iter()
        .take(3)
        .map(|script| script_preview(script))
        .collect::<Vec<_>>()
        .join(" | ");
    format!("{} scripts: {previews}", scripts.len())
}

fn can_reuse_cached_canvas(
    frame_ready: bool,
    script_count: usize,
    cached: Option<&Canvas>,
    width: u32,
    height: u32,
) -> bool {
    !frame_ready
        && script_count == 0
        && cached.is_some_and(|cached| cached.width() == width && cached.height() == height)
}

fn combined_script(buffer: &mut String, scripts: &[String]) {
    let capacity = scripts.iter().map(String::len).sum::<usize>() + scripts.len();
    buffer.clear();
    if buffer.capacity() < capacity {
        buffer.reserve(capacity - buffer.capacity());
    }
    for script in scripts {
        buffer.push_str(script);
        buffer.push('\n');
    }
}

fn elapsed_micros(start: Instant) -> u64 {
    start.elapsed().as_micros().try_into().unwrap_or(u64::MAX)
}

fn log_servo_render_stage_timings(
    session_id: ServoSessionId,
    width: u32,
    height: u32,
    script_count: usize,
    script_bytes: usize,
    frame_ready: bool,
    reused_cached_canvas: bool,
    timings: ServoRenderStageTimings,
) {
    trace!(
        ?session_id,
        width,
        height,
        script_count,
        script_bytes,
        frame_ready,
        reused_cached_canvas,
        evaluate_scripts_us = timings.evaluate_scripts_us,
        event_loop_us = timings.event_loop_us,
        paint_us = timings.paint_us,
        readback_us = timings.readback_us,
        total_us = timings.total_us,
        "Servo render stage timings"
    );
}

enum SharedServoWorkerState {
    Vacant,
    Running(ServoWorker),
    Poisoned { reason: String },
}

/// Owner of the Servo worker thread and its command channel.
pub(super) struct ServoWorker {
    pub(super) command_tx: Option<Sender<WorkerCommand>>,
    pub(super) thread_handle: Option<thread::JoinHandle<()>>,
    pub(super) client_state: std::sync::Arc<ServoWorkerClientSharedState>,
}

impl ServoWorker {
    fn spawn() -> Result<Self> {
        let (command_tx, command_rx) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let client_state = std::sync::Arc::new(ServoWorkerClientSharedState::new());

        let thread_handle = thread::Builder::new()
            .name("hypercolor-servo-worker".to_owned())
            .spawn(move || {
                let runtime = match ServoWorkerRuntime::new() {
                    Ok(runtime) => {
                        let _ = ready_tx.send(Ok(()));
                        runtime
                    }
                    Err(error) => {
                        let _ = ready_tx.send(Err(error));
                        return;
                    }
                };
                runtime.run(command_rx);
            })
            .context("failed to spawn Servo worker thread")?;

        let readiness = match ready_rx.recv_timeout(WORKER_READY_TIMEOUT) {
            Ok(readiness) => readiness,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                bail!(
                    "timed out waiting for Servo worker readiness after {}ms",
                    WORKER_READY_TIMEOUT.as_millis()
                );
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                bail!("Servo worker exited before reporting readiness");
            }
        };
        readiness?;

        Ok(Self {
            command_tx: Some(command_tx),
            thread_handle: Some(thread_handle),
            client_state,
        })
    }

    fn client(&self) -> Result<ServoWorkerClient> {
        Ok(ServoWorkerClient::new(
            self.command_tx()?.clone(),
            std::sync::Arc::clone(&self.client_state),
        ))
    }

    fn shutdown(&mut self) -> Result<()> {
        let command_tx = self.command_tx.take();
        if let Some(command_tx) = command_tx {
            let (response_tx, response_rx) = mpsc::sync_channel(1);
            if command_tx
                .send(WorkerCommand::Shutdown { response_tx })
                .is_ok()
            {
                match response_rx.recv_timeout(WORKER_READY_TIMEOUT) {
                    Ok(()) => {}
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        bail!(
                            "timed out waiting for Servo worker shutdown after {}ms",
                            WORKER_READY_TIMEOUT.as_millis()
                        );
                    }
                    Err(mpsc::RecvTimeoutError::Disconnected) => {
                        bail!("Servo worker disconnected before acknowledging shutdown");
                    }
                }
            }
        }

        if let Some(thread_handle) = self.thread_handle.take() {
            thread_handle.join().map_err(|panic| {
                anyhow!(
                    "Servo worker thread panicked during shutdown: {}",
                    panic_payload_message(&*panic)
                )
            })?;
        }

        Ok(())
    }

    fn command_tx(&self) -> Result<&Sender<WorkerCommand>> {
        self.command_tx
            .as_ref()
            .ok_or_else(|| anyhow!("Servo worker is not running"))
    }
}

impl Drop for ServoWorker {
    fn drop(&mut self) {
        if let Err(error) = self.shutdown() {
            warn!(%error, "Servo worker dropped with shutdown error");
        }
    }
}

struct ServoSession {
    webview: Option<WebView>,
    rendering_context: Rc<dyn RenderingContext>,
    delegate: Rc<HypercolorWebViewDelegate>,
    loaded_html_path: Option<PathBuf>,
    script_buffer: String,
    /// Most recent successful readback, retained so ticks where Servo has
    /// nothing new to composite can skip `read_to_image` entirely. Cloning a
    /// `Canvas` is an Arc refcount bump (zero-copy), so repeated reuse costs
    /// nothing beyond the flag check.
    last_canvas: Option<Canvas>,
}

struct ServoWorkerRuntime {
    sessions: HashMap<ServoSessionId, ServoSession>,
    servo: Servo,
}

impl ServoWorkerRuntime {
    fn new() -> Result<Self> {
        install_rustls_provider();

        let servo = ServoBuilder::default()
            .preferences(trimmed_servo_preferences())
            .build();
        Ok(Self {
            sessions: HashMap::new(),
            servo,
        })
    }

    fn run(mut self, command_rx: Receiver<WorkerCommand>) {
        for command in command_rx {
            match command {
                WorkerCommand::CreateSession {
                    session_id,
                    width,
                    height,
                    response_tx,
                } => {
                    let result = self.create_session(session_id, width, height);
                    let _ = response_tx.send(result);
                }
                WorkerCommand::Load {
                    session_id,
                    html_path,
                    width,
                    height,
                    response_tx,
                } => {
                    let result = self.load_effect(session_id, &html_path, width, height);
                    let _ = response_tx.send(result);
                }
                WorkerCommand::LoadUrl {
                    session_id,
                    url,
                    width,
                    height,
                    response_tx,
                } => {
                    let result = self.load_url(session_id, &url, width, height);
                    let _ = response_tx.send(result);
                }
                WorkerCommand::Unload {
                    session_id,
                    response_tx,
                } => {
                    let result = self.unload_effect(session_id);
                    let _ = response_tx.send(result);
                }
                WorkerCommand::Render {
                    session_id,
                    scripts,
                    width,
                    height,
                    response_tx,
                } => {
                    let result = self.render_frame(session_id, &scripts, width, height);
                    let _ = response_tx.send(result);
                }
                WorkerCommand::DestroySession {
                    session_id,
                    response_tx,
                } => {
                    let result = self.destroy_session(session_id);
                    let _ = response_tx.send(result);
                }
                WorkerCommand::Shutdown { response_tx } => {
                    let _ = response_tx.send(());
                    break;
                }
            }
        }

        let Self { sessions, servo } = self;
        drop(sessions);
        drop(servo);
    }

    fn create_session(
        &mut self,
        session_id: ServoSessionId,
        width: u32,
        height: u32,
    ) -> Result<()> {
        if self.sessions.contains_key(&session_id) {
            bail!("Servo session {session_id:?} already exists");
        }

        let rendering_context = Self::create_rendering_context(width, height)?;
        rendering_context.make_current().map_err(|error| {
            anyhow!("failed to make Servo rendering context current: {error:?}")
        })?;
        let delegate = Rc::new(HypercolorWebViewDelegate::new());
        let url = Url::parse("about:blank").context("failed to parse about:blank URL")?;
        let webview = Self::build_webview(
            &self.servo,
            Rc::clone(&rendering_context),
            delegate.clone(),
            url,
        );

        self.sessions.insert(
            session_id,
            ServoSession {
                webview: Some(webview),
                rendering_context,
                delegate,
                loaded_html_path: None,
                script_buffer: String::new(),
                last_canvas: None,
            },
        );

        if let Err(error) = self.wait_for_load_completion(session_id, WORKER_READY_TIMEOUT, None) {
            self.sessions.remove(&session_id);
            return Err(error);
        }

        Ok(())
    }

    fn create_rendering_context(width: u32, height: u32) -> Result<Rc<dyn RenderingContext>> {
        Ok(Rc::new(bootstrap_software_rendering_context(
            width, height,
        )?))
    }

    fn build_webview(
        servo: &Servo,
        rendering_context: Rc<dyn RenderingContext>,
        delegate: Rc<HypercolorWebViewDelegate>,
        url: Url,
    ) -> WebView {
        let webview = WebViewBuilder::new(servo, rendering_context)
            .delegate(delegate)
            .url(url)
            .build();
        webview.set_throttled(true);
        webview
    }

    fn session(&self, session_id: ServoSessionId) -> Result<&ServoSession> {
        self.sessions
            .get(&session_id)
            .ok_or_else(|| anyhow!("Servo session {session_id:?} is not initialized"))
    }

    fn session_mut(&mut self, session_id: ServoSessionId) -> Result<&mut ServoSession> {
        self.sessions
            .get_mut(&session_id)
            .ok_or_else(|| anyhow!("Servo session {session_id:?} is not initialized"))
    }

    fn active_webview(&self, session_id: ServoSessionId) -> Result<&WebView> {
        self.session(session_id)?
            .webview
            .as_ref()
            .ok_or_else(|| anyhow!("Servo webview is not initialized"))
    }

    fn close_webview(&mut self, session_id: ServoSessionId) -> Result<()> {
        // Dropping the last WebView handle synchronously issues Servo's
        // `CloseWebView` message. `notify_closed` is only documented for
        // `window.close()`, so waiting on it here just turns every effect
        // switch into a timeout path.
        let Some(webview) = self.session_mut(session_id)?.webview.take() else {
            return Ok(());
        };
        drop(webview);
        self.servo.spin_event_loop();
        Ok(())
    }

    fn replace_webview(
        &mut self,
        session_id: ServoSessionId,
        url: Url,
        timeout: Duration,
    ) -> Result<()> {
        // Dropping the last handle closes the old webview and lets Servo tear
        // down page-scoped resources before we build the next one.
        self.close_webview(session_id)
            .context("failed to close previous Servo webview")?;
        {
            let session = self.session_mut(session_id)?;
            session.delegate.reset_navigation_state();
        }
        let (rendering_context, delegate) = {
            let session = self.session(session_id)?;
            (
                Rc::clone(&session.rendering_context),
                session.delegate.clone(),
            )
        };
        let webview = Self::build_webview(&self.servo, rendering_context, delegate, url.clone());
        self.session_mut(session_id)?.webview = Some(webview);
        self.wait_for_load_completion(session_id, timeout, Some(url.as_str()))
    }

    fn load_effect(
        &mut self,
        session_id: ServoSessionId,
        html_path: &Path,
        width: u32,
        height: u32,
    ) -> Result<()> {
        let had_loaded_effect = self.session(session_id)?.loaded_html_path.is_some();
        self.resize_if_needed(session_id, width, height)?;
        let url = file_url_for_path(html_path)?;
        self.session_mut(session_id)?.loaded_html_path = Some(html_path.to_path_buf());
        debug!(url = %url, "Loading Servo effect page");
        if had_loaded_effect {
            self.replace_webview(session_id, url.clone(), LOAD_TIMEOUT)
                .context(
                    "failed to replace previous Servo effect page before loading new effect",
                )?;
        } else {
            self.session(session_id)?.delegate.reset_navigation_state();
            {
                let webview = self.active_webview(session_id)?;
                webview.load(url.clone());
            }
            self.wait_for_load_completion(session_id, LOAD_TIMEOUT, Some(url.as_str()))?;
        }
        let recent_console_entries = self
            .session(session_id)?
            .delegate
            .recent_console_messages(RECENT_CONSOLE_SAMPLE_SIZE);
        let recent_console = summarize_console_messages(
            &recent_console_entries,
            self.session(session_id)?.loaded_html_path.as_deref(),
        );
        if !recent_console.is_empty() {
            debug!(
                url = %url,
                recent_console = ?recent_console,
                "Recent console output while loading Servo effect page"
            );
        }
        if let Some(message) = find_initialization_failure_message(&recent_console_entries) {
            let loaded_html_path = self.session(session_id)?.loaded_html_path.clone();
            bail!(
                "effect initialization failed: {}",
                format_console_message(message, loaded_html_path.as_deref())
            );
        }
        Ok(())
    }

    fn load_url(
        &mut self,
        session_id: ServoSessionId,
        url: &str,
        width: u32,
        height: u32,
    ) -> Result<()> {
        let had_loaded_content = self
            .session(session_id)?
            .delegate
            .last_url()
            .as_deref()
            .is_some_and(|current| current != "about:blank");
        self.resize_if_needed(session_id, width, height)?;
        let parsed_url = Url::parse(url).with_context(|| format!("failed to parse URL '{url}'"))?;
        self.session_mut(session_id)?.loaded_html_path = None;
        debug!(url = %parsed_url, "Loading Servo URL");
        if had_loaded_content {
            self.replace_webview(session_id, parsed_url.clone(), URL_LOAD_TIMEOUT)
                .context("failed to replace previous Servo page before loading URL")?;
        } else {
            self.session(session_id)?.delegate.reset_navigation_state();
            {
                let webview = self.active_webview(session_id)?;
                webview.load(parsed_url.clone());
            }
            self.wait_for_load_completion(session_id, URL_LOAD_TIMEOUT, Some(parsed_url.as_str()))?;
        }
        let recent_console = summarize_console_messages(
            &self
                .session(session_id)?
                .delegate
                .recent_console_messages(RECENT_CONSOLE_SAMPLE_SIZE),
            None,
        );
        if !recent_console.is_empty() {
            debug!(
                url = %parsed_url,
                recent_console = ?recent_console,
                "Recent console output while loading Servo URL"
            );
        }
        Ok(())
    }

    fn unload_effect(&mut self, session_id: ServoSessionId) -> Result<()> {
        if self.session(session_id)?.loaded_html_path.is_none() {
            return Ok(());
        }

        let url = Url::parse("about:blank").context("failed to parse about:blank URL")?;
        debug!("Unloading Servo effect page");
        self.replace_webview(session_id, url.clone(), UNLOAD_TIMEOUT)?;
        self.session_mut(session_id)?.loaded_html_path = None;
        Ok(())
    }

    fn destroy_session(&mut self, session_id: ServoSessionId) -> Result<()> {
        let Some(mut session) = self.sessions.remove(&session_id) else {
            return Ok(());
        };

        if let Some(webview) = session.webview.take() {
            drop(webview);
            self.servo.spin_event_loop();
        }

        drop(session);
        Ok(())
    }

    fn render_frame(
        &mut self,
        session_id: ServoSessionId,
        scripts: &[String],
        width: u32,
        height: u32,
    ) -> Result<Canvas> {
        let script_count = scripts.len();
        let script_bytes = scripts.iter().map(String::len).sum::<usize>();
        let result = (|| {
            let frame_start = Instant::now();
            let mut timings = ServoRenderStageTimings::default();
            self.resize_if_needed(session_id, width, height)?;

            let evaluate_scripts_start = Instant::now();
            self.evaluate_scripts(session_id, scripts)?;
            timings.evaluate_scripts_us = elapsed_micros(evaluate_scripts_start);

            // Let timers/RAF advance for one daemon-driven frame after scripts
            // have injected controls/audio for this tick. Leaving the webview
            // unthrottled between ticks lets effect-side RAF/timer loops free-run.
            self.active_webview(session_id)?.set_throttled(false);
            let event_loop_start = Instant::now();
            self.servo.spin_event_loop();
            timings.event_loop_us = elapsed_micros(event_loop_start);
            let frame_ready = self.session(session_id)?.delegate.take_frame_ready();
            if frame_ready {
                trace!("Servo delegate signaled new frame");
            }

            // Fast path: the delegate didn't observe a fresh composition this
            // tick, so `paint()` + `read_to_image()` would just re-deliver
            // bytes we already have. `read_framebuffer_to_image` in
            // servo-paint-api does a `glReadPixels` + full `Vec::clone` + a
            // per-row flip — three passes over a 640×480×4 (≈1.2 MB) buffer
            // on the Servo worker thread. Skipping that when nothing changed
            // was the single biggest memmove win in the profile, and
            // `Canvas::clone` is an Arc bump so reuse is effectively free.
            if can_reuse_cached_canvas(
                frame_ready,
                script_count,
                self.session(session_id)?.last_canvas.as_ref(),
                width,
                height,
            ) {
                let cached = self
                    .session(session_id)?
                    .last_canvas
                    .as_ref()
                    .expect("cached canvas presence should match reuse check");
                timings.total_us = elapsed_micros(frame_start);
                log_servo_render_stage_timings(
                    session_id,
                    width,
                    height,
                    script_count,
                    script_bytes,
                    frame_ready,
                    true,
                    timings,
                );
                return Ok(cached.clone());
            }

            let paint_start = Instant::now();
            self.active_webview(session_id)?.paint();
            timings.paint_us = elapsed_micros(paint_start);

            let size = self.session(session_id)?.rendering_context.size();
            let width_i32 =
                i32::try_from(size.width).context("canvas width overflow for Servo readback")?;
            let height_i32 =
                i32::try_from(size.height).context("canvas height overflow for Servo readback")?;

            let readback_start = Instant::now();
            let canvas =
                read_framebuffer_into_canvas(self.session(session_id)?, width_i32, height_i32)?;
            timings.readback_us = elapsed_micros(readback_start);
            self.session_mut(session_id)?.last_canvas = Some(canvas.clone());
            timings.total_us = elapsed_micros(frame_start);
            log_servo_render_stage_timings(
                session_id,
                width,
                height,
                script_count,
                script_bytes,
                frame_ready,
                false,
                timings,
            );
            Ok(canvas)
        })();
        result
    }

    fn evaluate_scripts(&mut self, session_id: ServoSessionId, scripts: &[String]) -> Result<()> {
        if scripts.is_empty() {
            return Ok(());
        }

        let mut script_buffer = {
            let session = self.session_mut(session_id)?;
            std::mem::take(&mut session.script_buffer)
        };
        combined_script(&mut script_buffer, scripts);
        let result = self
            .evaluate_script(session_id, &script_buffer)
            .with_context(|| {
                format!(
                    "failed to evaluate script batch: {}",
                    batched_script_preview(scripts)
                )
            });
        self.session_mut(session_id)?.script_buffer = script_buffer;
        result
    }

    fn resize_if_needed(&self, session_id: ServoSessionId, width: u32, height: u32) -> Result<()> {
        let new_size = PhysicalSize::new(width, height);
        let session = self.session(session_id)?;
        if session.rendering_context.size() == new_size {
            return Ok(());
        }

        session.rendering_context.resize(new_size);
        self.active_webview(session_id)?.resize(new_size);
        Ok(())
    }

    fn evaluate_script(&mut self, session_id: ServoSessionId, script: &str) -> Result<()> {
        let result_slot: Rc<RefCell<Option<Result<JSValue, JavaScriptEvaluationError>>>> =
            Rc::new(RefCell::new(None));
        let callback_slot = Rc::clone(&result_slot);

        self.active_webview(session_id)?
            .evaluate_javascript(script, move |result| {
                *callback_slot.borrow_mut() = Some(result);
            });

        let deadline = Instant::now() + SCRIPT_TIMEOUT;
        while result_slot.borrow().is_none() {
            self.servo.spin_event_loop();
            if Instant::now() >= deadline {
                bail!("timed out waiting for JavaScript callback");
            }
            std::thread::sleep(Duration::from_millis(1));
        }

        let result = result_slot
            .borrow_mut()
            .take()
            .ok_or_else(|| anyhow!("missing JavaScript callback result"))?;
        result.map(|_| ()).map_err(|error| {
            let session = self.session(session_id).ok();
            let recent_console = session.map_or_else(Vec::new, |session| {
                summarize_console_messages(
                    &session
                        .delegate
                        .recent_console_messages(RECENT_CONSOLE_SAMPLE_SIZE),
                    session.loaded_html_path.as_deref(),
                )
            });
            let mut message = format!("javascript evaluation failed: {error:?}");
            if !recent_console.is_empty() {
                let _ = write!(message, "; recent console: {}", recent_console.join(" | "));
            }
            anyhow!(message)
        })
    }

    fn wait_for_load_completion(
        &mut self,
        session_id: ServoSessionId,
        timeout: Duration,
        expected_url: Option<&str>,
    ) -> Result<()> {
        let deadline = Instant::now() + timeout;

        loop {
            self.servo.spin_event_loop();
            let delegate = self.session(session_id)?.delegate.clone();
            let loaded = delegate.is_page_loaded();
            let last_url = delegate.last_url();
            let url_matches = load_completion_url_matches(expected_url, last_url.as_deref());
            if loaded && url_matches {
                delegate.take_page_loaded();
                debug!("Servo page load completed");
                return Ok(());
            }

            if Instant::now() >= deadline {
                let loaded_html_path = self.session(session_id)?.loaded_html_path.clone();
                let recent_console = summarize_console_messages(
                    &delegate.recent_console_messages(RECENT_CONSOLE_SAMPLE_SIZE),
                    loaded_html_path.as_deref(),
                );
                let current_url = last_url.unwrap_or_else(|| "<unknown>".to_owned());
                let mut message = format!(
                    "timed out waiting for Servo page load completion (expected_url={expected_url:?}, current_url={current_url})"
                );
                if !recent_console.is_empty() {
                    let _ = write!(message, "; recent console: {}", recent_console.join(" | "));
                }
                bail!("{message}");
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}

fn load_completion_url_matches(expected_url: Option<&str>, current_url: Option<&str>) -> bool {
    let Some(expected_url) = expected_url else {
        return true;
    };
    let Some(current_url) = current_url else {
        return false;
    };
    if current_url == expected_url {
        return true;
    }

    // Arbitrary webpages often redirect to canonical hosts, locale paths, or
    // consent/login URLs. Once the fresh webview reports a completed load on a
    // non-blank document, treat it as success instead of timing out forever on
    // an exact-URL mismatch.
    !current_url.eq_ignore_ascii_case("about:blank")
}

#[cfg(test)]
pub(super) fn reset_shared_servo_worker_state() {
    let slot = SERVO_WORKER.get_or_init(|| Mutex::new(SharedServoWorkerState::Vacant));
    let mut guard = match slot.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    *guard = SharedServoWorkerState::Vacant;
}

#[cfg(test)]
pub(super) fn install_running_shared_worker(worker: ServoWorker) {
    let slot = SERVO_WORKER.get_or_init(|| Mutex::new(SharedServoWorkerState::Vacant));
    let mut guard = match slot.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    *guard = SharedServoWorkerState::Running(worker);
}

#[cfg(test)]
pub(super) fn install_poisoned_shared_worker(reason: impl Into<String>) {
    let slot = SERVO_WORKER.get_or_init(|| Mutex::new(SharedServoWorkerState::Vacant));
    let mut guard = match slot.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    *guard = SharedServoWorkerState::Poisoned {
        reason: reason.into(),
    };
}

#[cfg(test)]
pub(super) fn shared_worker_is_vacant() -> bool {
    let slot = SERVO_WORKER.get_or_init(|| Mutex::new(SharedServoWorkerState::Vacant));
    let guard = match slot.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    matches!(&*guard, SharedServoWorkerState::Vacant)
}

#[cfg(test)]
pub(super) mod test_support {
    use std::sync::Arc;
    use std::sync::LazyLock;
    use std::sync::Mutex as StdMutex;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc::{self, Receiver, Sender};
    use std::thread;

    use hypercolor_types::canvas::Canvas;

    use super::super::worker_client::{
        ServoWorkerClient, ServoWorkerClientSharedState, WorkerCommand,
    };
    use super::ServoWorker;

    pub static SHARED_WORKER_STATE_TEST_LOCK: LazyLock<StdMutex<()>> =
        LazyLock::new(|| StdMutex::new(()));

    pub struct RecordedRenderCommand {
        pub scripts: Vec<String>,
        pub width: u32,
        pub height: u32,
    }

    pub struct RecordedLoadCommand {
        pub width: u32,
        pub height: u32,
    }

    fn solid_canvas(r: u8, g: u8, b: u8) -> Canvas {
        use hypercolor_types::canvas::{DEFAULT_CANVAS_HEIGHT, DEFAULT_CANVAS_WIDTH, Rgba};
        let mut canvas = Canvas::new(DEFAULT_CANVAS_WIDTH, DEFAULT_CANVAS_HEIGHT);
        canvas.fill(Rgba::new(r, g, b, 255));
        canvas
    }

    pub fn spawn_test_worker() -> (ServoWorker, Arc<AtomicBool>) {
        let (command_tx, command_rx) = mpsc::channel();
        let client_state = Arc::new(ServoWorkerClientSharedState::new());
        let stopped = Arc::new(AtomicBool::new(false));
        let stopped_clone = Arc::clone(&stopped);
        let thread_handle = thread::spawn(move || {
            while let Ok(command) = command_rx.recv() {
                match command {
                    WorkerCommand::CreateSession { response_tx, .. }
                    | WorkerCommand::Unload { response_tx, .. }
                    | WorkerCommand::Load { response_tx, .. }
                    | WorkerCommand::LoadUrl { response_tx, .. }
                    | WorkerCommand::DestroySession { response_tx, .. } => {
                        let _ = response_tx.send(Ok(()));
                    }
                    WorkerCommand::Shutdown { response_tx } => {
                        stopped_clone.store(true, Ordering::SeqCst);
                        let _ = response_tx.send(());
                        break;
                    }
                    WorkerCommand::Render { response_tx, .. } => {
                        let _ = response_tx.send(Ok(solid_canvas(12, 34, 56)));
                    }
                }
            }
        });

        (
            ServoWorker {
                command_tx: Some(command_tx),
                thread_handle: Some(thread_handle),
                client_state,
            },
            stopped,
        )
    }

    pub fn spawn_render_test_worker() -> (
        ServoWorker,
        Receiver<RecordedRenderCommand>,
        Sender<anyhow::Result<Canvas>>,
        Receiver<()>,
        Receiver<()>,
        Arc<AtomicBool>,
    ) {
        let (command_tx, command_rx) = mpsc::channel();
        let client_state = Arc::new(ServoWorkerClientSharedState::new());
        let (render_tx, render_rx) = mpsc::channel();
        let (result_tx, result_rx) = mpsc::channel();
        let (delivered_tx, delivered_rx) = mpsc::channel();
        let (unload_tx, unload_rx) = mpsc::channel();
        let stopped = Arc::new(AtomicBool::new(false));
        let stopped_clone = Arc::clone(&stopped);
        let thread_handle = thread::spawn(move || {
            while let Ok(command) = command_rx.recv() {
                match command {
                    WorkerCommand::CreateSession { response_tx, .. } => {
                        let _ = response_tx.send(Ok(()));
                    }
                    WorkerCommand::Render {
                        scripts,
                        width,
                        height,
                        response_tx,
                        ..
                    } => {
                        let _ = render_tx.send(RecordedRenderCommand {
                            scripts,
                            width,
                            height,
                        });
                        let result = result_rx
                            .recv()
                            .unwrap_or_else(|_| Ok(solid_canvas(12, 34, 56)));
                        let _ = response_tx.send(result);
                        let _ = delivered_tx.send(());
                    }
                    WorkerCommand::Unload { response_tx, .. } => {
                        let _ = unload_tx.send(());
                        let _ = response_tx.send(Ok(()));
                    }
                    WorkerCommand::DestroySession { response_tx, .. } => {
                        let _ = unload_tx.send(());
                        let _ = response_tx.send(Ok(()));
                    }
                    WorkerCommand::Shutdown { response_tx } => {
                        stopped_clone.store(true, Ordering::SeqCst);
                        let _ = response_tx.send(());
                        break;
                    }
                    WorkerCommand::Load { response_tx, .. } => {
                        let _ = response_tx.send(Ok(()));
                    }
                    WorkerCommand::LoadUrl { response_tx, .. } => {
                        let _ = response_tx.send(Ok(()));
                    }
                }
            }
        });

        (
            ServoWorker {
                command_tx: Some(command_tx),
                thread_handle: Some(thread_handle),
                client_state,
            },
            render_rx,
            result_tx,
            delivered_rx,
            unload_rx,
            stopped,
        )
    }

    pub fn spawn_load_test_worker() -> (
        ServoWorker,
        Receiver<RecordedLoadCommand>,
        Receiver<()>,
        Arc<AtomicBool>,
    ) {
        let (command_tx, command_rx) = mpsc::channel();
        let client_state = Arc::new(ServoWorkerClientSharedState::new());
        let (load_tx, load_rx) = mpsc::channel();
        let (unload_tx, unload_rx) = mpsc::channel();
        let stopped = Arc::new(AtomicBool::new(false));
        let stopped_clone = Arc::clone(&stopped);
        let thread_handle = thread::spawn(move || {
            while let Ok(command) = command_rx.recv() {
                match command {
                    WorkerCommand::CreateSession {
                        width,
                        height,
                        response_tx,
                        ..
                    } => {
                        let _ = load_tx.send(RecordedLoadCommand { width, height });
                        let _ = response_tx.send(Ok(()));
                    }
                    WorkerCommand::Load { response_tx, .. } => {
                        let _ = response_tx.send(Ok(()));
                    }
                    WorkerCommand::LoadUrl { response_tx, .. } => {
                        let _ = response_tx.send(Ok(()));
                    }
                    WorkerCommand::Unload { response_tx, .. }
                    | WorkerCommand::DestroySession { response_tx, .. } => {
                        let _ = unload_tx.send(());
                        let _ = response_tx.send(Ok(()));
                    }
                    WorkerCommand::Render { response_tx, .. } => {
                        let _ = response_tx.send(Ok(solid_canvas(12, 34, 56)));
                    }
                    WorkerCommand::Shutdown { response_tx } => {
                        stopped_clone.store(true, Ordering::SeqCst);
                        let _ = response_tx.send(());
                        break;
                    }
                }
            }
        });

        (
            ServoWorker {
                command_tx: Some(command_tx),
                thread_handle: Some(thread_handle),
                client_state,
            },
            load_rx,
            unload_rx,
            stopped,
        )
    }

    pub fn worker_client_from(worker: &ServoWorker) -> ServoWorkerClient {
        worker.client().expect("test worker client")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::Ordering;

    #[test]
    fn control_preamble_assigns_all_defaults() {
        let mut controls = HashMap::new();
        controls.insert("speed".to_owned(), ControlValue::Float(42.0));
        controls.insert("enabled".to_owned(), ControlValue::Boolean(true));
        controls.insert("color".to_owned(), ControlValue::Text("#00ffaa".to_owned()));

        let script = build_control_preamble_script(&controls);

        assert!(script.contains("globalThis[\"speed\"] = 42"));
        assert!(script.contains("globalThis[\"enabled\"] = true"));
        assert!(script.contains("globalThis[\"color\"] = \"#00ffaa\""));
        assert!(script.contains("window.__hypercolorCaptureMode = true"));
        assert!(script.contains("window.__hypercolorPreserveDrawingBuffer = true"));
        assert!(script.contains("globalThis.__hypercolorCaptureMode = true"));
        assert!(script.contains("globalThis.__hypercolorPreserveDrawingBuffer = true"));
    }

    #[test]
    fn inject_runtime_block_prefers_head_tag() {
        let html = "<html><head><title>x</title></head><body><script>run()</script></body></html>";
        let block = "<script>bootstrap()</script>\n";

        let injected = inject_runtime_head_block(html, block);
        let expected = "<html><head>\n<script>bootstrap()</script>\n<title>x</title></head>";
        assert!(injected.contains(expected));
    }

    #[test]
    fn inject_runtime_block_falls_back_to_first_script() {
        let html = "<body><script>run()</script></body>";
        let block = "<script>bootstrap()</script>\n";

        let injected = inject_runtime_head_block(html, block);
        assert!(injected.starts_with("<body>\n<script>bootstrap()</script>"));
    }

    #[test]
    fn prepare_runtime_html_source_injects_capture_flags_without_controls() {
        let temp = tempfile::tempdir().expect("tempdir should create");
        let html_path = temp.path().join("effect.html");
        std::fs::write(
            &html_path,
            "<html><head><title>x</title></head><body><script>run()</script></body></html>",
        )
        .expect("html write should work");

        let controls = HashMap::new();
        let (runtime_path, runtime_html_path) =
            prepare_runtime_html_source(&html_path, &controls).expect("runtime html should build");

        assert_ne!(runtime_path, html_path);
        assert_eq!(runtime_html_path.as_deref(), Some(runtime_path.as_path()));

        let runtime_html =
            std::fs::read_to_string(&runtime_path).expect("runtime html should be readable");
        assert!(runtime_html.contains("window.__hypercolorCaptureMode = true"));
        assert!(runtime_html.contains("window.__hypercolorPreserveDrawingBuffer = true"));
    }

    #[test]
    fn effect_is_audio_reactive_for_audio_category() {
        use hypercolor_types::effect::{EffectId, EffectSource};
        use uuid::Uuid;
        let metadata = EffectMetadata {
            id: EffectId::from(Uuid::nil()),
            name: "Audio".to_owned(),
            author: "hypercolor".to_owned(),
            version: "0.1.0".to_owned(),
            description: "Audio reactive".to_owned(),
            category: EffectCategory::Audio,
            tags: Vec::new(),
            controls: Vec::new(),
            presets: Vec::new(),
            audio_reactive: true,
            screen_reactive: false,
            source: EffectSource::Html {
                path: PathBuf::from("effects/audio.html"),
            },
            license: None,
        };

        assert!(effect_is_audio_reactive(&metadata));
    }

    #[test]
    fn effect_is_audio_reactive_for_audio_tags() {
        use hypercolor_types::effect::{EffectId, EffectSource};
        use uuid::Uuid;
        let metadata = EffectMetadata {
            id: EffectId::from(Uuid::nil()),
            name: "Ambient Audio".to_owned(),
            author: "hypercolor".to_owned(),
            version: "0.1.0".to_owned(),
            description: "Ambient effect with audio response".to_owned(),
            category: EffectCategory::Ambient,
            tags: vec!["visual".to_owned(), "audio-reactive".to_owned()],
            controls: Vec::new(),
            presets: Vec::new(),
            audio_reactive: false,
            screen_reactive: false,
            source: EffectSource::Html {
                path: PathBuf::from("effects/ambient-audio.html"),
            },
            license: None,
        };

        assert!(effect_is_audio_reactive(&metadata));
    }

    #[test]
    fn effect_is_not_audio_reactive_without_audio_signals() {
        use hypercolor_types::effect::{EffectId, EffectSource};
        use uuid::Uuid;
        let metadata = EffectMetadata {
            id: EffectId::from(Uuid::nil()),
            name: "Electric Colors".to_owned(),
            author: "hypercolor".to_owned(),
            version: "0.1.0".to_owned(),
            description: "Ambient effect".to_owned(),
            category: EffectCategory::Ambient,
            tags: vec!["ambient".to_owned(), "canvas2d".to_owned()],
            controls: Vec::new(),
            presets: Vec::new(),
            audio_reactive: false,
            screen_reactive: false,
            source: EffectSource::Html {
                path: PathBuf::from("effects/electric-colors.html"),
            },
            license: None,
        };

        assert!(!effect_is_audio_reactive(&metadata));
    }

    #[test]
    fn cached_canvas_reuse_requires_empty_script_batch() {
        let cached = Canvas::new(320, 200);

        assert!(can_reuse_cached_canvas(false, 0, Some(&cached), 320, 200));
        assert!(!can_reuse_cached_canvas(false, 1, Some(&cached), 320, 200));
        assert!(!can_reuse_cached_canvas(true, 0, Some(&cached), 320, 200));
    }

    #[test]
    fn cached_canvas_reuse_requires_matching_dimensions() {
        let cached = Canvas::new(320, 200);

        assert!(!can_reuse_cached_canvas(false, 0, Some(&cached), 640, 360));
        assert!(!can_reuse_cached_canvas(false, 0, None, 320, 200));
    }

    #[test]
    fn extracts_line_number_from_quoted_file_url_tuple_pattern() {
        let message = r#"Initialization failed (new TypeError("Ge is not a function", "file:///tmp/effects/custom/", 10585))"#;
        let token = extract_file_url_token(message).expect("file URL token");
        assert_eq!(extract_line_from_file_url_token(&token), None);
        assert_eq!(
            extract_line_after_file_url_token(message, Some(&token)),
            Some(10585)
        );
    }

    #[test]
    fn parses_location_with_fallback_when_console_url_is_directory() {
        let fallback = Path::new("/tmp/runtime-effect.html");
        let location = parse_console_source_location(
            r#"Failed to initialize: TypeError("boom", "file:///tmp/effects/custom/", 42)"#,
            Some(fallback),
        )
        .expect("source location");

        assert_eq!(location.path, fallback);
        assert_eq!(location.line_number, 42);
    }

    #[test]
    fn source_snippet_formats_context_window() {
        let temp_dir = tempfile::tempdir().expect("temporary directory");
        let source_path = temp_dir.path().join("effect.js");
        std::fs::write(&source_path, "line1\nline2\nline3\nline4\nline5\n")
            .expect("write source file");

        let snippet = load_source_snippet(&source_path, 3, 1).expect("snippet");
        assert!(snippet.contains("-2: line2"));
        assert!(snippet.contains(">3: line3"));
        assert!(snippet.contains("-4: line4"));
    }

    #[test]
    fn trimmed_servo_preferences_use_transparent_shell_background() {
        assert_eq!(
            trimmed_servo_preferences().shell_background_color_rgba,
            [0.0, 0.0, 0.0, 0.0]
        );
    }

    #[test]
    fn flip_rows_in_place_inverts_row_order() {
        // 3 rows × 2 pixels × 4 bytes = 24 bytes. Row 0 is RRGG.., row 1
        // is ..BBWW.., row 2 is YYCC.. — after a flip, row 0 is expected
        // to carry row 2's bytes and vice versa, row 1 unchanged.
        let mut pixels = vec![
            0x11, 0x11, 0x11, 0xff, 0x22, 0x22, 0x22, 0xff, // row 0
            0x33, 0x33, 0x33, 0xff, 0x44, 0x44, 0x44, 0xff, // row 1
            0x55, 0x55, 0x55, 0xff, 0x66, 0x66, 0x66, 0xff, // row 2
        ];
        flip_rows_in_place(&mut pixels, 8);
        assert_eq!(
            pixels,
            vec![
                0x55, 0x55, 0x55, 0xff, 0x66, 0x66, 0x66, 0xff, 0x33, 0x33, 0x33, 0xff, 0x44, 0x44,
                0x44, 0xff, 0x11, 0x11, 0x11, 0xff, 0x22, 0x22, 0x22, 0xff,
            ]
        );
    }

    #[test]
    fn flip_rows_in_place_handles_even_row_count() {
        let mut pixels = vec![
            0xaa, 0xaa, 0xaa, 0xff, // row 0
            0xbb, 0xbb, 0xbb, 0xff, // row 1
            0xcc, 0xcc, 0xcc, 0xff, // row 2
            0xdd, 0xdd, 0xdd, 0xff, // row 3
        ];
        flip_rows_in_place(&mut pixels, 4);
        assert_eq!(
            pixels,
            vec![
                0xdd, 0xdd, 0xdd, 0xff, 0xcc, 0xcc, 0xcc, 0xff, 0xbb, 0xbb, 0xbb, 0xff, 0xaa, 0xaa,
                0xaa, 0xff,
            ]
        );
    }

    #[test]
    fn flip_rows_in_place_is_a_noop_for_degenerate_buffers() {
        let mut single_row = vec![0x01, 0x02, 0x03, 0xff];
        flip_rows_in_place(&mut single_row, 4);
        assert_eq!(single_row, vec![0x01, 0x02, 0x03, 0xff]);

        let mut empty: Vec<u8> = Vec::new();
        flip_rows_in_place(&mut empty, 4);
        assert!(empty.is_empty());

        // A zero stride can't meaningfully flip — guard against division
        // by zero rather than panicking.
        let mut pixels = vec![0u8; 16];
        flip_rows_in_place(&mut pixels, 0);
        assert_eq!(pixels, vec![0u8; 16]);
    }

    #[test]
    fn format_console_message_includes_source_context() {
        let temp_dir = tempfile::tempdir().expect("temporary directory");
        let source_path = temp_dir.path().join("effect.js");
        std::fs::write(&source_path, "alpha\nbeta\ngamma\ndelta\n").expect("write source");

        let entry = ConsoleMessage {
            level: "error".to_owned(),
            message: r#"TypeError("boom", "file:///tmp/effects/custom/", 3)"#.to_owned(),
        };
        let formatted = format_console_message(&entry, Some(&source_path));
        assert!(formatted.contains("error: TypeError(\"boom\""));
        assert!(formatted.contains("effect.js:3"));
        assert!(formatted.contains(">3: gamma"));
    }

    #[test]
    fn servo_worker_shutdown_joins_thread() {
        let (mut worker, stopped) = test_support::spawn_test_worker();

        worker.shutdown().expect("worker shutdown should succeed");

        assert!(stopped.load(Ordering::SeqCst));
        assert!(worker.command_tx.is_none());
        assert!(worker.thread_handle.is_none());
    }

    #[test]
    fn poisoned_shared_worker_requires_daemon_restart() {
        let _lock = test_support::SHARED_WORKER_STATE_TEST_LOCK
            .lock()
            .expect("shared worker test lock");
        reset_shared_servo_worker_state();

        install_poisoned_shared_worker("test failure");

        let result = acquire_servo_worker();
        assert!(result.is_err(), "poisoned worker should fail closed");
        let error = result.err().expect("poisoned worker should fail closed");
        assert!(
            error
                .to_string()
                .contains("Servo runtime is unrecoverable until the daemon restarts")
        );

        reset_shared_servo_worker_state();
    }

    #[test]
    fn shutdown_clears_poisoned_shared_worker_state() {
        let _lock = test_support::SHARED_WORKER_STATE_TEST_LOCK
            .lock()
            .expect("shared worker test lock");
        reset_shared_servo_worker_state();

        install_poisoned_shared_worker("test failure");

        shutdown_shared_servo_worker().expect("shutdown should clear poisoned state");

        assert!(shared_worker_is_vacant());
    }

    #[test]
    fn load_completion_url_matches_exact_expected_url() {
        assert!(load_completion_url_matches(
            Some("https://example.com"),
            Some("https://example.com")
        ));
    }

    #[test]
    fn load_completion_url_matches_redirected_url() {
        assert!(load_completion_url_matches(
            Some("https://example.com"),
            Some("https://www.example.com/en")
        ));
    }

    #[test]
    fn load_completion_url_rejects_blank_page() {
        assert!(!load_completion_url_matches(
            Some("https://example.com"),
            Some("about:blank")
        ));
        assert!(!load_completion_url_matches(
            Some("https://example.com"),
            None
        ));
    }
}
