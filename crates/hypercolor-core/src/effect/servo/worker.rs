//! Shared Servo worker thread lifecycle and runtime.
//!
//! Servo initializes process-global options exactly once; recreating the
//! runtime after a shutdown panics inside servo. Hypercolor therefore
//! keeps one worker alive for the entire daemon lifetime while creating
//! and destroying effect sessions on switch. The [`ServoCircuitBreaker`]
//! gates retries for soft failures so a flaky effect load can't
//! permanently knock HTML effects offline, while the legacy "poison
//! forever" path still applies to fatal conditions (channel disconnect,
//! thread exit).

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use base::generic_channel::GenericCallback;
use dpi::PhysicalSize;
use hypercolor_types::canvas::Canvas;
use profile_traits::mem::MemoryReportResult;
use reqwest::Url;
use servo::{
    JSValue, JavaScriptEvaluationError, Preferences, RenderingContext, Servo, ServoBuilder,
    WebView, WebViewBuilder,
};
#[cfg(feature = "servo-gpu-import")]
use tracing::warn;
use tracing::{debug, trace};

use super::delegate::HypercolorWebViewDelegate;
#[cfg(feature = "servo-gpu-import")]
use super::gpu_import_backend::{
    ServoFrameUnavailable, ServoGpuImportBackend, ServoGpuImportSessionContext,
    classify_failure as classify_servo_gpu_import_error,
    failure_detail as servo_gpu_import_failure_detail,
    failure_is_transient as servo_gpu_import_failure_is_transient,
    failure_should_clear_importer as servo_gpu_import_failure_should_clear_importer,
    record_imported_frame,
};
use super::memory::ServoMemoryReportSnapshot;
#[cfg(feature = "servo-gpu-import")]
use super::telemetry::record_servo_gpu_import_failure;
use super::telemetry::{
    record_servo_cpu_render_frame, record_servo_destroy_wait, record_servo_gpu_render_frame,
    record_servo_render_queue_wait,
};
use super::worker_client::{
    ServoFramePayload, ServoProducerRole, ServoRenderMode, ServoSessionId, WORKER_READY_TIMEOUT,
    WorkerCommand,
};
use crate::effect::servo_bootstrap::{ServoRenderingContextHandle, bootstrap_rendering_context};
use crate::effect::traits::EffectRenderOutput;
#[cfg(feature = "servo-gpu-import")]
use crate::effect::traits::ImportedEffectFrame;

mod console;
mod readback;
mod runtime_html;
mod scheduler;
mod shared;

use console::{
    find_initialization_failure_message, format_console_message, summarize_console_messages,
    truncate_for_log,
};
use readback::read_framebuffer_into_canvas;
pub(super) use runtime_html::{effect_is_audio_reactive, prepare_runtime_html_source};
use scheduler::{PendingRenderCommand, ScheduledServoWork, ServoWorkerScheduler};
#[cfg(test)]
pub(super) use shared::{
    ServoWorker, install_poisoned_shared_worker, install_running_shared_worker,
    reset_shared_servo_worker_state, shared_worker_is_vacant, shutdown_shared_servo_worker,
};
pub(super) use shared::{
    acquire_servo_worker, poison_shared_servo_worker_if_fatal, servo_worker_is_fatal_error,
};
pub use shared::{servo_memory_report_snapshot, shutdown_servo_runtime};

pub(super) const LOAD_TIMEOUT: Duration = Duration::from_secs(5);
const URL_LOAD_TIMEOUT: Duration = Duration::from_secs(15);
const SCRIPT_TIMEOUT: Duration = Duration::from_millis(250);
pub(super) const RENDER_RESPONSE_TIMEOUT: Duration = Duration::from_millis(500);
pub(super) const RECENT_CONSOLE_SAMPLE_SIZE: usize = 6;
const SCHEDULER_DRAIN_LIMIT: usize = 64;
const JS_TIMER_MIN_DURATION_MS: i64 = 4;
const MAX_SERVO_READBACK_RETIREES: usize = 8;
const STATIC_CANVAS_REUSE_NO_READY_FRAMES: u32 = 2;

/// Per-frame Servo render timings, split by stage. All durations in
/// microseconds; the shared `_us` suffix is deliberate.
#[allow(
    clippy::struct_field_names,
    reason = "every field is a duration in microseconds; the suffix is the unit"
)]
#[derive(Debug, Clone, Copy, Default)]
struct ServoRenderStageTimings {
    evaluate_scripts_us: u64,
    event_loop_us: u64,
    paint_us: u64,
    readback_us: u64,
    total_us: u64,
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
        js_mem_gc_empty_chunk_count_min: 0,
        js_mem_gc_high_frequency_heap_growth_max: 150,
        js_mem_gc_high_frequency_heap_growth_min: 120,
        js_mem_gc_high_frequency_high_limit_mb: 128,
        js_mem_gc_high_frequency_low_limit_mb: 64,
        js_mem_gc_low_frequency_heap_growth: 120,
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
        media_glvideo_enabled: false,
        shell_background_color_rgba: [0.0, 0.0, 0.0, 0.0],
        ..Preferences::default()
    }
}

fn script_preview(script: &str) -> String {
    let single_line = script.split_whitespace().collect::<Vec<_>>().join(" ");
    truncate_for_log(&single_line, 120)
}

fn render_update_preview(scripts: &[String], frame_payloads: &[ServoFramePayload]) -> String {
    let update_count = scripts.len() + frame_payloads.len();
    if scripts.len() == 1 && frame_payloads.is_empty() {
        return script_preview(&scripts[0]);
    }
    if scripts.is_empty() && frame_payloads.len() == 1 {
        return format!(
            "frame payload: {}",
            script_preview(frame_payloads[0].as_json())
        );
    }

    let mut previews = scripts
        .iter()
        .take(3)
        .map(|script| script_preview(script))
        .collect::<Vec<_>>();
    if previews.len() < 3 {
        previews.extend(
            frame_payloads
                .iter()
                .take(3 - previews.len())
                .map(|payload| format!("frame payload: {}", script_preview(payload.as_json()))),
        );
    }
    format!("{} updates: {}", update_count, previews.join(" | "))
}

fn can_reuse_cached_canvas(
    frame_ready: bool,
    update_count: usize,
    consecutive_no_ready_frames: u32,
    cached: Option<&Canvas>,
    width: u32,
    height: u32,
) -> bool {
    !frame_ready
        && update_count == 0
        && consecutive_no_ready_frames >= STATIC_CANVAS_REUSE_NO_READY_FRAMES
        && cached.is_some_and(|cached| cached.width() == width && cached.height() == height)
}

fn should_reuse_cached_canvas_after_transparent_readback(
    cached: Option<&Canvas>,
    candidate: &Canvas,
    width: u32,
    height: u32,
) -> bool {
    cached.is_some_and(|cached| {
        cached.width() == width && cached.height() == height && canvas_has_visible_alpha(cached)
    }) && candidate.width() == width
        && candidate.height() == height
        && canvas_is_fully_transparent(candidate)
}

#[cfg(feature = "servo-gpu-import")]
fn can_reuse_cached_gpu_frame(
    reuse_cached_on_no_ready: bool,
    frame_ready: bool,
    cached_width: u32,
    cached_height: u32,
    width: u32,
    height: u32,
) -> bool {
    reuse_cached_on_no_ready && !frame_ready && cached_width == width && cached_height == height
}

fn canvas_has_visible_alpha(canvas: &Canvas) -> bool {
    canvas
        .as_rgba_bytes()
        .chunks_exact(4)
        .any(|pixel| pixel[3] != 0)
}

fn canvas_is_fully_transparent(canvas: &Canvas) -> bool {
    canvas
        .as_rgba_bytes()
        .chunks_exact(4)
        .all(|pixel| pixel[3] == 0)
}

fn combined_script(buffer: &mut String, scripts: &[String], frame_payloads: &[ServoFramePayload]) {
    let script_bytes = scripts.iter().map(String::len).sum::<usize>();
    let payload_bytes = frame_payloads
        .iter()
        .map(ServoFramePayload::len)
        .sum::<usize>();
    let capacity = script_bytes
        + payload_bytes
        + scripts.len()
        + frame_payloads.len() * "window.__hypercolorApplyFramePayload();\n".len();
    buffer.clear();
    if buffer.capacity() < capacity {
        buffer.reserve(capacity - buffer.capacity());
    }
    for script in scripts {
        buffer.push_str(script);
        buffer.push('\n');
    }
    for payload in frame_payloads {
        let _ = writeln!(
            buffer,
            "window.__hypercolorApplyFramePayload({});",
            payload.as_json()
        );
    }
}

fn elapsed_micros(start: Instant) -> u64 {
    start.elapsed().as_micros().try_into().unwrap_or(u64::MAX)
}

fn log_servo_render_stage_timings(
    session_id: ServoSessionId,
    width: u32,
    height: u32,
    update_count: usize,
    update_bytes: usize,
    frame_ready: bool,
    emitted_cpu_frame: bool,
    reused_cached_canvas: bool,
    timings: ServoRenderStageTimings,
) {
    if emitted_cpu_frame {
        record_servo_cpu_render_frame(
            timings.evaluate_scripts_us,
            timings.event_loop_us,
            timings.paint_us,
            timings.readback_us,
            timings.total_us,
            reused_cached_canvas,
        );
    } else {
        record_servo_gpu_render_frame(
            timings.evaluate_scripts_us,
            timings.event_loop_us,
            timings.paint_us,
            timings.total_us,
        );
    }
    trace!(
        ?session_id,
        width,
        height,
        update_count,
        update_bytes,
        frame_ready,
        emitted_cpu_frame,
        reused_cached_canvas,
        evaluate_scripts_us = timings.evaluate_scripts_us,
        event_loop_us = timings.event_loop_us,
        paint_us = timings.paint_us,
        readback_us = timings.readback_us,
        total_us = timings.total_us,
        "Servo render stage timings"
    );
}

struct ServoSession {
    webview: Option<WebView>,
    rendering_context: Rc<dyn RenderingContext>,
    delegate: Rc<HypercolorWebViewDelegate>,
    loaded_html_path: Option<PathBuf>,
    script_buffer: String,
    readback_buffers: ServoReadbackBuffers,
    #[cfg(feature = "servo-gpu-import")]
    gpu_import: ServoGpuImportBackend,
    loaded_at: Option<Instant>,
    renders_since_load: u64,
    /// Most recent successful readback, retained so ticks where Servo has
    /// nothing new to composite can skip `read_to_image` entirely. Cloning a
    /// `Canvas` is an Arc refcount bump (zero-copy), so repeated reuse costs
    /// nothing beyond the flag check.
    last_canvas: Option<Canvas>,
    consecutive_no_ready_frames: u32,
}

#[derive(Default)]
struct ServoReadbackBuffers {
    retired_canvases: VecDeque<Canvas>,
}

impl ServoReadbackBuffers {
    fn take_buffer(&mut self, len: usize) -> Vec<u8> {
        let mut retained = VecDeque::with_capacity(self.retired_canvases.len());
        let mut reusable = None;

        while let Some(canvas) = self.retired_canvases.pop_front() {
            if reusable.is_none() && canvas.shared_ref_count() == 1 && canvas.rgba_len() == len {
                let (pixels, _copied) = canvas.into_rgba_bytes_with_copy_info();
                reusable = Some(pixels);
                continue;
            }

            if retained.len() < MAX_SERVO_READBACK_RETIREES {
                retained.push_back(canvas);
            }
        }

        self.retired_canvases = retained;
        reusable.unwrap_or_else(|| vec![0_u8; len])
    }

    fn retire_canvas(&mut self, canvas: Canvas) {
        self.retired_canvases.push_back(canvas);
        while self.retired_canvases.len() > MAX_SERVO_READBACK_RETIREES {
            self.retired_canvases.pop_front();
        }
    }
}

struct ServoWorkerRuntime {
    sessions: HashMap<ServoSessionId, ServoSession>,
    servo: Servo,
}

impl ServoWorkerRuntime {
    #[allow(
        clippy::unnecessary_wraps,
        reason = "signature matches fallible construction used elsewhere in the worker; collapsing would force call-site churn"
    )]
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
        let mut scheduler = ServoWorkerScheduler::default();

        loop {
            if scheduler.is_empty() {
                let Ok(command) = command_rx.recv() else {
                    break;
                };
                scheduler.push(command);
            }

            for _ in 0..SCHEDULER_DRAIN_LIMIT {
                let Ok(command) = command_rx.try_recv() else {
                    break;
                };
                scheduler.push(command);
            }

            let Some(work) = scheduler.next() else {
                continue;
            };

            match work {
                ScheduledServoWork::Command(WorkerCommand::CreateSession {
                    session_id,
                    producer_role,
                    width,
                    height,
                    response_tx,
                }) => {
                    let result = self.create_session(session_id, producer_role, width, height);
                    let _ = response_tx.send(result);
                }
                ScheduledServoWork::Command(WorkerCommand::Load {
                    session_id,
                    html_path,
                    width,
                    height,
                    response_tx,
                }) => {
                    let result = self.load_effect(session_id, &html_path, width, height);
                    let _ = response_tx.send(result);
                }
                ScheduledServoWork::Command(WorkerCommand::LoadUrl {
                    session_id,
                    url,
                    width,
                    height,
                    response_tx,
                }) => {
                    let result = self.load_url(session_id, &url, width, height);
                    let _ = response_tx.send(result);
                }
                ScheduledServoWork::Render(PendingRenderCommand {
                    session_id,
                    producer_role,
                    scripts,
                    frame_payloads,
                    width,
                    height,
                    mode,
                    submitted_at,
                    response_tx,
                }) => {
                    record_servo_render_queue_wait(producer_role, submitted_at.elapsed());
                    let result = self.render_frame(
                        session_id,
                        &scripts,
                        &frame_payloads,
                        width,
                        height,
                        mode,
                    );
                    let _ = response_tx.send(result);
                }
                ScheduledServoWork::Command(WorkerCommand::DestroySession {
                    session_id,
                    response_tx,
                }) => {
                    let destroy_started = Instant::now();
                    let result = self.destroy_session(session_id);
                    record_servo_destroy_wait(destroy_started.elapsed());
                    let _ = response_tx.send(result);
                }
                ScheduledServoWork::Command(WorkerCommand::MemoryReport { response_tx }) => {
                    let result = self.memory_report();
                    let _ = response_tx.send(result);
                }
                ScheduledServoWork::Command(WorkerCommand::Shutdown { response_tx }) => {
                    let _ = response_tx.send(());
                    break;
                }
                ScheduledServoWork::Command(WorkerCommand::Render { .. }) => {
                    unreachable!("render commands are normalized into scheduled render work")
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
        producer_role: ServoProducerRole,
        width: u32,
        height: u32,
    ) -> Result<()> {
        debug!(
            ?session_id,
            producer_role = producer_role.as_str(),
            width,
            height,
            "Creating Servo session"
        );
        if self.sessions.contains_key(&session_id) {
            bail!("Servo session {session_id:?} already exists");
        }

        #[cfg(feature = "servo-gpu-import")]
        let mut rendering_context_handle = Self::create_rendering_context(width, height)?;
        #[cfg(not(feature = "servo-gpu-import"))]
        let rendering_context_handle = Self::create_rendering_context(width, height)?;
        let rendering_context = rendering_context_handle.rendering_context.clone();
        #[cfg(feature = "servo-gpu-import")]
        let gpu_import = ServoGpuImportBackend::new(&mut rendering_context_handle);
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
                readback_buffers: ServoReadbackBuffers::default(),
                #[cfg(feature = "servo-gpu-import")]
                gpu_import,
                loaded_at: None,
                renders_since_load: 0,
                last_canvas: None,
                consecutive_no_ready_frames: 0,
            },
        );

        if let Err(error) = self.wait_for_load_completion(session_id, WORKER_READY_TIMEOUT, None) {
            self.sessions.remove(&session_id);
            return Err(error);
        }
        debug!(?session_id, width, height, "Servo session ready");

        #[cfg(all(feature = "servo-gpu-import", target_os = "macos"))]
        self.trace_macos_native_surface(session_id);

        Ok(())
    }

    fn create_rendering_context(width: u32, height: u32) -> Result<ServoRenderingContextHandle> {
        bootstrap_rendering_context(width, height)
    }

    #[cfg(all(feature = "servo-gpu-import", target_os = "macos"))]
    fn trace_macos_native_surface(&self, session_id: ServoSessionId) {
        let Ok(session) = self.session(session_id) else {
            return;
        };
        session.gpu_import.trace_macos_native_surface(session_id);
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

    fn make_session_rendering_context_current(&self, session_id: ServoSessionId) -> Result<()> {
        let session = self.session(session_id)?;
        session
            .rendering_context
            .make_current()
            .map_err(|error| anyhow!("failed to make Servo GL context current: {error:?}"))?;
        session.rendering_context.prepare_for_rendering();
        Ok(())
    }

    fn active_webview(&self, session_id: ServoSessionId) -> Result<&WebView> {
        self.session(session_id)?
            .webview
            .as_ref()
            .ok_or_else(|| anyhow!("Servo webview is not initialized"))
    }

    fn navigate_webview(
        &mut self,
        session_id: ServoSessionId,
        url: Url,
        timeout: Duration,
    ) -> Result<()> {
        {
            #[cfg(feature = "servo-gpu-import")]
            self.clear_gpu_importer(session_id);
            let session = self.session_mut(session_id)?;
            session.delegate.reset_navigation_state();
            session.script_buffer.clear();
            session.readback_buffers = ServoReadbackBuffers::default();
            #[cfg(feature = "servo-gpu-import")]
            {
                session.gpu_import.reset_retry_state();
            }
            session.loaded_at = None;
            session.renders_since_load = 0;
            session.last_canvas = None;
            session.consecutive_no_ready_frames = 0;
        }
        self.active_webview(session_id)?.load(url.clone());
        self.wait_for_load_completion(session_id, timeout, Some(url.as_str()))?;
        let session = self.session_mut(session_id)?;
        session.loaded_at = Some(Instant::now());
        session.renders_since_load = 0;
        Ok(())
    }

    fn load_effect(
        &mut self,
        session_id: ServoSessionId,
        html_path: &Path,
        width: u32,
        height: u32,
    ) -> Result<()> {
        self.resize_if_needed(session_id, width, height)?;
        let url = file_url_for_path(html_path)?;
        self.session_mut(session_id)?.loaded_html_path = Some(html_path.to_path_buf());
        debug!(
            ?session_id,
            url = %url,
            width,
            height,
            path = %html_path.display(),
            "Loading Servo effect page"
        );
        self.navigate_webview(session_id, url.clone(), LOAD_TIMEOUT)
            .context("failed to load Servo effect page")?;
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
        #[cfg(feature = "servo-gpu-import")]
        self.warm_gpu_importer_if_available(session_id, width, height);
        Ok(())
    }

    fn load_url(
        &mut self,
        session_id: ServoSessionId,
        url: &str,
        width: u32,
        height: u32,
    ) -> Result<()> {
        self.resize_if_needed(session_id, width, height)?;
        let parsed_url = Url::parse(url).with_context(|| format!("failed to parse URL '{url}'"))?;
        self.session_mut(session_id)?.loaded_html_path = None;
        debug!(url = %parsed_url, "Loading Servo URL");
        self.navigate_webview(session_id, parsed_url.clone(), URL_LOAD_TIMEOUT)
            .context("failed to load Servo URL")?;
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

    #[allow(
        clippy::unnecessary_wraps,
        reason = "callers invoke with `?` alongside other fallible session ops; keeping the Result keeps the dispatch uniform"
    )]
    fn destroy_session(&mut self, session_id: ServoSessionId) -> Result<()> {
        let Some(mut session) = self.sessions.remove(&session_id) else {
            return Ok(());
        };
        debug!(
            ?session_id,
            loaded_html_path = session
                .loaded_html_path
                .as_deref()
                .map_or_else(String::new, |path| path.display().to_string()),
            renders_since_load = session.renders_since_load,
            "Destroying Servo session"
        );

        #[cfg(feature = "servo-gpu-import")]
        session
            .gpu_import
            .clear_importer(&session.rendering_context);
        if let Some(webview) = session.webview.take() {
            drop(webview);
            self.servo.spin_event_loop();
        }

        drop(session);
        #[cfg(all(feature = "servo-gpu-import", target_os = "linux"))]
        self.refresh_linux_surfaces_after_peer_destroy(session_id);
        Ok(())
    }

    fn render_frame(
        &mut self,
        session_id: ServoSessionId,
        scripts: &[String],
        frame_payloads: &[ServoFramePayload],
        width: u32,
        height: u32,
        mode: ServoRenderMode,
    ) -> Result<EffectRenderOutput> {
        let update_count = scripts.len() + frame_payloads.len();
        let update_bytes = scripts.iter().map(String::len).sum::<usize>()
            + frame_payloads
                .iter()
                .map(ServoFramePayload::len)
                .sum::<usize>();
        {
            let frame_start = Instant::now();
            let mut timings = ServoRenderStageTimings::default();
            self.resize_if_needed(session_id, width, height)?;
            self.make_session_rendering_context_current(session_id)?;
            let renders_since_load = self
                .session(session_id)?
                .renders_since_load
                .saturating_add(1);
            self.session_mut(session_id)?.renders_since_load = renders_since_load;

            if update_count > 0 {
                let evaluate_scripts_start = Instant::now();
                self.evaluate_render_updates(session_id, scripts, frame_payloads)?;
                timings.evaluate_scripts_us = elapsed_micros(evaluate_scripts_start);
            }

            // Let timers/RAF advance after this tick's control/audio injection.
            let event_loop_start = Instant::now();
            self.active_webview(session_id)?.set_throttled(false);
            self.servo.spin_event_loop();
            timings.event_loop_us = elapsed_micros(event_loop_start);
            let frame_ready = self.session(session_id)?.delegate.take_frame_ready();
            if frame_ready {
                trace!("Servo delegate signaled new frame");
            }
            let consecutive_no_ready_frames = {
                let session = self.session_mut(session_id)?;
                if frame_ready {
                    session.consecutive_no_ready_frames = 0;
                } else {
                    session.consecutive_no_ready_frames =
                        session.consecutive_no_ready_frames.saturating_add(1);
                }
                session.consecutive_no_ready_frames
            };

            // Fast path for settled static pages. One missed delegate signal
            // is not enough: Servo can repaint changed content without first
            // calling `notify_new_frame_ready`, and RAF-driven effects can
            // otherwise look like every other frame is duplicated.
            if can_reuse_cached_canvas(
                frame_ready,
                update_count,
                consecutive_no_ready_frames,
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
                    update_count,
                    update_bytes,
                    frame_ready,
                    true,
                    true,
                    timings,
                );
                return Ok(EffectRenderOutput::Cpu(cached.clone()));
            }

            #[cfg(feature = "servo-gpu-import")]
            if mode.prefers_gpu()
                && let Some(cached) = self.session(session_id)?.gpu_import.cached_frame()
                && can_reuse_cached_gpu_frame(
                    mode.reuse_cached_gpu_frame_on_no_ready(),
                    frame_ready,
                    cached.width,
                    cached.height,
                    width,
                    height,
                )
            {
                let cached = cached.clone();
                timings.total_us = elapsed_micros(frame_start);
                log_servo_render_stage_timings(
                    session_id,
                    width,
                    height,
                    update_count,
                    update_bytes,
                    frame_ready,
                    false,
                    true,
                    timings,
                );
                return Ok(EffectRenderOutput::Gpu(cached));
            }

            let output = render_servo_framebuffer(self, session_id, mode, &mut timings)?;
            let output = match output {
                EffectRenderOutput::Cpu(canvas) => {
                    let session = self.session_mut(session_id)?;
                    #[cfg(feature = "servo-gpu-import")]
                    {
                        session.gpu_import.clear_cached_frame();
                    }
                    if should_reuse_cached_canvas_after_transparent_readback(
                        session.last_canvas.as_ref(),
                        &canvas,
                        width,
                        height,
                    ) {
                        let cached = session
                            .last_canvas
                            .as_ref()
                            .expect("cached canvas presence should match reuse check")
                            .clone();
                        session.readback_buffers.retire_canvas(canvas);
                        timings.total_us = elapsed_micros(frame_start);
                        log_servo_render_stage_timings(
                            session_id,
                            width,
                            height,
                            update_count,
                            update_bytes,
                            frame_ready,
                            true,
                            true,
                            timings,
                        );
                        return Ok(EffectRenderOutput::Cpu(cached));
                    }
                    let output_canvas = canvas.clone();
                    if let Some(previous) = session.last_canvas.replace(canvas) {
                        session.readback_buffers.retire_canvas(previous);
                    }
                    EffectRenderOutput::Cpu(output_canvas)
                }
                #[cfg(feature = "servo-gpu-import")]
                EffectRenderOutput::Gpu(frame) => {
                    let session = self.session_mut(session_id)?;
                    session.gpu_import.store_frame(frame.clone());
                    EffectRenderOutput::Gpu(frame)
                }
                EffectRenderOutput::Pending => EffectRenderOutput::Pending,
            };
            let emitted_cpu_frame = output.as_cpu_canvas().is_some();
            timings.total_us = elapsed_micros(frame_start);
            log_servo_render_stage_timings(
                session_id,
                width,
                height,
                update_count,
                update_bytes,
                frame_ready,
                emitted_cpu_frame,
                false,
                timings,
            );
            Ok(output)
        }
    }

    fn memory_report(&mut self) -> Result<ServoMemoryReportSnapshot> {
        let (response_tx, response_rx) = mpsc::sync_channel(1);
        let callback = GenericCallback::new(move |result: Result<MemoryReportResult, _>| {
            let result = result
                .map(ServoMemoryReportSnapshot::from_servo_result)
                .map_err(|error| anyhow!("Servo memory report callback failed: {error:?}"));
            let _ = response_tx.send(result);
        })
        .context("failed to create Servo memory report callback")?;

        self.servo.create_memory_report(callback);

        let deadline = Instant::now() + WORKER_READY_TIMEOUT;
        loop {
            match response_rx.try_recv() {
                Ok(result) => return result,
                Err(mpsc::TryRecvError::Disconnected) => {
                    bail!("Servo memory report callback disconnected");
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }

            self.servo.spin_event_loop();
            if Instant::now() >= deadline {
                bail!(
                    "timed out waiting for Servo memory report callback after {}ms",
                    WORKER_READY_TIMEOUT.as_millis()
                );
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    fn evaluate_render_updates(
        &mut self,
        session_id: ServoSessionId,
        scripts: &[String],
        frame_payloads: &[ServoFramePayload],
    ) -> Result<()> {
        if scripts.is_empty() && frame_payloads.is_empty() {
            return Ok(());
        }

        let mut script_buffer = {
            let session = self.session_mut(session_id)?;
            std::mem::take(&mut session.script_buffer)
        };
        combined_script(&mut script_buffer, scripts, frame_payloads);
        let result = self
            .evaluate_script(session_id, &script_buffer)
            .with_context(|| {
                format!(
                    "failed to evaluate render updates: {}",
                    render_update_preview(scripts, frame_payloads)
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
                debug!(
                    ?session_id,
                    current_url = last_url.as_deref().unwrap_or("<unknown>"),
                    "Servo page load completed"
                );
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

#[cfg(feature = "servo-gpu-import")]
impl ServoWorkerRuntime {
    fn warm_gpu_importer_if_available(
        &mut self,
        session_id: ServoSessionId,
        width: u32,
        height: u32,
    ) {
        let Ok(rendering_context) = self
            .session(session_id)
            .map(|session| Rc::clone(&session.rendering_context))
        else {
            return;
        };
        if let Ok(session) = self.session_mut(session_id) {
            session
                .gpu_import
                .warm_if_available(&rendering_context, width, height);
        }
    }

    fn import_gpu_frame(
        &mut self,
        session_id: ServoSessionId,
        width: u32,
        height: u32,
    ) -> Result<ImportedEffectFrame> {
        let (rendering_context, loaded_html_path, loaded_at, renders_since_load) = {
            let session = self.session(session_id)?;
            (
                Rc::clone(&session.rendering_context),
                session.loaded_html_path.clone(),
                session.loaded_at,
                session.renders_since_load,
            )
        };
        let context = ServoGpuImportSessionContext {
            session_id,
            rendering_context: &rendering_context,
            loaded_html_path: loaded_html_path.as_deref(),
            loaded_at,
            renders_since_load,
        };
        self.session_mut(session_id)?
            .gpu_import
            .import_frame(context, width, height)
    }

    fn clear_gpu_importer(&mut self, session_id: ServoSessionId) {
        let Ok(rendering_context) = self
            .session(session_id)
            .map(|session| Rc::clone(&session.rendering_context))
        else {
            return;
        };
        if let Ok(session) = self.session_mut(session_id) {
            session.gpu_import.clear_importer(&rendering_context);
        }
    }
}

#[cfg(all(feature = "servo-gpu-import", target_os = "linux"))]
impl ServoWorkerRuntime {
    fn refresh_linux_surfaces_after_peer_destroy(&mut self, destroyed_session_id: ServoSessionId) {
        let remaining_session_ids = self.sessions.keys().copied().collect::<Vec<_>>();
        for session_id in remaining_session_ids {
            if let Err(error) =
                self.refresh_linux_surface_after_peer_destroy(destroyed_session_id, session_id)
            {
                debug!(
                    %error,
                    ?destroyed_session_id,
                    ?session_id,
                    "Servo Linux surface refresh skipped after peer session teardown"
                );
            }
        }
    }

    fn refresh_linux_surface_after_peer_destroy(
        &mut self,
        destroyed_session_id: ServoSessionId,
        session_id: ServoSessionId,
    ) -> Result<()> {
        let rendering_context = {
            let session = self.session(session_id)?;
            Rc::clone(&session.rendering_context)
        };
        self.session_mut(session_id)?
            .gpu_import
            .refresh_linux_surface_after_peer_destroy(
                destroyed_session_id,
                session_id,
                &rendering_context,
            )
    }
}

#[cfg(feature = "servo-gpu-import")]
fn present_after_gpu_import_skip(runtime: &ServoWorkerRuntime, session_id: ServoSessionId) {
    if let Ok(session) = runtime.session(session_id) {
        if let Err(error) = session.rendering_context.make_current() {
            debug!(
                ?error,
                "Servo present skipped because context could not be made current"
            );
            return;
        }
        session.rendering_context.present();
    }
}

fn render_servo_framebuffer(
    runtime: &mut ServoWorkerRuntime,
    session_id: ServoSessionId,
    mode: ServoRenderMode,
    timings: &mut ServoRenderStageTimings,
) -> Result<EffectRenderOutput> {
    #[cfg(not(feature = "servo-gpu-import"))]
    let _ = mode;

    let paint_start = Instant::now();
    runtime.make_session_rendering_context_current(session_id)?;
    runtime.active_webview(session_id)?.paint();
    timings.paint_us = elapsed_micros(paint_start);

    let size = runtime.session(session_id)?.rendering_context.size();
    let width_i32 =
        i32::try_from(size.width).context("canvas width overflow for Servo readback")?;
    let height_i32 =
        i32::try_from(size.height).context("canvas height overflow for Servo readback")?;

    #[cfg(feature = "servo-gpu-import")]
    if mode.prefers_gpu() {
        let auto_import = matches!(
            super::servo_gpu_import_mode(),
            hypercolor_types::config::ServoGpuImportMode::Auto
        );
        let retry_delay = if auto_import {
            let now = Instant::now();
            runtime.session(session_id)?.gpu_import.retry_delay(now)
        } else {
            None
        };

        if let Some(retry_delay) = retry_delay {
            let retry_ms = duration_millis_u64(retry_delay);
            let session = runtime.session(session_id)?;
            trace!(
                ?session_id,
                retry_ms,
                page_age_ms = session
                    .loaded_at
                    .map(|loaded_at| duration_millis_u64(loaded_at.elapsed())),
                renders_since_load = session.renders_since_load,
                transient_failures = session.gpu_import.transient_failures(),
                "Servo GPU framebuffer import retry cooling down after transient failure"
            );
            present_after_gpu_import_skip(runtime, session_id);
            return Err(ServoFrameUnavailable::new(
                "transient_retry_cooldown",
                "gpu import retry is cooling down".to_owned(),
                retry_ms,
            )
            .into());
        }
        match import_servo_framebuffer_into_wgpu(runtime, session_id, size.width, size.height) {
            Ok(frame) => {
                let session = runtime.session_mut(session_id)?;
                session.gpu_import.note_success();
                record_imported_frame(&frame);
                super::servo_gpu_import_note_success();
                runtime.session(session_id)?.rendering_context.present();
                return Ok(EffectRenderOutput::Gpu(frame));
            }
            Err(error) => {
                if error.downcast_ref::<ServoFrameUnavailable>().is_some() {
                    present_after_gpu_import_skip(runtime, session_id);
                    return Err(error);
                }
                let reason = classify_servo_gpu_import_error(&error);
                let detail = servo_gpu_import_failure_detail(&error);
                let transient = servo_gpu_import_failure_is_transient(reason);
                let retry_ms = if transient && auto_import {
                    let session = runtime.session_mut(session_id)?;
                    Some(session.gpu_import.schedule_transient_retry())
                } else {
                    None
                };
                let cooldown_ms = if transient {
                    None
                } else {
                    super::servo_gpu_import_note_failure().map(duration_millis_u64)
                };

                if servo_gpu_import_failure_should_clear_importer(reason) {
                    runtime.clear_gpu_importer(session_id);
                }

                if transient && auto_import {
                    record_servo_gpu_import_failure(reason, false);
                    let session = runtime.session(session_id)?;
                    let page_age_ms = session
                        .loaded_at
                        .map(|loaded_at| duration_millis_u64(loaded_at.elapsed()));
                    debug!(
                        %error,
                        detail = detail.as_str(),
                        ?session_id,
                        reason = reason.as_str(),
                        width = size.width,
                        height = size.height,
                        retry_ms,
                        page_age_ms,
                        renders_since_load = session.renders_since_load,
                        transient_failures = session.gpu_import.transient_failures(),
                        has_cached_canvas = session.last_canvas.is_some(),
                        "Servo GPU framebuffer import hit transient GL state; deferring frame until retry"
                    );
                    present_after_gpu_import_skip(runtime, session_id);
                    return Err(ServoFrameUnavailable::new(
                        reason.as_str(),
                        detail,
                        retry_ms.expect("transient auto import failures should schedule retry"),
                    )
                    .into());
                }

                let allow_cpu_fallback = auto_import;
                record_servo_gpu_import_failure(reason, allow_cpu_fallback);

                if !allow_cpu_fallback {
                    warn!(
                        %error,
                        detail = detail.as_str(),
                        reason = reason.as_str(),
                        transient,
                        retry_ms,
                        cooldown_ms,
                        "Servo GPU framebuffer import failed; refusing CPU readback fallback"
                    );
                    present_after_gpu_import_skip(runtime, session_id);
                    return Err(error).context(
                        "Servo GPU framebuffer import failed without CPU readback fallback",
                    );
                }
                if transient {
                    debug!(
                        %error,
                        detail = detail.as_str(),
                        reason = reason.as_str(),
                        retry_ms,
                        "Servo GPU framebuffer import hit transient GL state; falling back to CPU readback"
                    );
                } else {
                    warn!(
                        %error,
                        detail = detail.as_str(),
                        reason = reason.as_str(),
                        cooldown_ms,
                        "Servo GPU framebuffer import failed; falling back to CPU readback"
                    );
                }
            }
        }
    }

    let readback_start = Instant::now();
    let canvas = {
        let session = runtime.session_mut(session_id)?;
        let canvas = read_framebuffer_into_canvas(session, width_i32, height_i32)?;
        session.rendering_context.present();
        canvas
    };
    timings.readback_us = elapsed_micros(readback_start);
    Ok(EffectRenderOutput::Cpu(canvas))
}

#[cfg(feature = "servo-gpu-import")]
fn import_servo_framebuffer_into_wgpu(
    runtime: &mut ServoWorkerRuntime,
    session_id: ServoSessionId,
    width: u32,
    height: u32,
) -> Result<ImportedEffectFrame> {
    runtime
        .import_gpu_frame(session_id, width, height)
        .context("failed to import Servo framebuffer into wgpu")
}

#[cfg(feature = "servo-gpu-import")]
fn duration_millis_u64(duration: Duration) -> u64 {
    duration.as_millis().try_into().unwrap_or(u64::MAX)
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
pub(super) mod test_support;

#[cfg(test)]
mod tests;
