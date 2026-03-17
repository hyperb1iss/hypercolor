//! Servo-backed HTML effect renderer (feature-gated).
//!
//! This renderer runs Servo on a dedicated worker thread so the public
//! `EffectRenderer` remains `Send` while Servo internals stay on one thread.

use std::any::Any;
use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc::{self, Receiver, Sender, SyncSender, TryRecvError};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use dpi::PhysicalSize;
use hypercolor_types::canvas::{Canvas, DEFAULT_CANVAS_HEIGHT, DEFAULT_CANVAS_WIDTH, Rgba};
use hypercolor_types::effect::{ControlValue, EffectCategory, EffectMetadata, EffectSource};
use reqwest::Url;
use servo::{
    DeviceIntPoint, DeviceIntRect, JSValue, JavaScriptEvaluationError, Preferences,
    RenderingContext, Servo, ServoBuilder, WebView, WebViewBuilder,
};
use tracing::{debug, info, trace, warn};

use super::bootstrap_software_rendering_context;
use super::lightscript::LightscriptRuntime;
use super::paths::resolve_html_source_path;
use super::{ConsoleMessage, EffectRenderer, FrameInput, HypercolorWebViewDelegate};

const LOAD_TIMEOUT: Duration = Duration::from_secs(5);
const SCRIPT_TIMEOUT: Duration = Duration::from_millis(250);
const WORKER_READY_TIMEOUT: Duration = Duration::from_secs(10);
const RENDER_RESPONSE_TIMEOUT: Duration = Duration::from_millis(500);
const UNLOAD_TIMEOUT: Duration = Duration::from_secs(1);
const WEBVIEW_CLOSE_TIMEOUT: Duration = Duration::from_secs(2);
const RECENT_CONSOLE_SAMPLE_SIZE: usize = 6;
const CONSOLE_SNIPPET_RADIUS: usize = 1;
const CONSOLE_SNIPPET_LINE_MAX_CHARS: usize = 180;
const JS_TIMER_MIN_DURATION_MS: i64 = 4;
const DEFAULT_EFFECT_FPS_CAP: u32 = 30;
const MAX_EFFECT_FPS_CAP: u32 = 60;

// Servo initializes process-global options once. Recreating the runtime after a
// shutdown panics inside libservo, so Hypercolor keeps one shared worker alive
// for the daemon lifetime and reuses it across HTML effect switches.
static SERVO_WORKER: OnceLock<Mutex<Option<ServoWorker>>> = OnceLock::new();
thread_local! {
    static SERVO_WORKER_EXIT_GUARD: ServoWorkerExitGuard = const { ServoWorkerExitGuard };
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
        js_ion_offthread_compilation_enabled: false,
        media_glvideo_enabled: false,
        ..Preferences::default()
    }
}

/// Feature-gated renderer for HTML effects.
pub struct ServoRenderer {
    html_source: Option<PathBuf>,
    html_resolved_path: Option<PathBuf>,
    runtime_html_path: Option<PathBuf>,
    controls: HashMap<String, ControlValue>,
    runtime: LightscriptRuntime,
    initialized: bool,
    pending_scripts: Vec<String>,
    worker: Option<ServoWorkerClient>,
    queued_frame: Option<QueuedFrameInput>,
    in_flight_render: Option<PendingServoFrame>,
    last_canvas: Option<Canvas>,
    warned_fallback_frame: bool,
    warned_stalled_frame: bool,
    include_audio_updates: bool,
    last_animation_fps_cap: Option<u32>,
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
            runtime: LightscriptRuntime::new(DEFAULT_CANVAS_WIDTH, DEFAULT_CANVAS_HEIGHT),
            initialized: false,
            pending_scripts: Vec::new(),
            worker: None,
            queued_frame: None,
            in_flight_render: None,
            last_canvas: None,
            warned_fallback_frame: false,
            warned_stalled_frame: false,
            include_audio_updates: true,
            last_animation_fps_cap: None,
        }
    }

    fn enqueue_bootstrap_scripts(&mut self) {
        self.pending_scripts.push(self.runtime.bootstrap_script());
        self.pending_scripts
            .push(animation_fps_cap_script(DEFAULT_EFFECT_FPS_CAP));
        self.last_animation_fps_cap = Some(DEFAULT_EFFECT_FPS_CAP);
    }

    fn enqueue_frame_scripts(&mut self, input: &FrameInput) {
        let fps_cap = animation_fps_cap(input.delta_secs);
        if self.last_animation_fps_cap != Some(fps_cap) {
            self.pending_scripts.push(animation_fps_cap_script(fps_cap));
            self.last_animation_fps_cap = Some(fps_cap);
        }
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
        if let Some(script) = self
            .runtime
            .input_update_script_if_changed(&input.interaction)
        {
            self.pending_scripts.push(script);
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

    fn queue_frame(&mut self, input: &FrameInput<'_>) {
        if let Some(frame) = self.queued_frame.as_mut() {
            frame.merge_from_input(input);
            return;
        }

        self.queued_frame = Some(QueuedFrameInput::from_input(input));
    }

    fn poll_in_flight_render(&mut self) {
        let Some(render) = self.in_flight_render.as_mut() else {
            return;
        };

        match render.response_rx.try_recv() {
            Ok(result) => {
                self.in_flight_render = None;
                self.warned_stalled_frame = false;

                match result {
                    Ok(canvas) => {
                        self.last_canvas = Some(canvas);
                        self.warned_fallback_frame = false;
                    }
                    Err(error) => {
                        retire_shared_servo_worker_if_fatal("Servo frame render failed", &error);
                        if servo_worker_is_fatal_error(&error) {
                            self.worker = None;
                        }
                        warn!(%error, "Servo frame render failed");
                        if !self.warned_fallback_frame {
                            warn!("Falling back to the previous completed frame for this effect");
                            self.warned_fallback_frame = true;
                        }
                    }
                }
            }
            Err(TryRecvError::Empty) => {
                if !self.warned_stalled_frame
                    && render.submitted_at.elapsed() >= RENDER_RESPONSE_TIMEOUT
                {
                    warn!(
                        timeout_ms = RENDER_RESPONSE_TIMEOUT.as_millis(),
                        "Servo frame render is late; reusing previous frame"
                    );
                    self.warned_stalled_frame = true;
                }
            }
            Err(TryRecvError::Disconnected) => {
                self.in_flight_render = None;
                self.worker = None;
                retire_shared_servo_worker(
                    "Servo worker disconnected before sending frame response",
                );
                warn!("Servo worker disconnected before sending frame response");
                if !self.warned_fallback_frame {
                    warn!("Falling back to the previous completed frame for this effect");
                    self.warned_fallback_frame = true;
                }
            }
        }
    }

    fn try_submit_queued_frame(&mut self) {
        if self.in_flight_render.is_some() {
            return;
        }

        let Some(worker) = self.worker.clone() else {
            return;
        };
        let Some(frame) = self.queued_frame.take() else {
            return;
        };

        let frame_input = frame.as_frame_input();
        self.enqueue_frame_scripts(&frame_input);
        let scripts = std::mem::take(&mut self.pending_scripts);
        match worker.submit_render(scripts, frame.canvas_width, frame.canvas_height) {
            Ok(render) => {
                self.in_flight_render = Some(render);
                self.warned_stalled_frame = false;
            }
            Err(error) => {
                self.worker = None;
                retire_shared_servo_worker_if_fatal("Failed to queue Servo frame render", &error);
                warn!(%error, "Failed to queue Servo frame render");
                if !self.warned_fallback_frame {
                    warn!("Falling back to the previous completed frame for this effect");
                    self.warned_fallback_frame = true;
                }
            }
        }
    }

    fn drain_in_flight_render(&mut self) {
        let Some(render) = self.in_flight_render.take() else {
            return;
        };

        match render.response_rx.recv_timeout(RENDER_RESPONSE_TIMEOUT) {
            Ok(Ok(canvas)) => {
                self.last_canvas = Some(canvas);
                self.warned_fallback_frame = false;
                self.warned_stalled_frame = false;
            }
            Ok(Err(error)) => {
                retire_shared_servo_worker_if_fatal(
                    "Servo frame render failed while draining effect teardown",
                    &error,
                );
                if servo_worker_is_fatal_error(&error) {
                    self.worker = None;
                }
                warn!(%error, "Servo frame render failed while draining effect teardown");
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                retire_shared_servo_worker(
                    "Timed out waiting for in-flight Servo frame during effect teardown",
                );
                warn!(
                    timeout_ms = RENDER_RESPONSE_TIMEOUT.as_millis(),
                    "Timed out waiting for in-flight Servo frame during effect teardown"
                );
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                retire_shared_servo_worker(
                    "Servo worker disconnected while draining effect teardown",
                );
                warn!("Servo worker disconnected while draining effect teardown");
                self.worker = None;
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
        self.runtime = LightscriptRuntime::new(DEFAULT_CANVAS_WIDTH, DEFAULT_CANVAS_HEIGHT);
        self.pending_scripts.clear();
        self.warned_fallback_frame = false;
        self.warned_stalled_frame = false;
        self.include_audio_updates = effect_is_audio_reactive(metadata);
        self.last_animation_fps_cap = None;
        self.queued_frame = None;
        self.in_flight_render = None;
        self.last_canvas = None;
        self.controls = metadata
            .controls
            .iter()
            .map(|control| {
                (
                    control.control_id().to_owned(),
                    control.default_value.clone(),
                )
            })
            .collect();
        if !self.controls.is_empty() {
            debug!(
                effect = %metadata.name,
                control_count = self.controls.len(),
                controls = ?self.controls.keys().collect::<Vec<_>>(),
                "Loaded HTML default controls from metadata"
            );
        }

        let (runtime_source, runtime_html_path) =
            prepare_runtime_html_source(&resolved, &self.controls).with_context(|| {
                format!(
                    "failed to prepare runtime HTML source for '{}'",
                    resolved.display()
                )
            })?;

        let worker = acquire_servo_worker(DEFAULT_CANVAS_WIDTH, DEFAULT_CANVAS_HEIGHT)?;
        if let Err(error) = worker.load_effect(&runtime_source) {
            retire_shared_servo_worker_if_fatal("Servo effect page load failed", &error);
            return Err(error);
        }
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

    fn tick(&mut self, input: &FrameInput<'_>) -> Result<Canvas> {
        if !self.initialized {
            bail!("ServoRenderer tick called before init");
        }

        self.queue_frame(input);
        self.poll_in_flight_render();
        self.try_submit_queued_frame();

        Ok(self
            .last_canvas
            .clone()
            .unwrap_or_else(|| Self::placeholder_canvas(input)))
    }

    fn set_control(&mut self, name: &str, value: &ControlValue) {
        self.controls.insert(name.to_owned(), value.clone());
    }

    fn destroy(&mut self) {
        self.drain_in_flight_render();
        if let Some(worker) = self.worker.as_ref() {
            if let Err(error) = worker.unload_effect() {
                retire_shared_servo_worker_if_fatal(
                    "Failed to unload Servo effect page during destroy",
                    &error,
                );
                warn!(%error, "Failed to unload Servo effect page during destroy");
            }
        }
        self.worker = None;
        self.pending_scripts.clear();
        self.queued_frame = None;
        self.in_flight_render = None;
        self.last_canvas = None;
        self.controls.clear();
        self.html_source = None;
        self.html_resolved_path = None;
        self.cleanup_runtime_html();
        self.initialized = false;
        self.warned_fallback_frame = false;
        self.warned_stalled_frame = false;
        self.include_audio_updates = true;
        self.last_animation_fps_cap = None;
    }
}

#[derive(Debug, Clone)]
struct QueuedFrameInput {
    time_secs: f32,
    delta_secs: f32,
    frame_number: u64,
    audio: hypercolor_types::audio::AudioData,
    interaction: crate::input::InteractionData,
    canvas_width: u32,
    canvas_height: u32,
}

impl QueuedFrameInput {
    fn from_input(input: &FrameInput<'_>) -> Self {
        Self {
            time_secs: input.time_secs,
            delta_secs: input.delta_secs,
            frame_number: input.frame_number,
            audio: input.audio.clone(),
            interaction: input.interaction.clone(),
            canvas_width: input.canvas_width,
            canvas_height: input.canvas_height,
        }
    }

    fn merge_from_input(&mut self, input: &FrameInput<'_>) {
        let prior_recent_keys = self.interaction.keyboard.recent_keys.clone();
        self.time_secs = input.time_secs;
        self.delta_secs = input.delta_secs;
        self.frame_number = input.frame_number;
        self.audio = input.audio.clone();
        self.interaction = input.interaction.clone();
        merge_unique_strings(
            &mut self.interaction.keyboard.recent_keys,
            prior_recent_keys.into_iter(),
        );
        self.canvas_width = input.canvas_width;
        self.canvas_height = input.canvas_height;
    }

    fn as_frame_input(&self) -> FrameInput<'_> {
        FrameInput {
            time_secs: self.time_secs,
            delta_secs: self.delta_secs,
            frame_number: self.frame_number,
            audio: &self.audio,
            interaction: &self.interaction,
            canvas_width: self.canvas_width,
            canvas_height: self.canvas_height,
        }
    }
}

struct PendingServoFrame {
    response_rx: Receiver<Result<Canvas>>,
    submitted_at: Instant,
}

impl Default for ServoRenderer {
    fn default() -> Self {
        Self::new()
    }
}

struct ServoWorkerExitGuard;

impl Drop for ServoWorkerExitGuard {
    fn drop(&mut self) {
        if let Err(error) = shutdown_shared_servo_worker() {
            warn!(%error, "Failed to shut down shared Servo worker cleanly");
        }
    }
}

#[derive(Clone)]
struct ServoWorkerClient {
    command_tx: Sender<WorkerCommand>,
}

impl ServoWorkerClient {
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

    fn submit_render(
        &self,
        scripts: Vec<String>,
        width: u32,
        height: u32,
    ) -> Result<PendingServoFrame> {
        let (response_tx, response_rx) = mpsc::sync_channel(1);
        self.command_tx
            .send(WorkerCommand::Render {
                scripts,
                width,
                height,
                response_tx,
            })
            .context("failed to send render command to Servo worker")?;
        Ok(PendingServoFrame {
            response_rx,
            submitted_at: Instant::now(),
        })
    }

    fn unload_effect(&self) -> Result<()> {
        let (response_tx, response_rx) = mpsc::sync_channel(1);
        self.command_tx
            .send(WorkerCommand::Unload { response_tx })
            .context("failed to send unload command to Servo worker")?;

        match response_rx.recv_timeout(UNLOAD_TIMEOUT) {
            Ok(result) => result,
            Err(mpsc::RecvTimeoutError::Timeout) => bail!(
                "timed out waiting for Servo page unload after {}ms",
                UNLOAD_TIMEOUT.as_millis()
            ),
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                bail!("Servo worker disconnected before confirming page unload")
            }
        }
    }
}

/// Worker wrapper that owns the Servo runtime thread.
struct ServoWorker {
    command_tx: Option<Sender<WorkerCommand>>,
    thread_handle: Option<thread::JoinHandle<()>>,
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
        Ok(ServoWorkerClient {
            command_tx: self.command_tx()?.clone(),
        })
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

enum WorkerCommand {
    Load {
        html_path: PathBuf,
        response_tx: SyncSender<Result<()>>,
    },
    Unload {
        response_tx: SyncSender<Result<()>>,
    },
    Render {
        scripts: Vec<String>,
        width: u32,
        height: u32,
        response_tx: SyncSender<Result<Canvas>>,
    },
    Shutdown {
        response_tx: SyncSender<()>,
    },
}

struct ServoWorkerRuntime {
    webview: Option<WebView>,
    servo: Servo,
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
        let Some(webview) = self.webview.take() else {
            return Ok(());
        };

        let closed_before = self.delegate.closed_count();
        let webview_id = webview.id();
        drop(webview);

        let deadline = Instant::now() + WEBVIEW_CLOSE_TIMEOUT;
        while self.delegate.closed_count() == closed_before {
            self.servo.spin_event_loop();
            if Instant::now() >= deadline {
                bail!("timed out waiting for Servo webview close ({webview_id:?})");
            }
            std::thread::sleep(Duration::from_millis(1));
        }

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

    fn load_effect(&mut self, html_path: &Path) -> Result<()> {
        if self.loaded_html_path.is_some() {
            self.unload_effect()
                .context("failed to unload previous Servo effect page before loading new effect")?;
        }

        let url = file_url_for_path(html_path)?;
        self.loaded_html_path = Some(html_path.to_path_buf());
        self.delegate.reset_navigation_state();
        let webview = self.active_webview()?;
        debug!(url = %url, "Loading Servo effect page");
        webview.load(url.clone());
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

        let combined = combined_script(scripts);
        let preview = batched_script_preview(scripts);
        self.evaluate_script(&combined)
            .with_context(|| format!("failed to evaluate script batch: {preview}"))
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

fn combined_script(scripts: &[String]) -> String {
    let capacity = scripts.iter().map(String::len).sum::<usize>() + scripts.len();
    let mut combined = String::with_capacity(capacity);
    for script in scripts {
        combined.push_str(script);
        combined.push('\n');
    }
    combined
}

fn merge_unique_strings(destination: &mut Vec<String>, values: impl IntoIterator<Item = String>) {
    for value in values {
        if destination.iter().any(|existing| existing == &value) {
            continue;
        }
        destination.push(value);
    }
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

fn panic_payload_message(payload: &(dyn Any + Send + 'static)) -> String {
    if let Some(message) = payload.downcast_ref::<String>() {
        return message.clone();
    }
    if let Some(message) = payload.downcast_ref::<&'static str>() {
        return (*message).to_owned();
    }
    "unknown panic payload".to_owned()
}

#[allow(
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
fn animation_fps_cap(delta_secs: f32) -> u32 {
    if !delta_secs.is_finite() || delta_secs <= f32::EPSILON {
        return DEFAULT_EFFECT_FPS_CAP;
    }

    let fps = (1.0 / delta_secs).round();
    (fps as u32).clamp(1, MAX_EFFECT_FPS_CAP)
}

fn animation_fps_cap_script(fps_cap: u32) -> String {
    format!("window.__hypercolorFpsCap = {fps_cap};")
}

fn servo_worker_is_fatal_error(error: &anyhow::Error) -> bool {
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

fn retire_shared_servo_worker_if_fatal(context: &str, error: &anyhow::Error) {
    if !servo_worker_is_fatal_error(error) {
        return;
    }
    let message = format!("{context}: {error}");
    retire_shared_servo_worker(&message);
}

fn retire_shared_servo_worker(reason: &str) {
    let slot = SERVO_WORKER.get_or_init(|| Mutex::new(None));
    let mut guard = match slot.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };

    let Some(mut worker) = guard.take() else {
        return;
    };
    drop(guard);

    let had_command_tx = worker.command_tx.take().is_some();
    let had_thread_handle = worker.thread_handle.take().is_some();
    warn!(
        reason = reason,
        had_command_tx,
        had_thread_handle,
        "Retiring shared Servo worker without shutdown so a fresh worker can be spawned"
    );
}

fn acquire_servo_worker(width: u32, height: u32) -> Result<ServoWorkerClient> {
    SERVO_WORKER_EXIT_GUARD.with(|_| {});
    let slot = SERVO_WORKER.get_or_init(|| Mutex::new(None));
    let mut guard = match slot.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };

    if let Some(worker) = guard.as_ref() {
        return worker.client();
    }

    let worker = ServoWorker::spawn(width, height)?;
    let client = worker.client()?;
    *guard = Some(worker);
    Ok(client)
}

fn shutdown_shared_servo_worker() -> Result<()> {
    let slot = SERVO_WORKER.get_or_init(|| Mutex::new(None));
    let mut guard = match slot.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };

    let Some(mut worker) = guard.take() else {
        return Ok(());
    };
    drop(guard);
    worker.shutdown()
}

#[cfg(test)]
mod tests {
    use super::*;
    use hypercolor_types::audio::AudioData;
    use hypercolor_types::effect::EffectId;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, LazyLock};
    use uuid::Uuid;

    static SILENCE: LazyLock<AudioData> = LazyLock::new(AudioData::silence);
    static DEFAULT_INTERACTION: LazyLock<crate::input::InteractionData> =
        LazyLock::new(crate::input::InteractionData::default);

    #[derive(Debug)]
    struct RecordedRenderCommand {
        scripts: Vec<String>,
        width: u32,
        height: u32,
    }

    fn spawn_test_worker() -> (ServoWorker, Arc<AtomicBool>) {
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

    fn spawn_render_test_worker() -> (
        ServoWorker,
        Receiver<RecordedRenderCommand>,
        Sender<Result<Canvas>>,
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

    fn frame_input(delta_secs: f32) -> FrameInput<'static> {
        FrameInput {
            time_secs: 0.0,
            delta_secs,
            frame_number: 0,
            audio: &SILENCE,
            interaction: &DEFAULT_INTERACTION,
            canvas_width: DEFAULT_CANVAS_WIDTH,
            canvas_height: DEFAULT_CANVAS_HEIGHT,
        }
    }

    fn custom_interaction(
        recent_keys: &[&str],
        pressed_keys: &[&str],
    ) -> crate::input::InteractionData {
        crate::input::InteractionData {
            keyboard: crate::input::KeyboardData {
                pressed_keys: pressed_keys.iter().map(ToString::to_string).collect(),
                recent_keys: recent_keys.iter().map(ToString::to_string).collect(),
            },
            mouse: crate::input::MouseData::default(),
        }
    }

    fn custom_audio(rms_level: f32) -> AudioData {
        let mut audio = AudioData::silence();
        audio.rms_level = rms_level;
        audio
    }

    fn frame_input_with<'a>(
        delta_secs: f32,
        frame_number: u64,
        audio: &'a AudioData,
        interaction: &'a crate::input::InteractionData,
        canvas_width: u32,
        canvas_height: u32,
    ) -> FrameInput<'a> {
        FrameInput {
            time_secs: delta_secs * frame_number as f32,
            delta_secs,
            frame_number,
            audio,
            interaction,
            canvas_width,
            canvas_height,
        }
    }

    fn solid_canvas(r: u8, g: u8, b: u8) -> Canvas {
        let mut canvas = Canvas::new(DEFAULT_CANVAS_WIDTH, DEFAULT_CANVAS_HEIGHT);
        canvas.fill(Rgba::new(r, g, b, 255));
        canvas
    }

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
            controls: Vec::new(),
            presets: Vec::new(),
            audio_reactive: true,
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
            controls: Vec::new(),
            presets: Vec::new(),
            audio_reactive: false,
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
            controls: Vec::new(),
            presets: Vec::new(),
            audio_reactive: false,
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
        let (mut worker, stopped) = spawn_test_worker();

        worker.shutdown().expect("worker shutdown should succeed");

        assert!(stopped.load(Ordering::SeqCst));
        assert!(worker.command_tx.is_none());
        assert!(worker.thread_handle.is_none());
    }

    #[test]
    fn destroy_clears_renderer_state_without_shutting_down_shared_worker() {
        let (worker, stopped) = spawn_test_worker();

        let mut renderer = ServoRenderer::new();
        renderer.worker = Some(worker.client().expect("test worker client"));
        renderer.initialized = true;
        renderer.pending_scripts.push("tick()".to_owned());
        renderer
            .controls
            .insert("speed".to_owned(), ControlValue::Float(1.0));
        renderer.html_source = Some(PathBuf::from("source.html"));
        renderer.html_resolved_path = Some(PathBuf::from("resolved.html"));
        renderer.runtime_html_path = Some(PathBuf::from("runtime.html"));
        renderer.warned_fallback_frame = true;
        renderer.warned_stalled_frame = true;
        renderer.include_audio_updates = false;
        renderer.queued_frame = Some(QueuedFrameInput::from_input(&frame_input(1.0 / 30.0)));
        renderer.in_flight_render = Some(PendingServoFrame {
            response_rx: mpsc::channel().1,
            submitted_at: Instant::now(),
        });
        renderer.last_canvas = Some(solid_canvas(1, 2, 3));

        renderer.destroy();

        assert!(!stopped.load(Ordering::SeqCst));
        assert!(renderer.worker.is_none());
        assert!(renderer.pending_scripts.is_empty());
        assert!(renderer.queued_frame.is_none());
        assert!(renderer.in_flight_render.is_none());
        assert!(renderer.last_canvas.is_none());
        assert!(renderer.controls.is_empty());
        assert!(renderer.html_source.is_none());
        assert!(renderer.html_resolved_path.is_none());
        assert!(renderer.runtime_html_path.is_none());
        assert!(!renderer.initialized);
        assert!(!renderer.warned_fallback_frame);
        assert!(!renderer.warned_stalled_frame);
        assert!(renderer.include_audio_updates);

        drop(worker);
        assert!(stopped.load(Ordering::SeqCst));
    }

    #[test]
    fn bootstrap_scripts_seed_default_animation_cap() {
        let mut renderer = ServoRenderer::new();

        renderer.enqueue_bootstrap_scripts();

        assert_eq!(
            renderer.last_animation_fps_cap,
            Some(DEFAULT_EFFECT_FPS_CAP)
        );
        assert!(
            renderer
                .pending_scripts
                .iter()
                .any(|script| script == "window.__hypercolorFpsCap = 30;")
        );
    }

    #[test]
    fn frame_scripts_update_animation_cap_only_when_target_changes() {
        let mut renderer = ServoRenderer::new();
        renderer.enqueue_bootstrap_scripts();
        renderer.pending_scripts.clear();

        renderer.enqueue_frame_scripts(&frame_input(1.0 / 30.0));
        assert!(
            renderer
                .pending_scripts
                .iter()
                .all(|script| !script.contains("__hypercolorFpsCap"))
        );

        renderer.pending_scripts.clear();
        renderer.enqueue_frame_scripts(&frame_input(1.0 / 15.0));
        assert_eq!(renderer.last_animation_fps_cap, Some(15));
        assert!(
            renderer
                .pending_scripts
                .iter()
                .any(|script| script == "window.__hypercolorFpsCap = 15;")
        );
    }

    #[test]
    fn frame_scripts_skip_unchanged_input_updates() {
        let mut renderer = ServoRenderer::new();

        renderer.enqueue_frame_scripts(&frame_input(1.0 / 30.0));
        let first_input_scripts = renderer
            .pending_scripts
            .iter()
            .filter(|script| script.contains("window.engine.keyboard.keys"))
            .count();
        assert_eq!(first_input_scripts, 1);

        renderer.pending_scripts.clear();
        renderer.enqueue_frame_scripts(&frame_input(1.0 / 30.0));
        let second_input_scripts = renderer
            .pending_scripts
            .iter()
            .filter(|script| script.contains("window.engine.keyboard.keys"))
            .count();
        assert_eq!(second_input_scripts, 0);
    }

    #[test]
    fn queued_frames_submit_latest_state_after_in_flight_render_finishes() {
        let (worker, render_rx, result_tx, delivered_rx, _unload_rx, stopped) =
            spawn_render_test_worker();

        let mut renderer = ServoRenderer::new();
        renderer.worker = Some(worker.client().expect("test worker client"));
        renderer.initialized = true;
        renderer.enqueue_bootstrap_scripts();
        renderer.set_control("speed", &ControlValue::Float(0.25));

        let first_audio = custom_audio(0.1);
        let first_interaction = custom_interaction(&["a"], &["a"]);
        let first_frame =
            frame_input_with(1.0 / 30.0, 1, &first_audio, &first_interaction, 320, 200);

        let first_output = renderer.tick(&first_frame).expect("first tick");
        assert_eq!(first_output.width(), 320);
        assert_eq!(first_output.height(), 200);

        let first_render = render_rx
            .recv_timeout(Duration::from_millis(100))
            .expect("first render command");
        assert_eq!(first_render.width, 320);
        assert_eq!(first_render.height, 200);
        assert!(
            first_render
                .scripts
                .iter()
                .any(|script| script.contains("window[\"speed\"] = 0.25"))
        );

        renderer.set_control("speed", &ControlValue::Float(0.75));
        let second_audio = custom_audio(0.6);
        let second_interaction = custom_interaction(&["b"], &["b"]);
        let second_frame =
            frame_input_with(1.0 / 15.0, 2, &second_audio, &second_interaction, 640, 360);
        renderer.tick(&second_frame).expect("second tick");
        assert!(render_rx.recv_timeout(Duration::from_millis(20)).is_err());

        let third_interaction = custom_interaction(&["c"], &["c"]);
        let third_frame =
            frame_input_with(1.0 / 15.0, 3, &second_audio, &third_interaction, 640, 360);
        renderer.tick(&third_frame).expect("third tick");
        assert!(render_rx.recv_timeout(Duration::from_millis(20)).is_err());

        result_tx
            .send(Ok(solid_canvas(9, 8, 7)))
            .expect("first result should be delivered");
        delivered_rx
            .recv_timeout(Duration::from_millis(100))
            .expect("first result delivery ack");

        let resumed_output = renderer.tick(&third_frame).expect("resume tick");
        assert_eq!(resumed_output.get_pixel(0, 0), Rgba::new(9, 8, 7, 255));

        let second_render = render_rx
            .recv_timeout(Duration::from_millis(100))
            .expect("second render command");
        assert_eq!(second_render.width, 640);
        assert_eq!(second_render.height, 360);
        assert!(
            second_render
                .scripts
                .iter()
                .any(|script| script == "window.__hypercolorFpsCap = 15;")
        );
        assert!(
            second_render
                .scripts
                .iter()
                .any(|script| script.contains("window.engine.width = 640"))
        );
        assert!(
            second_render
                .scripts
                .iter()
                .any(|script| script.contains("window[\"speed\"] = 0.75"))
        );
        let interaction_script = second_render
            .scripts
            .iter()
            .find(|script| script.contains("window.engine.keyboard.recent"))
            .expect("interaction update script");
        assert!(interaction_script.contains("\"b\""));
        assert!(interaction_script.contains("\"c\""));
        assert!(interaction_script.contains("window.engine.mouse.down = false"));

        result_tx
            .send(Ok(solid_canvas(1, 1, 1)))
            .expect("cleanup render result");
        delivered_rx
            .recv_timeout(Duration::from_millis(100))
            .expect("cleanup result delivery ack");

        drop(worker);
        assert!(stopped.load(Ordering::SeqCst));
    }

    #[test]
    fn tick_reuses_last_completed_canvas_while_next_servo_frame_is_pending() {
        let (worker, render_rx, result_tx, delivered_rx, _unload_rx, stopped) =
            spawn_render_test_worker();

        let mut renderer = ServoRenderer::new();
        renderer.worker = Some(worker.client().expect("test worker client"));
        renderer.initialized = true;
        renderer.enqueue_bootstrap_scripts();

        let interaction = custom_interaction(&[], &[]);
        let audio = custom_audio(0.0);
        let frame = frame_input_with(1.0 / 30.0, 1, &audio, &interaction, 320, 200);

        renderer.tick(&frame).expect("initial tick");
        let _ = render_rx
            .recv_timeout(Duration::from_millis(100))
            .expect("first render command");

        result_tx
            .send(Ok(solid_canvas(20, 40, 60)))
            .expect("first result should be delivered");
        delivered_rx
            .recv_timeout(Duration::from_millis(100))
            .expect("first result delivery ack");

        let first_completed = renderer.tick(&frame).expect("completed tick");
        assert_eq!(first_completed.get_pixel(0, 0), Rgba::new(20, 40, 60, 255));
        let _ = render_rx
            .recv_timeout(Duration::from_millis(100))
            .expect("second render command");

        let reused = renderer.tick(&frame).expect("reused frame");
        assert_eq!(reused.get_pixel(0, 0), Rgba::new(20, 40, 60, 255));
        assert!(render_rx.recv_timeout(Duration::from_millis(20)).is_err());

        result_tx
            .send(Ok(solid_canvas(1, 1, 1)))
            .expect("cleanup render result");
        delivered_rx
            .recv_timeout(Duration::from_millis(100))
            .expect("cleanup result delivery ack");

        drop(worker);
        assert!(stopped.load(Ordering::SeqCst));
    }

    #[test]
    fn destroy_waits_for_in_flight_render_then_unloads_worker_page() {
        let (worker, render_rx, result_tx, _delivered_rx, unload_rx, stopped) =
            spawn_render_test_worker();

        let mut renderer = ServoRenderer::new();
        renderer.worker = Some(worker.client().expect("test worker client"));
        renderer.initialized = true;
        renderer.enqueue_bootstrap_scripts();

        let interaction = custom_interaction(&[], &[]);
        let audio = custom_audio(0.0);
        let frame = frame_input_with(1.0 / 30.0, 1, &audio, &interaction, 320, 200);

        renderer.tick(&frame).expect("initial tick");
        let _ = render_rx
            .recv_timeout(Duration::from_millis(100))
            .expect("first render command");

        let release_render = thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(20));
            result_tx
                .send(Ok(solid_canvas(7, 8, 9)))
                .expect("destroy should drain in-flight render");
        });

        renderer.destroy();

        unload_rx
            .recv_timeout(Duration::from_millis(100))
            .expect("destroy should unload the active Servo page");
        release_render
            .join()
            .expect("render release helper should not panic");

        drop(worker);
        assert!(stopped.load(Ordering::SeqCst));
    }
}
