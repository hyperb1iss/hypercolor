//! Servo-backed HTML effect renderer (feature-gated).
//!
//! This renderer runs Servo on a dedicated worker thread so the public
//! `EffectRenderer` remains `Send` while Servo internals stay on one thread.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc::{self, Receiver, Sender, SyncSender};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use dpi::PhysicalSize;
use hypercolor_types::canvas::{Canvas, Rgba};
use hypercolor_types::effect::{ControlValue, EffectMetadata, EffectSource};
use reqwest::Url;
use servo::{
    DeviceIntPoint, DeviceIntRect, JSValue, JavaScriptEvaluationError, LoadStatus,
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
const NO_FRAME_STREAK_THRESHOLD: u32 = 3;
const NO_FRAME_STREAK_LOG_INTERVAL: u32 = 120;
const RECENT_CONSOLE_SAMPLE_SIZE: usize = 6;
const ANIMATION_KICK_SCRIPT: &str = r"
(function(){
  const candidates = ['loop', 'update', 'render', 'animate', 'tick', 'draw', 'frame', 'main'];
  for (const name of candidates) {
    const fn = globalThis[name];
    if (typeof fn === 'function') {
      try { globalThis.requestAnimationFrame(fn); } catch (_err) {}
      break;
    }
  }
})();
";

static SERVO_WORKER: OnceLock<Mutex<Option<Arc<ServoWorker>>>> = OnceLock::new();

/// Feature-gated renderer for HTML effects.
pub struct ServoRenderer {
    html_source: Option<PathBuf>,
    html_resolved_path: Option<PathBuf>,
    controls: HashMap<String, ControlValue>,
    runtime: LightscriptRuntime,
    initialized: bool,
    pending_scripts: Vec<String>,
    worker: Option<Arc<ServoWorker>>,
    warned_fallback_frame: bool,
}

impl ServoRenderer {
    /// Create a new Servo renderer instance.
    #[must_use]
    pub fn new() -> Self {
        Self {
            html_source: None,
            html_resolved_path: None,
            controls: HashMap::new(),
            runtime: LightscriptRuntime::new(DEFAULT_WIDTH, DEFAULT_HEIGHT),
            initialized: false,
            pending_scripts: Vec::new(),
            worker: None,
            warned_fallback_frame: false,
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
        let frame_scripts = self.runtime.frame_scripts(&input.audio, &self.controls);
        self.pending_scripts.push(frame_scripts.audio_update);
        self.pending_scripts.extend(frame_scripts.control_updates);
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

        self.worker = None;
        self.controls.clear();
        self.runtime = LightscriptRuntime::new(DEFAULT_WIDTH, DEFAULT_HEIGHT);
        self.pending_scripts.clear();
        self.warned_fallback_frame = false;

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

        let worker = acquire_servo_worker(DEFAULT_WIDTH, DEFAULT_HEIGHT)?;
        worker.load_effect(&resolved)?;
        self.worker = Some(worker);
        self.html_source = Some(path.clone());
        self.html_resolved_path = Some(resolved.clone());
        self.initialized = true;
        self.enqueue_bootstrap_scripts();

        info!(
            effect = %metadata.name,
            source = %path.display(),
            resolved = %resolved.display(),
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
        self.initialized = false;
        self.warned_fallback_frame = false;
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
    pending_unthrottle: bool,
    no_frame_streak: u32,
    animation_kick_attempted: bool,
}

impl ServoWorkerRuntime {
    fn new(width: u32, height: u32) -> Result<Self> {
        install_rustls_provider();

        let rendering_context: Rc<dyn RenderingContext> =
            Rc::new(bootstrap_software_rendering_context(width, height)?);
        rendering_context.make_current().map_err(|error| {
            anyhow!("failed to make Servo rendering context current: {error:?}")
        })?;

        let servo = ServoBuilder::default().build();
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
            pending_unthrottle: false,
            no_frame_streak: 0,
            animation_kick_attempted: false,
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
        self.delegate.take_page_loaded();
        self.webview.set_throttled(true);
        debug!(url = %url, "Loading Servo effect page");
        self.webview.load(url.clone());
        self.wait_for_load_completion(LOAD_TIMEOUT, Some(url.as_str()))?;
        let recent_console = summarize_console_messages(
            self.delegate
                .recent_console_messages(RECENT_CONSOLE_SAMPLE_SIZE),
        );
        if !recent_console.is_empty() {
            debug!(
                url = %url,
                recent_console = ?recent_console,
                "Recent console output while loading Servo effect page"
            );
        }
        self.pending_unthrottle = true;
        self.no_frame_streak = 0;
        self.animation_kick_attempted = false;
        Ok(())
    }

    fn render_frame(&mut self, scripts: &[String], width: u32, height: u32) -> Result<Canvas> {
        self.resize_if_needed(width, height);

        for script in scripts {
            self.evaluate_script(script)
                .with_context(|| format!("failed to evaluate script: {script}"))?;
        }

        if self.pending_unthrottle {
            self.webview.set_throttled(false);
            self.pending_unthrottle = false;
        }

        self.servo.spin_event_loop();
        let frame_ready = self.delegate.take_frame_ready();
        if frame_ready {
            self.no_frame_streak = 0;
            trace!("Servo delegate signaled new frame");
        } else {
            self.no_frame_streak = self.no_frame_streak.saturating_add(1);
            if self.animation_kick_attempted
                && self.no_frame_streak >= NO_FRAME_STREAK_LOG_INTERVAL
                && self.no_frame_streak % NO_FRAME_STREAK_LOG_INTERVAL == 0
            {
                self.log_frame_stall("Servo effect is still not reporting new frames");
            }
        }

        if !self.animation_kick_attempted && self.no_frame_streak >= NO_FRAME_STREAK_THRESHOLD {
            self.log_frame_stall("Servo effect has not reported frames; attempting animation kick");
            if let Err(error) = self.evaluate_script(ANIMATION_KICK_SCRIPT) {
                warn!(%error, "Failed to evaluate animation kick script");
            }
            self.animation_kick_attempted = true;
            self.servo.spin_event_loop();
            if self.delegate.take_frame_ready() {
                self.no_frame_streak = 0;
                info!("Animation kick restored Servo frame delivery");
            } else {
                self.log_frame_stall("Animation kick did not restore frame delivery");
            }
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

        Ok(Canvas::from_rgba(
            image.as_raw(),
            image.width(),
            image.height(),
        ))
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
        result
            .map(|_| ())
            .map_err(|error| anyhow!("javascript evaluation failed: {error:?}"))
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
                bail!("timed out waiting for Servo page load completion");
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    fn log_frame_stall(&self, message: &'static str) {
        let recent_console = summarize_console_messages(
            self.delegate
                .recent_console_messages(RECENT_CONSOLE_SAMPLE_SIZE),
        );
        let current_url = self.delegate.last_url();
        if recent_console.is_empty() {
            warn!(
                streak = self.no_frame_streak,
                url = ?current_url,
                "{message}"
            );
        } else {
            warn!(
                streak = self.no_frame_streak,
                url = ?current_url,
                recent_console = ?recent_console,
                "{message}"
            );
        }
    }
}

fn summarize_console_messages(messages: Vec<ConsoleMessage>) -> Vec<String> {
    messages
        .into_iter()
        .map(|entry| format!("{}: {}", entry.level, entry.message))
        .collect()
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
