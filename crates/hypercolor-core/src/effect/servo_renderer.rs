//! Servo-backed HTML effect renderer (feature-gated).
//!
//! This renderer runs Servo on a dedicated worker thread so the public
//! `EffectRenderer` remains `Send` while Servo internals stay on one thread.

use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc::{self, Receiver, Sender, SyncSender};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use dpi::PhysicalSize;
use hypercolor_types::canvas::{Canvas, Rgba};
use hypercolor_types::effect::{ControlValue, EffectCategory, EffectMetadata, EffectSource};
use reqwest::Url;
use servo::{
    DeviceIntPoint, DeviceIntRect, JSValue, JavaScriptEvaluationError, LoadStatus, Preferences,
    RenderingContext, Servo, ServoBuilder, WebView, WebViewBuilder,
};
use tracing::{debug, info, trace, warn};

use super::bootstrap_software_rendering_context;
use super::lightscript::LightscriptRuntime;
use super::paths::resolve_html_source_path;
use super::{
    ConsoleMessage, EffectRenderer, FrameInput, HtmlControlKind, HypercolorWebViewDelegate,
    parse_html_effect_metadata,
};

const DEFAULT_WIDTH: u32 = 320;
const DEFAULT_HEIGHT: u32 = 200;
const LOAD_TIMEOUT: Duration = Duration::from_secs(5);
const SCRIPT_TIMEOUT: Duration = Duration::from_millis(250);
const WORKER_READY_TIMEOUT: Duration = Duration::from_secs(10);
const RENDER_RESPONSE_TIMEOUT: Duration = Duration::from_millis(500);
const RECENT_CONSOLE_SAMPLE_SIZE: usize = 6;
const CONSOLE_SNIPPET_RADIUS: usize = 1;
const CONSOLE_SNIPPET_LINE_MAX_CHARS: usize = 180;
const JS_TIMER_MIN_DURATION_MS: i64 = 4;

static SERVO_WORKER: OnceLock<Mutex<Option<Arc<ServoWorker>>>> = OnceLock::new();

/// Feature-gated renderer for HTML effects.
pub struct ServoRenderer {
    html_source: Option<PathBuf>,
    html_resolved_path: Option<PathBuf>,
    runtime_html_path: Option<PathBuf>,
    controls: HashMap<String, ControlValue>,
    runtime: LightscriptRuntime,
    initialized: bool,
    pending_scripts: Vec<String>,
    worker: Option<Arc<ServoWorker>>,
    warned_fallback_frame: bool,
    include_audio_updates: bool,
}

impl ServoRenderer {
    /// Create a new Servo renderer instance.
    #[must_use]
    pub fn new() -> Self {
        Self {
            html_source: None,
            html_resolved_path: None,
            runtime_html_path: None,
            controls: HashMap::new(),
            runtime: LightscriptRuntime::new(DEFAULT_WIDTH, DEFAULT_HEIGHT),
            initialized: false,
            pending_scripts: Vec::new(),
            worker: None,
            warned_fallback_frame: false,
            include_audio_updates: true,
        }
    }

    fn enqueue_bootstrap_scripts(&mut self) {
        self.pending_scripts.push(self.runtime.bootstrap_script());
    }

    fn enqueue_frame_scripts(&mut self, input: &FrameInput) {
        if let Some(script) = self
            .runtime
            .resize_script(input.canvas_width, input.canvas_height)
        {
            self.pending_scripts.push(script);
        }
        let frame_scripts =
            self.runtime
                .frame_scripts(&input.audio, &self.controls, self.include_audio_updates);
        self.pending_scripts.extend(frame_scripts.control_updates);
        if let Some(audio_update) = frame_scripts.audio_update {
            self.pending_scripts.push(audio_update);
        }
    }

    fn placeholder_canvas(input: &FrameInput) -> Canvas {
        let mut canvas = Canvas::new(input.canvas_width, input.canvas_height);
        let frame_mod = u8::try_from(input.frame_number % u64::from(u8::MAX)).unwrap_or_default();
        #[allow(
            clippy::as_conversions,
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss
        )]
        let audio_mod = (input.audio.rms_level.clamp(0.0, 1.0) * f32::from(u8::MAX)) as u8;

        let color = Rgba::new(frame_mod, audio_mod, frame_mod.saturating_add(32), 255);
        canvas.fill(color);
        canvas
    }

    fn cleanup_runtime_html(&mut self) {
        if let Some(path) = self.runtime_html_path.take() {
            if let Err(error) = std::fs::remove_file(&path) {
                debug!(
                    path = %path.display(),
                    %error,
                    "Failed to remove temporary runtime HTML source"
                );
            }
        }
    }
}

impl EffectRenderer for ServoRenderer {
    fn init(&mut self, metadata: &EffectMetadata) -> Result<()> {
        let EffectSource::Html { path } = &metadata.source else {
            bail!(
                "ServoRenderer requires EffectSource::Html, got source {:?} for effect '{}'",
                metadata.source,
                metadata.name
            );
        };

        let resolved = resolve_html_source_path(path).with_context(|| {
            format!(
                "failed to resolve HTML source for effect '{}' from '{}'",
                metadata.name,
                path.display()
            )
        })?;

        self.cleanup_runtime_html();
        self.worker = None;
        self.controls.clear();
        self.runtime = LightscriptRuntime::new(DEFAULT_WIDTH, DEFAULT_HEIGHT);
        self.pending_scripts.clear();
        self.warned_fallback_frame = false;
        self.include_audio_updates = effect_is_audio_reactive(metadata);

        match load_default_controls(&resolved) {
            Ok(default_controls) => {
                debug!(
                    effect = %metadata.name,
                    control_count = default_controls.len(),
                    controls = ?default_controls.keys().collect::<Vec<_>>(),
                    "Loaded HTML default controls"
                );
                self.controls = default_controls;
            }
            Err(error) => {
                warn!(
                    path = %resolved.display(),
                    %error,
                    "Failed to pre-seed HTML control defaults"
                );
            }
        }

        let (runtime_source, runtime_html_path) =
            prepare_runtime_html_source(&resolved, &self.controls).with_context(|| {
                format!(
                    "failed to prepare runtime HTML source for '{}'",
                    resolved.display()
                )
            })?;

        let worker = acquire_servo_worker(DEFAULT_WIDTH, DEFAULT_HEIGHT)?;
        worker.load_effect(&runtime_source)?;
        self.worker = Some(worker);
        self.html_source = Some(path.clone());
        self.html_resolved_path = Some(runtime_source.clone());
        self.runtime_html_path = runtime_html_path;
        self.initialized = true;
        self.enqueue_bootstrap_scripts();

        info!(
            effect = %metadata.name,
            source = %path.display(),
            resolved = %runtime_source.display(),
            "Initialized ServoRenderer worker"
        );

        Ok(())
    }

    fn tick(&mut self, input: &FrameInput) -> Result<Canvas> {
        if !self.initialized {
            bail!("ServoRenderer tick called before init");
        }

        self.enqueue_frame_scripts(input);
        let scripts = std::mem::take(&mut self.pending_scripts);

        let Some(worker) = self.worker.as_ref() else {
            return Ok(Self::placeholder_canvas(input));
        };

        match worker.render_frame(scripts, input.canvas_width, input.canvas_height) {
            Ok(canvas) => Ok(canvas),
            Err(error) => {
                warn!(%error, "Servo frame render failed");
                if !self.warned_fallback_frame {
                    warn!("Falling back to placeholder frame for this effect");
                    self.warned_fallback_frame = true;
                }
                Ok(Self::placeholder_canvas(input))
            }
        }
    }

    fn set_control(&mut self, name: &str, value: &ControlValue) {
        self.controls.insert(name.to_owned(), value.clone());
    }

    fn destroy(&mut self) {
        self.worker = None;
        self.pending_scripts.clear();
        self.controls.clear();
        self.html_source = None;
        self.html_resolved_path = None;
        self.cleanup_runtime_html();
        self.initialized = false;
        self.warned_fallback_frame = false;
        self.include_audio_updates = true;
    }
}

impl Default for ServoRenderer {
    fn default() -> Self {
        Self::new()
    }
}

/// Worker wrapper that owns the Servo runtime thread.
struct ServoWorker {
    command_tx: Sender<WorkerCommand>,
}

impl ServoWorker {
    fn spawn(width: u32, height: u32) -> Result<Self> {
        let (command_tx, command_rx) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);

        thread::Builder::new()
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

        Ok(Self { command_tx })
    }

    fn load_effect(&self, html_path: &Path) -> Result<()> {
        let (response_tx, response_rx) = mpsc::sync_channel(1);
        self.command_tx
            .send(WorkerCommand::Load {
                html_path: html_path.to_path_buf(),
                response_tx,
            })
            .context("failed to send load command to Servo worker")?;

        match response_rx.recv_timeout(WORKER_READY_TIMEOUT) {
            Ok(result) => result,
            Err(mpsc::RecvTimeoutError::Timeout) => bail!(
                "timed out waiting for Servo page load after {}ms",
                WORKER_READY_TIMEOUT.as_millis()
            ),
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                bail!("Servo worker disconnected before confirming page load")
            }
        }
    }

    fn render_frame(&self, scripts: Vec<String>, width: u32, height: u32) -> Result<Canvas> {
        let (response_tx, response_rx) = mpsc::sync_channel(1);
        self.command_tx
            .send(WorkerCommand::Render {
                scripts,
                width,
                height,
                response_tx,
            })
            .context("failed to send render command to Servo worker")?;
        match response_rx.recv_timeout(RENDER_RESPONSE_TIMEOUT) {
            Ok(result) => result,
            Err(mpsc::RecvTimeoutError::Timeout) => bail!(
                "timed out waiting for Servo frame response after {}ms",
                RENDER_RESPONSE_TIMEOUT.as_millis()
            ),
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                bail!("Servo worker disconnected before sending frame response")
            }
        }
    }
}

enum WorkerCommand {
    Load {
        html_path: PathBuf,
        response_tx: SyncSender<Result<()>>,
    },
    Render {
        scripts: Vec<String>,
        width: u32,
        height: u32,
        response_tx: SyncSender<Result<Canvas>>,
    },
}

struct ServoWorkerRuntime {
    servo: Servo,
    webview: WebView,
    rendering_context: Rc<dyn RenderingContext>,
    delegate: Rc<HypercolorWebViewDelegate>,
    loaded_html_path: Option<PathBuf>,
}

impl ServoWorkerRuntime {
    fn new(width: u32, height: u32) -> Result<Self> {
        install_rustls_provider();

        let rendering_context: Rc<dyn RenderingContext> =
            Rc::new(bootstrap_software_rendering_context(width, height)?);
        rendering_context.make_current().map_err(|error| {
            anyhow!("failed to make Servo rendering context current: {error:?}")
        })?;

        // Avoid one-second timer clamping in embedder-throttled mode.
        let preferences = Preferences {
            js_timers_minimum_duration: JS_TIMER_MIN_DURATION_MS,
            // Workshop effects are Three.js/WebGL + OffscreenCanvas heavy.
            // Servo defaults these off, which makes WebGL context creation fail
            // during effect initialization.
            dom_webgl2_enabled: true,
            dom_offscreen_canvas_enabled: true,
            ..Preferences::default()
        };

        let servo = ServoBuilder::default().preferences(preferences).build();
        let delegate = Rc::new(HypercolorWebViewDelegate::new());
        let url = Url::parse("about:blank").context("failed to parse about:blank URL")?;

        let webview = WebViewBuilder::new(&servo, Rc::clone(&rendering_context))
            .delegate(delegate.clone())
            .url(url)
            .build();

        let runtime = Self {
            servo,
            webview,
            rendering_context,
            delegate,
            loaded_html_path: None,
        };
        runtime.wait_for_load_completion(LOAD_TIMEOUT, None)?;
        Ok(runtime)
    }

    fn run(mut self, command_rx: Receiver<WorkerCommand>) {
        for command in command_rx {
            match command {
                WorkerCommand::Load {
                    html_path,
                    response_tx,
                } => {
                    let result = self.load_effect(&html_path);
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
            }
        }
    }

    fn load_effect(&mut self, html_path: &Path) -> Result<()> {
        let url = file_url_for_path(html_path)?;
        self.loaded_html_path = Some(html_path.to_path_buf());
        self.delegate.take_page_loaded();
        debug!(url = %url, "Loading Servo effect page");
        self.webview.load(url.clone());
        self.wait_for_load_completion(LOAD_TIMEOUT, Some(url.as_str()))?;
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

    fn render_frame(&mut self, scripts: &[String], width: u32, height: u32) -> Result<Canvas> {
        (|| {
            self.resize_if_needed(width, height);

            for script in scripts {
                let preview = script_preview(script);
                self.evaluate_script(script)
                    .with_context(|| format!("failed to evaluate script: {preview}"))?;
            }

            // Let timers/RAF advance for one daemon-driven frame after scripts
            // have injected controls/audio for this tick.
            self.servo.spin_event_loop();
            let frame_ready = self.delegate.take_frame_ready();
            if frame_ready {
                trace!("Servo delegate signaled new frame");
            }
            self.webview.paint();

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
        })()
    }

    fn resize_if_needed(&self, width: u32, height: u32) {
        let new_size = PhysicalSize::new(width, height);
        if self.rendering_context.size() == new_size {
            return;
        }

        self.rendering_context.resize(new_size);
        self.webview.resize(new_size);
    }

    fn evaluate_script(&mut self, script: &str) -> Result<()> {
        let result_slot: Rc<RefCell<Option<Result<JSValue, JavaScriptEvaluationError>>>> =
            Rc::new(RefCell::new(None));
        let callback_slot = Rc::clone(&result_slot);

        self.webview.evaluate_javascript(script, move |result| {
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
            let status_complete = matches!(self.webview.load_status(), LoadStatus::Complete);
            let loaded = self.delegate.is_page_loaded() || status_complete;
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

fn summarize_console_messages(
    messages: &[ConsoleMessage],
    fallback_source: Option<&Path>,
) -> Vec<String> {
    messages
        .iter()
        .map(|entry| format_console_message(entry, fallback_source))
        .collect()
}

fn format_console_message(entry: &ConsoleMessage, fallback_source: Option<&Path>) -> String {
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

fn find_initialization_failure_message(messages: &[ConsoleMessage]) -> Option<&ConsoleMessage> {
    messages.iter().rev().find(|entry| {
        let is_error_level = matches!(entry.level.as_str(), "error" | "warn");
        let lower = entry.message.to_ascii_lowercase();
        is_error_level
            && (lower.contains("initialization failed") || lower.contains("failed to initialize"))
    })
}

fn script_preview(script: &str) -> String {
    let single_line = script.split_whitespace().collect::<Vec<_>>().join(" ");
    truncate_for_log(&single_line, 120)
}

fn acquire_servo_worker(width: u32, height: u32) -> Result<Arc<ServoWorker>> {
    let slot = SERVO_WORKER.get_or_init(|| Mutex::new(None));
    let mut guard = match slot.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };

    if let Some(worker) = guard.as_ref() {
        return Ok(Arc::clone(worker));
    }

    let worker = Arc::new(ServoWorker::spawn(width, height)?);
    *guard = Some(Arc::clone(&worker));
    Ok(worker)
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

fn prepare_runtime_html_source(
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

fn effect_is_audio_reactive(metadata: &EffectMetadata) -> bool {
    if matches!(metadata.category, EffectCategory::Audio) {
        return true;
    }

    metadata
        .tags
        .iter()
        .any(|tag| tag.eq_ignore_ascii_case("audio") || tag.eq_ignore_ascii_case("audio-reactive"))
}

fn load_default_controls(path: &Path) -> Result<HashMap<String, ControlValue>> {
    let html = std::fs::read_to_string(path).with_context(|| {
        format!(
            "failed to read HTML effect file while extracting defaults: {}",
            path.display()
        )
    })?;
    let parsed = parse_html_effect_metadata(&html);
    let controls = parsed
        .controls
        .into_iter()
        .filter_map(|control| {
            default_control_value(&control.kind, control.default.as_deref(), &control.values)
                .map(|value| (control.property, value))
        })
        .collect();
    Ok(controls)
}

fn default_control_value(
    kind: &HtmlControlKind,
    default: Option<&str>,
    values: &[String],
) -> Option<ControlValue> {
    match kind {
        HtmlControlKind::Number | HtmlControlKind::Hue | HtmlControlKind::Area => default
            .and_then(|value| value.trim().parse::<f32>().ok())
            .map(ControlValue::Float),
        HtmlControlKind::Boolean => {
            default.map(|value| ControlValue::Boolean(parse_bool_default(value)))
        }
        HtmlControlKind::Combobox => default
            .or_else(|| values.first().map(String::as_str))
            .map(|value| ControlValue::Enum(value.to_owned())),
        HtmlControlKind::Color
        | HtmlControlKind::Sensor
        | HtmlControlKind::Text
        | HtmlControlKind::Other(_) => default.map(|value| ControlValue::Text(value.to_owned())),
    }
}

fn parse_bool_default(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use hypercolor_types::effect::EffectId;
    use uuid::Uuid;

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
        let metadata = EffectMetadata {
            id: EffectId::from(Uuid::nil()),
            name: "Audio".to_owned(),
            author: "hypercolor".to_owned(),
            version: "0.1.0".to_owned(),
            description: "Audio reactive".to_owned(),
            category: EffectCategory::Audio,
            tags: Vec::new(),
            source: EffectSource::Html {
                path: PathBuf::from("effects/audio.html"),
            },
            license: None,
        };

        assert!(effect_is_audio_reactive(&metadata));
    }

    #[test]
    fn effect_is_audio_reactive_for_audio_tags() {
        let metadata = EffectMetadata {
            id: EffectId::from(Uuid::nil()),
            name: "Ambient Audio".to_owned(),
            author: "hypercolor".to_owned(),
            version: "0.1.0".to_owned(),
            description: "Ambient effect with audio response".to_owned(),
            category: EffectCategory::Ambient,
            tags: vec!["visual".to_owned(), "audio-reactive".to_owned()],
            source: EffectSource::Html {
                path: PathBuf::from("effects/ambient-audio.html"),
            },
            license: None,
        };

        assert!(effect_is_audio_reactive(&metadata));
    }

    #[test]
    fn effect_is_not_audio_reactive_without_audio_signals() {
        let metadata = EffectMetadata {
            id: EffectId::from(Uuid::nil()),
            name: "Electric Colors".to_owned(),
            author: "hypercolor".to_owned(),
            version: "0.1.0".to_owned(),
            description: "Ambient effect".to_owned(),
            category: EffectCategory::Ambient,
            tags: vec!["ambient".to_owned(), "canvas2d".to_owned()],
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
}
