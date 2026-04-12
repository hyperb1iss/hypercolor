//! Shared Servo worker thread lifecycle and runtime.
//!
//! Servo initializes process-global options exactly once; recreating the
//! runtime after a shutdown panics inside libservo. Hypercolor therefore
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
    DeviceIntPoint, DeviceIntRect, JSValue, JavaScriptEvaluationError, Preferences,
    RenderingContext, Servo, ServoBuilder, WebView, WebViewBuilder,
};
use tracing::{debug, trace, warn};

use super::circuit_breaker::ServoCircuitBreaker;
use super::delegate::{ConsoleMessage, HypercolorWebViewDelegate};
use super::worker_client::{
    ServoWorkerClient, UNLOAD_TIMEOUT, WORKER_READY_TIMEOUT, WorkerCommand,
};
use crate::effect::servo_bootstrap::bootstrap_software_rendering_context;

pub(super) const LOAD_TIMEOUT: Duration = Duration::from_secs(5);
const SCRIPT_TIMEOUT: Duration = Duration::from_millis(250);
pub(super) const RENDER_RESPONSE_TIMEOUT: Duration = Duration::from_millis(500);
pub(super) const RECENT_CONSOLE_SAMPLE_SIZE: usize = 6;
const CONSOLE_SNIPPET_RADIUS: usize = 1;
const CONSOLE_SNIPPET_LINE_MAX_CHARS: usize = 180;
const JS_TIMER_MIN_DURATION_MS: i64 = 4;

// The shared worker. Servo can only exist once per process; this OnceLock
// keeps the single instance alive across effect switches for the entire
// daemon lifetime.
static SERVO_WORKER: OnceLock<Mutex<SharedServoWorkerState>> = OnceLock::new();
static SERVO_CIRCUIT_BREAKER: ServoCircuitBreaker = ServoCircuitBreaker::new();

/// Acquire a client handle to the shared Servo worker, spawning it on first use.
pub(super) fn acquire_servo_worker(width: u32, height: u32) -> Result<ServoWorkerClient> {
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

    match ServoWorker::spawn(width, height) {
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
    if controls.is_empty() {
        return Ok((original_path.to_path_buf(), None));
    }

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

    if let Some(head_start) = lowered.find("<head") {
        if let Some(head_close_offset) = lowered[head_start..].find('>') {
            let insert_at = head_start + head_close_offset + 1;
            let (before, after) = html.split_at(insert_at);
            return format!("{before}\n{block}{after}");
        }
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

enum SharedServoWorkerState {
    Vacant,
    Running(ServoWorker),
    Poisoned { reason: String },
}

/// Owner of the Servo worker thread and its command channel.
pub(super) struct ServoWorker {
    pub(super) command_tx: Option<Sender<WorkerCommand>>,
    pub(super) thread_handle: Option<thread::JoinHandle<()>>,
}

impl ServoWorker {
    fn spawn(width: u32, height: u32) -> Result<Self> {
        let (command_tx, command_rx) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);

        let thread_handle = thread::Builder::new()
            .name("hypercolor-servo-worker".to_owned())
            .spawn(move || {
                let runtime = match ServoWorkerRuntime::new(width, height) {
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
        })
    }

    fn client(&self) -> Result<ServoWorkerClient> {
        Ok(ServoWorkerClient::new(self.command_tx()?.clone()))
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

struct ServoWorkerRuntime {
    webview: Option<WebView>,
    servo: Servo,
    rendering_context: Rc<dyn RenderingContext>,
    delegate: Rc<HypercolorWebViewDelegate>,
    loaded_html_path: Option<PathBuf>,
    script_buffer: String,
}

impl ServoWorkerRuntime {
    fn new(width: u32, height: u32) -> Result<Self> {
        install_rustls_provider();

        let rendering_context: Rc<dyn RenderingContext> =
            Rc::new(bootstrap_software_rendering_context(width, height)?);
        rendering_context.make_current().map_err(|error| {
            anyhow!("failed to make Servo rendering context current: {error:?}")
        })?;

        let servo = ServoBuilder::default()
            .preferences(trimmed_servo_preferences())
            .build();
        let delegate = Rc::new(HypercolorWebViewDelegate::new());
        let url = Url::parse("about:blank").context("failed to parse about:blank URL")?;

        let webview = WebViewBuilder::new(&servo, Rc::clone(&rendering_context))
            .delegate(delegate.clone())
            .url(url)
            .build();

        let runtime = Self {
            webview: Some(webview),
            servo,
            rendering_context,
            delegate,
            loaded_html_path: None,
            script_buffer: String::new(),
        };
        runtime.wait_for_load_completion(LOAD_TIMEOUT, None)?;
        Ok(runtime)
    }

    fn run(mut self, command_rx: Receiver<WorkerCommand>) {
        for command in command_rx {
            match command {
                WorkerCommand::Load {
                    html_path,
                    width,
                    height,
                    response_tx,
                } => {
                    let result = self.load_effect(&html_path, width, height);
                    let _ = response_tx.send(result);
                }
                WorkerCommand::Unload { response_tx } => {
                    let result = self.unload_effect();
                    let _ = response_tx.send(result);
                }
                WorkerCommand::Render {
                    scripts,
                    width,
                    height,
                    response_tx,
                } => {
                    let result = self.render_frame(&scripts, width, height);
                    let _ = response_tx.send(result);
                }
                WorkerCommand::Shutdown { response_tx } => {
                    let _ = response_tx.send(());
                    break;
                }
            }
        }

        let Self {
            webview,
            servo,
            rendering_context,
            delegate,
            loaded_html_path,
            ..
        } = self;
        drop(loaded_html_path);
        drop(delegate);
        drop(webview);
        drop(rendering_context);
        drop(servo);
    }

    fn active_webview(&self) -> Result<&WebView> {
        self.webview
            .as_ref()
            .ok_or_else(|| anyhow!("Servo webview is not initialized"))
    }

    fn build_webview(&self, url: Url) -> WebView {
        let webview = WebViewBuilder::new(&self.servo, Rc::clone(&self.rendering_context))
            .delegate(self.delegate.clone())
            .url(url)
            .build();
        webview.set_throttled(true);
        webview
    }

    fn close_webview(&mut self) -> Result<()> {
        // Dropping the last WebView handle synchronously issues Servo's
        // `CloseWebView` message. `notify_closed` is only documented for
        // `window.close()`, so waiting on it here just turns every effect
        // switch into a timeout path.
        let Some(webview) = self.webview.take() else {
            return Ok(());
        };
        drop(webview);
        self.servo.spin_event_loop();
        Ok(())
    }

    fn replace_webview(&mut self, url: Url, timeout: Duration) -> Result<()> {
        // Dropping the last handle closes the old webview and lets Servo tear
        // down page-scoped resources before we build the next one.
        self.close_webview()
            .context("failed to close previous Servo webview")?;
        self.delegate.reset_navigation_state();
        self.webview = Some(self.build_webview(url.clone()));
        self.wait_for_load_completion(timeout, Some(url.as_str()))
    }

    fn load_effect(&mut self, html_path: &Path, width: u32, height: u32) -> Result<()> {
        let had_loaded_effect = self.loaded_html_path.is_some();
        self.resize_if_needed(width, height)?;
        let url = file_url_for_path(html_path)?;
        self.loaded_html_path = Some(html_path.to_path_buf());
        debug!(url = %url, "Loading Servo effect page");
        if had_loaded_effect {
            self.replace_webview(url.clone(), LOAD_TIMEOUT).context(
                "failed to replace previous Servo effect page before loading new effect",
            )?;
        } else {
            self.delegate.reset_navigation_state();
            let webview = self.active_webview()?;
            webview.load(url.clone());
            self.wait_for_load_completion(LOAD_TIMEOUT, Some(url.as_str()))?;
        }
        let recent_console_entries = self
            .delegate
            .recent_console_messages(RECENT_CONSOLE_SAMPLE_SIZE);
        let recent_console =
            summarize_console_messages(&recent_console_entries, self.loaded_html_path.as_deref());
        if !recent_console.is_empty() {
            debug!(
                url = %url,
                recent_console = ?recent_console,
                "Recent console output while loading Servo effect page"
            );
        }
        if let Some(message) = find_initialization_failure_message(&recent_console_entries) {
            bail!(
                "effect initialization failed: {}",
                format_console_message(message, self.loaded_html_path.as_deref())
            );
        }
        Ok(())
    }

    fn unload_effect(&mut self) -> Result<()> {
        if self.loaded_html_path.is_none() {
            return Ok(());
        }

        let url = Url::parse("about:blank").context("failed to parse about:blank URL")?;
        debug!("Unloading Servo effect page");
        self.replace_webview(url.clone(), UNLOAD_TIMEOUT)?;
        self.loaded_html_path = None;
        Ok(())
    }

    fn render_frame(&mut self, scripts: &[String], width: u32, height: u32) -> Result<Canvas> {
        let result = (|| {
            self.resize_if_needed(width, height)?;

            self.evaluate_scripts(scripts)?;

            let webview = self.active_webview()?;
            // Let timers/RAF advance for one daemon-driven frame after scripts
            // have injected controls/audio for this tick. Leaving the webview
            // unthrottled between ticks lets effect-side RAF/timer loops free-run.
            webview.set_throttled(false);
            self.servo.spin_event_loop();
            let frame_ready = self.delegate.take_frame_ready();
            if frame_ready {
                trace!("Servo delegate signaled new frame");
            }
            webview.paint();

            let size = self.rendering_context.size();
            let width_i32 =
                i32::try_from(size.width).context("canvas width overflow for Servo readback")?;
            let height_i32 =
                i32::try_from(size.height).context("canvas height overflow for Servo readback")?;
            let rect = DeviceIntRect::new(
                DeviceIntPoint::new(0, 0),
                DeviceIntPoint::new(width_i32, height_i32),
            );

            let image = self
                .rendering_context
                .read_to_image(rect)
                .ok_or_else(|| anyhow!("Servo returned no pixels for readback rectangle"))?;

            let image_width = image.width();
            let image_height = image.height();
            Ok(Canvas::from_vec(
                image.into_raw(),
                image_width,
                image_height,
            ))
        })();

        if let Some(webview) = self.webview.as_ref() {
            webview.set_throttled(true);
        }
        result
    }

    fn evaluate_scripts(&mut self, scripts: &[String]) -> Result<()> {
        if scripts.is_empty() {
            return Ok(());
        }

        let mut script_buffer = std::mem::take(&mut self.script_buffer);
        combined_script(&mut script_buffer, scripts);
        let result = self.evaluate_script(&script_buffer).with_context(|| {
            format!(
                "failed to evaluate script batch: {}",
                batched_script_preview(scripts)
            )
        });
        self.script_buffer = script_buffer;
        result
    }

    fn resize_if_needed(&self, width: u32, height: u32) -> Result<()> {
        let new_size = PhysicalSize::new(width, height);
        if self.rendering_context.size() == new_size {
            return Ok(());
        }

        self.rendering_context.resize(new_size);
        self.active_webview()?.resize(new_size);
        Ok(())
    }

    fn evaluate_script(&mut self, script: &str) -> Result<()> {
        let result_slot: Rc<RefCell<Option<Result<JSValue, JavaScriptEvaluationError>>>> =
            Rc::new(RefCell::new(None));
        let callback_slot = Rc::clone(&result_slot);

        self.active_webview()?
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
            let recent_console = summarize_console_messages(
                &self
                    .delegate
                    .recent_console_messages(RECENT_CONSOLE_SAMPLE_SIZE),
                self.loaded_html_path.as_deref(),
            );
            let mut message = format!("javascript evaluation failed: {error:?}");
            if !recent_console.is_empty() {
                let _ = write!(message, "; recent console: {}", recent_console.join(" | "));
            }
            anyhow!(message)
        })
    }

    fn wait_for_load_completion(
        &self,
        timeout: Duration,
        expected_url: Option<&str>,
    ) -> Result<()> {
        let deadline = Instant::now() + timeout;

        loop {
            self.servo.spin_event_loop();
            let loaded = self.delegate.is_page_loaded();
            let url_matches =
                expected_url.is_none_or(|url| self.delegate.last_url().as_deref() == Some(url));
            if loaded && url_matches {
                self.delegate.take_page_loaded();
                debug!("Servo page load completed");
                return Ok(());
            }

            if Instant::now() >= deadline {
                let recent_console = summarize_console_messages(
                    &self
                        .delegate
                        .recent_console_messages(RECENT_CONSOLE_SAMPLE_SIZE),
                    self.loaded_html_path.as_deref(),
                );
                let current_url = self
                    .delegate
                    .last_url()
                    .unwrap_or_else(|| "<unknown>".to_owned());
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

    use super::super::worker_client::{ServoWorkerClient, WorkerCommand};
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
        let stopped = Arc::new(AtomicBool::new(false));
        let stopped_clone = Arc::clone(&stopped);
        let thread_handle = thread::spawn(move || {
            while let Ok(command) = command_rx.recv() {
                match command {
                    WorkerCommand::Unload { response_tx } => {
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
        let (render_tx, render_rx) = mpsc::channel();
        let (result_tx, result_rx) = mpsc::channel();
        let (delivered_tx, delivered_rx) = mpsc::channel();
        let (unload_tx, unload_rx) = mpsc::channel();
        let stopped = Arc::new(AtomicBool::new(false));
        let stopped_clone = Arc::clone(&stopped);
        let thread_handle = thread::spawn(move || {
            while let Ok(command) = command_rx.recv() {
                match command {
                    WorkerCommand::Render {
                        scripts,
                        width,
                        height,
                        response_tx,
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
                    WorkerCommand::Unload { response_tx } => {
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
                }
            }
        });

        (
            ServoWorker {
                command_tx: Some(command_tx),
                thread_handle: Some(thread_handle),
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
        let (load_tx, load_rx) = mpsc::channel();
        let (unload_tx, unload_rx) = mpsc::channel();
        let stopped = Arc::new(AtomicBool::new(false));
        let stopped_clone = Arc::clone(&stopped);
        let thread_handle = thread::spawn(move || {
            while let Ok(command) = command_rx.recv() {
                match command {
                    WorkerCommand::Load {
                        width,
                        height,
                        response_tx,
                        ..
                    } => {
                        let _ = load_tx.send(RecordedLoadCommand { width, height });
                        let _ = response_tx.send(Ok(()));
                    }
                    WorkerCommand::Unload { response_tx } => {
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
    use hypercolor_types::canvas::{DEFAULT_CANVAS_HEIGHT, DEFAULT_CANVAS_WIDTH};
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

        let result = acquire_servo_worker(DEFAULT_CANVAS_WIDTH, DEFAULT_CANVAS_HEIGHT);
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
}
