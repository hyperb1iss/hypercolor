//! Servo-backed HTML effect renderer (feature-gated).
//!
//! This renderer runs Servo on a dedicated worker thread so the public
//! `EffectRenderer` remains `Send` while Servo internals stay on one thread.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc::{self, Receiver, Sender, SyncSender};
use std::thread::{self, JoinHandle};
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
use super::{EffectRenderer, FrameInput, HypercolorWebViewDelegate};

const DEFAULT_WIDTH: u32 = 320;
const DEFAULT_HEIGHT: u32 = 200;
const LOAD_TIMEOUT: Duration = Duration::from_secs(5);
const SCRIPT_TIMEOUT: Duration = Duration::from_millis(250);

/// Feature-gated renderer for HTML effects.
pub struct ServoRenderer {
    html_source: Option<PathBuf>,
    html_resolved_path: Option<PathBuf>,
    controls: HashMap<String, ControlValue>,
    runtime: LightscriptRuntime,
    initialized: bool,
    pending_scripts: Vec<String>,
    worker: Option<ServoWorker>,
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

        self.deactivate_worker();
        self.controls.clear();
        self.pending_scripts.clear();
        self.warned_fallback_frame = false;

        let worker = ServoWorker::spawn(&resolved, DEFAULT_WIDTH, DEFAULT_HEIGHT)?;
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
        self.deactivate_worker();
        self.pending_scripts.clear();
        self.controls.clear();
        self.html_source = None;
        self.html_resolved_path = None;
        self.initialized = false;
        self.warned_fallback_frame = false;
    }
}

impl ServoRenderer {
    fn deactivate_worker(&mut self) {
        if let Some(mut worker) = self.worker.take() {
            worker.shutdown();
        }
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
    join_handle: Option<JoinHandle<()>>,
}

impl ServoWorker {
    fn spawn(html_path: &Path, width: u32, height: u32) -> Result<Self> {
        let (command_tx, command_rx) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let html_path = html_path.to_path_buf();

        let join_handle = thread::Builder::new()
            .name("hypercolor-servo-worker".to_owned())
            .spawn(move || {
                let runtime = match ServoWorkerRuntime::new(&html_path, width, height) {
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

        let readiness = ready_rx
            .recv()
            .context("failed to receive Servo worker readiness signal")?;
        if let Err(error) = readiness {
            let _ = join_handle.join();
            return Err(error);
        }

        Ok(Self {
            command_tx,
            join_handle: Some(join_handle),
        })
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
        response_rx
            .recv()
            .context("failed to receive render response from Servo worker")?
    }

    fn shutdown(&mut self) {
        let _ = self.command_tx.send(WorkerCommand::Shutdown);
        if let Some(handle) = self.join_handle.take() {
            if let Err(error) = handle.join() {
                warn!(?error, "Servo worker thread panicked");
            }
        }
    }
}

impl Drop for ServoWorker {
    fn drop(&mut self) {
        self.shutdown();
    }
}

enum WorkerCommand {
    Render {
        scripts: Vec<String>,
        width: u32,
        height: u32,
        response_tx: SyncSender<Result<Canvas>>,
    },
    Shutdown,
}

struct ServoWorkerRuntime {
    servo: Servo,
    webview: WebView,
    rendering_context: Rc<dyn RenderingContext>,
    delegate: Rc<HypercolorWebViewDelegate>,
}

impl ServoWorkerRuntime {
    fn new(html_path: &Path, width: u32, height: u32) -> Result<Self> {
        install_rustls_provider();

        let rendering_context: Rc<dyn RenderingContext> =
            Rc::new(bootstrap_software_rendering_context(width, height)?);
        rendering_context.make_current().map_err(|error| {
            anyhow!("failed to make Servo rendering context current: {error:?}")
        })?;

        let servo = ServoBuilder::default().build();
        let delegate = Rc::new(HypercolorWebViewDelegate::new());
        let url = file_url_for_path(html_path)?;

        let webview = WebViewBuilder::new(&servo, Rc::clone(&rendering_context))
            .delegate(delegate.clone())
            .url(url)
            .build();

        let runtime = Self {
            servo,
            webview,
            rendering_context,
            delegate,
        };
        runtime.wait_for_load_completion(LOAD_TIMEOUT)?;
        Ok(runtime)
    }

    fn run(mut self, command_rx: Receiver<WorkerCommand>) {
        for command in command_rx {
            match command {
                WorkerCommand::Render {
                    scripts,
                    width,
                    height,
                    response_tx,
                } => {
                    let result = self.render_frame(&scripts, width, height);
                    let _ = response_tx.send(result);
                }
                WorkerCommand::Shutdown => break,
            }
        }
    }

    fn render_frame(&mut self, scripts: &[String], width: u32, height: u32) -> Result<Canvas> {
        self.resize_if_needed(width, height);

        for script in scripts {
            self.evaluate_script(script)
                .with_context(|| format!("failed to evaluate script: {script}"))?;
        }

        self.servo.spin_event_loop();
        if self.delegate.take_frame_ready() {
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

    fn wait_for_load_completion(&self, timeout: Duration) -> Result<()> {
        let deadline = Instant::now() + timeout;

        loop {
            self.servo.spin_event_loop();
            if self.delegate.take_page_loaded()
                || matches!(self.webview.load_status(), LoadStatus::Complete)
            {
                debug!("Servo page load completed");
                return Ok(());
            }

            if Instant::now() >= deadline {
                bail!("timed out waiting for Servo page load completion");
            }
            std::thread::sleep(Duration::from_millis(1));
        }
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
