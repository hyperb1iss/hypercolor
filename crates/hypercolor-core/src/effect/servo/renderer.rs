//! `EffectRenderer` facade over the shared Servo worker.
//!
//! The public [`ServoRenderer`] type is what the effect engine stores as
//! `Box<dyn EffectRenderer>` for HTML effects. It owns a cloneable handle
//! to the shared Servo worker and drives it through the per-frame
//! lifecycle: queue the latest frame, poll any in-flight render, and
//! submit a new render if the worker is idle. Hairy worker/runtime logic
//! lives in [`super::worker`]; the client state machine in
//! [`super::worker_client`]; this file is the orchestration layer.

use anyhow::{Result, bail};
use hypercolor_types::canvas::{Canvas, DEFAULT_CANVAS_HEIGHT, DEFAULT_CANVAS_WIDTH, Rgba};
use hypercolor_types::effect::{
    ControlKind, ControlValue, EffectCategory, EffectMetadata, EffectSource,
};
use hypercolor_types::sensor::SystemSnapshot;
use std::collections::HashMap;
use std::path::PathBuf;
#[cfg(not(test))]
use std::sync::OnceLock;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use tracing::{debug, info, warn};

use super::telemetry::{
    record_servo_detached_destroy, record_servo_page_load, record_servo_session_create,
    record_servo_soft_stall,
};
use super::worker::{
    RENDER_RESPONSE_TIMEOUT, effect_is_audio_reactive, prepare_runtime_html_source,
    servo_worker_is_fatal_error,
};
use super::{ServoSessionHandle, SessionConfig, note_servo_session_error};
use crate::effect::lightscript::LightscriptRuntime;
use crate::effect::paths::resolve_html_source_path;
#[cfg(feature = "servo-gpu-import")]
use crate::effect::traits::EffectRenderOutput;
use crate::effect::traits::{EffectRenderer, FrameInput, prepare_target_canvas};
use crate::engine::FpsTier;

const DEFAULT_EFFECT_FPS_CAP: u32 = 30;
const DEFAULT_DISPLAY_FPS_CAP: u32 = 30;
const MAX_EFFECT_FPS_CAP: u32 = 60;
const SOFT_STALL_FRAME_INTERVALS: u32 = 5;

#[cfg(not(test))]
static REUSABLE_SERVO_SESSION: OnceLock<Mutex<Option<ServoSessionHandle>>> = OnceLock::new();

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AnimationCadence {
    MatchRenderLoop,
    Fixed(u32),
}

impl AnimationCadence {
    fn fps_cap(self, delta_secs: f32) -> u32 {
        match self {
            Self::MatchRenderLoop => animation_fps_cap(delta_secs),
            Self::Fixed(fps_cap) => fps_cap,
        }
    }

    fn render_due(self, last_submit_time_secs: Option<f32>, next_time_secs: f32) -> bool {
        match self {
            Self::MatchRenderLoop => true,
            Self::Fixed(fps_cap) => {
                let min_frame_interval_secs = 1.0 / fps_cap.max(1) as f32;
                last_submit_time_secs.is_none_or(|last_submit_time_secs| {
                    next_time_secs + f32::EPSILON >= last_submit_time_secs + min_frame_interval_secs
                })
            }
        }
    }
}

struct ServoLoadTask {
    response_rx: Receiver<Result<LoadedServoSession>>,
    shared: Arc<Mutex<ServoLoadTaskState>>,
    started_at: Instant,
}

impl ServoLoadTask {
    fn try_discard_loaded_session(&self) {
        let mut state = lock_servo_load_task_state(&self.shared);
        state.canceled = true;
        match self.response_rx.try_recv() {
            Ok(Ok(loaded)) => loaded.discard(),
            Ok(Err(_)) | Err(TryRecvError::Empty | TryRecvError::Disconnected) => {}
        }
    }
}

impl Drop for ServoLoadTask {
    fn drop(&mut self) {
        self.try_discard_loaded_session();
    }
}

struct ServoLoadTaskState {
    canceled: bool,
}

struct LoadedServoSession {
    session: ServoSessionHandle,
    runtime_source: PathBuf,
    runtime_html_path: Option<PathBuf>,
}

impl LoadedServoSession {
    fn discard(self) {
        let runtime_html_path = self.runtime_html_path.clone();
        recycle_servo_session(self.session, "abandoned Servo session");
        if let Some(path) = runtime_html_path.as_ref() {
            cleanup_runtime_html_path(path);
        }
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
    session: Option<ServoSessionHandle>,
    load_task: Option<ServoLoadTask>,
    load_failed: Option<String>,
    queued_frame: Option<QueuedFrameInput>,
    last_canvas: Option<Canvas>,
    #[cfg(feature = "servo-gpu-import")]
    last_gpu_frame: Option<hypercolor_linux_gpu_interop::ImportedEffectFrame>,
    warned_fallback_frame: bool,
    warned_stalled_frame: bool,
    include_audio_updates: bool,
    include_screen_updates: bool,
    include_sensor_updates: bool,
    last_animation_fps_cap: Option<u32>,
    animation_cadence: AnimationCadence,
    host_driven_animation: bool,
    last_submit_time_secs: Option<f32>,
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
            session: None,
            load_task: None,
            load_failed: None,
            queued_frame: None,
            last_canvas: None,
            #[cfg(feature = "servo-gpu-import")]
            last_gpu_frame: None,
            warned_fallback_frame: false,
            warned_stalled_frame: false,
            include_audio_updates: true,
            include_screen_updates: false,
            include_sensor_updates: false,
            last_animation_fps_cap: None,
            animation_cadence: AnimationCadence::MatchRenderLoop,
            host_driven_animation: true,
            last_submit_time_secs: None,
        }
    }

    fn enqueue_bootstrap_scripts(&mut self) {
        self.pending_scripts.push(self.runtime.bootstrap_script());
        self.last_animation_fps_cap = Some(DEFAULT_EFFECT_FPS_CAP);
    }

    fn enqueue_frame_scripts(&mut self, input: &FrameInput) {
        let fps_cap = self.animation_cadence.fps_cap(input.delta_secs);
        self.last_animation_fps_cap = Some(fps_cap);
        if let Some(script) = self
            .runtime
            .resize_script(input.canvas_width, input.canvas_height)
        {
            self.pending_scripts.push(script);
        }
        self.runtime.push_frame_scripts(
            &mut self.pending_scripts,
            &input.audio,
            input.screen,
            input.sensors,
            &self.controls,
            self.include_audio_updates,
            self.include_screen_updates,
            self.include_sensor_updates,
        );
        if let Some(script) = self
            .runtime
            .input_update_script_if_changed(&input.interaction)
        {
            self.pending_scripts.push(script);
        }
        if self.host_driven_animation {
            self.pending_scripts
                .push(LightscriptRuntime::host_frame_script());
        }
    }

    fn render_placeholder_into(target: &mut Canvas, input: &FrameInput) {
        if target.width() != input.canvas_width || target.height() != input.canvas_height {
            *target = Canvas::new(input.canvas_width, input.canvas_height);
        }
        let frame_mod = u8::try_from(input.frame_number % u64::from(u8::MAX)).unwrap_or_default();
        #[allow(
            clippy::as_conversions,
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss
        )]
        let audio_mod = (input.audio.rms_level.clamp(0.0, 1.0) * f32::from(u8::MAX)) as u8;

        let color = Rgba::new(frame_mod, audio_mod, frame_mod.saturating_add(32), 255);
        target.fill(color);
    }

    fn take_pending_scripts(&mut self) -> Vec<String> {
        let capacity = self.pending_scripts.capacity();
        std::mem::replace(&mut self.pending_scripts, Vec::with_capacity(capacity))
    }

    fn cleanup_runtime_html(&mut self) {
        if let Some(path) = self.runtime_html_path.take() {
            cleanup_runtime_html_path(&path);
        }
    }

    fn initialize_with_canvas_size(
        &mut self,
        metadata: &EffectMetadata,
        canvas_width: u32,
        canvas_height: u32,
    ) -> Result<()> {
        let EffectSource::Html { path } = &metadata.source else {
            bail!(
                "ServoRenderer requires EffectSource::Html, got source {:?} for effect '{}'",
                metadata.source,
                metadata.name
            );
        };

        self.destroy();
        self.cleanup_runtime_html();
        self.session = None;
        self.load_task = None;
        self.load_failed = None;
        self.controls.clear();
        self.runtime = LightscriptRuntime::new(canvas_width, canvas_height);
        self.pending_scripts.clear();
        self.warned_fallback_frame = false;
        self.warned_stalled_frame = false;
        self.include_audio_updates = effect_is_audio_reactive(metadata);
        self.include_screen_updates = metadata.screen_reactive;
        self.include_sensor_updates = effect_uses_sensor_data(metadata);
        self.last_animation_fps_cap = None;
        self.animation_cadence = animation_cadence(metadata);
        self.host_driven_animation = host_driven_animation(metadata);
        self.last_submit_time_secs = None;
        self.queued_frame = None;
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

        self.html_source = Some(path.clone());
        self.html_resolved_path = None;
        self.runtime_html_path = None;
        self.initialized = true;
        self.load_task = Some(start_servo_load_task(
            metadata.name.clone(),
            path.clone(),
            self.controls.clone(),
            canvas_width,
            canvas_height,
        ));

        info!(
            effect = %metadata.name,
            source = %path.display(),
            canvas_width,
            canvas_height,
            "Queued ServoRenderer load"
        );

        Ok(())
    }

    fn poll_load_task(&mut self) {
        let Some(result) = self
            .load_task
            .as_ref()
            .map(|task| task.response_rx.try_recv())
        else {
            return;
        };

        match result {
            Ok(Ok(loaded)) => {
                let started_at = self
                    .load_task
                    .as_ref()
                    .map_or_else(Instant::now, |task| task.started_at);
                let LoadedServoSession {
                    session,
                    runtime_source,
                    runtime_html_path,
                } = loaded;
                self.load_task = None;
                info!(
                    resolved = %runtime_source.display(),
                    wait_ms = started_at.elapsed().as_millis(),
                    "ServoRenderer load completed"
                );
                self.html_resolved_path = Some(runtime_source);
                self.runtime_html_path = runtime_html_path;
                self.session = Some(session);
                self.load_failed = None;
                self.enqueue_bootstrap_scripts();
            }
            Ok(Err(error)) => {
                self.load_task = None;
                let message = error.to_string();
                if self.load_failed.as_deref() != Some(message.as_str()) {
                    warn!(%error, "ServoRenderer load failed; rendering placeholder frames");
                }
                self.load_failed = Some(message);
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => {
                self.load_task = None;
                let message = "Servo load task disconnected before completion".to_owned();
                if self.load_failed.as_deref() != Some(message.as_str()) {
                    warn!(
                        message,
                        "ServoRenderer load failed; rendering placeholder frames"
                    );
                }
                self.load_failed = Some(message);
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
        let pending_age = self
            .session
            .as_ref()
            .and_then(ServoSessionHandle::pending_render_age);
        let soft_stall_timeout = self.soft_stall_timeout();
        let Some(session) = self.session.as_mut() else {
            return;
        };

        match session.poll_frame() {
            Ok(Some(canvas)) => {
                self.warned_stalled_frame = false;
                self.last_canvas = Some(canvas);
                self.warned_fallback_frame = false;
            }
            Ok(None) => {
                if !self.warned_stalled_frame
                    && pending_age.is_some_and(|age| age >= soft_stall_timeout)
                {
                    record_servo_soft_stall();
                    warn!(
                        fps_cap = self.active_fps_cap(),
                        pending_age_ms = pending_age.map_or(0, |age| age.as_millis()),
                        soft_timeout_ms = soft_stall_timeout.as_millis(),
                        "Servo frame render is late; reusing previous frame"
                    );
                    self.warned_stalled_frame = true;
                }
            }
            Err(error) => {
                note_servo_session_error("Servo frame render failed", &error);
                if servo_worker_is_fatal_error(&error) {
                    self.session = None;
                }
                warn!(%error, "Servo frame render failed");
                if !self.warned_fallback_frame {
                    warn!("Falling back to the previous completed frame for this effect");
                    self.warned_fallback_frame = true;
                }
            }
        }
    }

    #[cfg(feature = "servo-gpu-import")]
    fn poll_in_flight_render_output(&mut self) {
        let pending_age = self
            .session
            .as_ref()
            .and_then(ServoSessionHandle::pending_render_age);
        let soft_stall_timeout = self.soft_stall_timeout();
        let Some(session) = self.session.as_mut() else {
            return;
        };

        match session.poll_output() {
            Ok(Some(EffectRenderOutput::Cpu(canvas))) => {
                self.warned_stalled_frame = false;
                self.last_canvas = Some(canvas);
                self.last_gpu_frame = None;
                self.warned_fallback_frame = false;
            }
            Ok(Some(EffectRenderOutput::Gpu(frame))) => {
                self.warned_stalled_frame = false;
                self.last_gpu_frame = Some(frame);
                self.warned_fallback_frame = false;
            }
            Ok(None) => {
                if !self.warned_stalled_frame
                    && pending_age.is_some_and(|age| age >= soft_stall_timeout)
                {
                    record_servo_soft_stall();
                    warn!(
                        fps_cap = self.active_fps_cap(),
                        pending_age_ms = pending_age.map_or(0, |age| age.as_millis()),
                        soft_timeout_ms = soft_stall_timeout.as_millis(),
                        "Servo frame render is late; reusing previous frame"
                    );
                    self.warned_stalled_frame = true;
                }
            }
            Err(error) => {
                note_servo_session_error("Servo frame render failed", &error);
                if servo_worker_is_fatal_error(&error) {
                    self.session = None;
                }
                warn!(%error, "Servo frame render failed");
                if !self.warned_fallback_frame {
                    warn!("Falling back to the previous completed frame for this effect");
                    self.warned_fallback_frame = true;
                }
            }
        }
    }

    fn try_submit_queued_frame(&mut self) {
        self.try_submit_queued_frame_with_gpu_preference(false);
    }

    fn try_submit_queued_frame_with_gpu_preference(&mut self, prefer_gpu: bool) {
        let Some(session) = self.session.as_ref() else {
            return;
        };
        if session.has_pending_render() {
            return;
        }
        let Some(frame) = self.queued_frame.take() else {
            return;
        };
        if !self
            .animation_cadence
            .render_due(self.last_submit_time_secs, frame.time_secs)
        {
            self.queued_frame = Some(frame);
            return;
        }

        let frame_input = frame.as_frame_input();
        self.enqueue_frame_scripts(&frame_input);
        if let Some(session) = self.session.as_mut() {
            session.resize(frame.canvas_width, frame.canvas_height);
        }
        let scripts = self.take_pending_scripts();
        let request_result = {
            let session = self
                .session
                .as_mut()
                .expect("session presence should be stable while queuing one render");
            if prefer_gpu {
                #[cfg(feature = "servo-gpu-import")]
                {
                    session.request_render_gpu(scripts)
                }
                #[cfg(not(feature = "servo-gpu-import"))]
                {
                    session.request_render(scripts)
                }
            } else {
                session.request_render(scripts)
            }
        };

        match request_result {
            Ok(()) => {
                self.warned_stalled_frame = false;
                self.last_submit_time_secs = Some(frame.time_secs);
            }
            Err(error) => {
                note_servo_session_error("Failed to queue Servo frame render", &error);
                self.session = None;
                warn!(%error, "Failed to queue Servo frame render");
                if !self.warned_fallback_frame {
                    warn!("Falling back to the previous completed frame for this effect");
                    self.warned_fallback_frame = true;
                }
            }
        }
    }

    fn active_fps_cap(&self) -> u32 {
        self.last_animation_fps_cap
            .unwrap_or(DEFAULT_EFFECT_FPS_CAP)
    }

    fn soft_stall_timeout(&self) -> Duration {
        let tier = FpsTier::from_fps(self.active_fps_cap());
        let soft_timeout = tier
            .frame_interval()
            .mul_f32(SOFT_STALL_FRAME_INTERVALS as f32);
        soft_timeout.min(RENDER_RESPONSE_TIMEOUT)
    }
}

fn copy_completed_canvas_into_target(source: &Canvas, target: &mut Canvas) {
    prepare_target_canvas(target, source.width(), source.height());
    target
        .as_rgba_bytes_mut()
        .copy_from_slice(source.as_rgba_bytes());
}

fn start_servo_load_task(
    effect_name: String,
    html_source: PathBuf,
    controls: HashMap<String, ControlValue>,
    canvas_width: u32,
    canvas_height: u32,
) -> ServoLoadTask {
    let (response_tx, response_rx) = mpsc::sync_channel(1);
    let response_tx_for_thread = response_tx.clone();
    let shared = Arc::new(Mutex::new(ServoLoadTaskState { canceled: false }));
    let shared_for_thread = Arc::clone(&shared);
    let spawn_result = thread::Builder::new()
        .name(format!("hypercolor-servo-load-{effect_name}"))
        .spawn(move || {
            let result = load_servo_session(
                &effect_name,
                html_source,
                &controls,
                canvas_width,
                canvas_height,
            );
            match result {
                Ok(loaded) => {
                    let state = lock_servo_load_task_state(&shared_for_thread);
                    if state.canceled {
                        drop(state);
                        loaded.discard();
                    } else if let Err(mpsc::SendError(Ok(abandoned))) =
                        response_tx_for_thread.send(Ok(loaded))
                    {
                        drop(state);
                        abandoned.discard();
                    }
                }
                Err(error) => {
                    let state = lock_servo_load_task_state(&shared_for_thread);
                    if !state.canceled {
                        let _ = response_tx_for_thread.send(Err(error));
                    }
                }
            }
        });
    if let Err(error) = spawn_result {
        let _ = response_tx.send(Err(anyhow::anyhow!(
            "failed to spawn Servo load helper thread: {error}"
        )));
    }

    ServoLoadTask {
        response_rx,
        shared,
        started_at: Instant::now(),
    }
}

fn lock_servo_load_task_state(
    shared: &Arc<Mutex<ServoLoadTaskState>>,
) -> std::sync::MutexGuard<'_, ServoLoadTaskState> {
    match shared.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

#[cfg(not(test))]
fn reusable_servo_session_slot() -> &'static Mutex<Option<ServoSessionHandle>> {
    REUSABLE_SERVO_SESSION.get_or_init(|| Mutex::new(None))
}

#[cfg(not(test))]
fn lock_reusable_servo_session_slot() -> std::sync::MutexGuard<'static, Option<ServoSessionHandle>>
{
    match reusable_servo_session_slot().lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

#[cfg(not(test))]
fn take_reusable_servo_session(width: u32, height: u32) -> Option<ServoSessionHandle> {
    let mut session = lock_reusable_servo_session_slot().take()?;
    session.prepare_for_reuse(width, height);
    debug!(
        canvas_width = width,
        canvas_height = height,
        "Reusing shared Servo effect session"
    );
    Some(session)
}

#[cfg(test)]
fn take_reusable_servo_session(_width: u32, _height: u32) -> Option<ServoSessionHandle> {
    None
}

#[cfg(not(test))]
fn recycle_servo_session(mut session: ServoSessionHandle, reason: &'static str) {
    session.discard_frame_state();
    let mut maybe_session = Some(session);
    {
        let mut slot = lock_reusable_servo_session_slot();
        if slot.is_none() {
            *slot = maybe_session.take();
            debug!(reason, "Recycled shared Servo effect session");
        }
    }

    if let Some(session) = maybe_session {
        close_servo_session_detached(session, "extra reusable Servo session");
    }
}

#[cfg(test)]
fn recycle_servo_session(session: ServoSessionHandle, reason: &'static str) {
    close_servo_session_detached(session, reason);
}

fn close_servo_session_detached(session: ServoSessionHandle, reason: &'static str) {
    match session.close_detached() {
        Ok(()) => record_servo_detached_destroy(true),
        Err(error) => {
            record_servo_detached_destroy(false);
            note_servo_session_error("Failed to queue Servo session destroy", &error);
            warn!(%error, reason, "Failed to queue Servo session destroy");
        }
    }
}

fn load_servo_session(
    effect_name: &str,
    html_source: PathBuf,
    controls: &HashMap<String, ControlValue>,
    canvas_width: u32,
    canvas_height: u32,
) -> Result<LoadedServoSession> {
    use anyhow::Context;

    let resolved = resolve_html_source_path(&html_source).with_context(|| {
        format!(
            "failed to resolve HTML source for effect '{effect_name}' from '{}'",
            html_source.display()
        )
    })?;

    let (runtime_source, runtime_html_path) = prepare_runtime_html_source(&resolved, controls)
        .with_context(|| {
            format!(
                "failed to prepare runtime HTML source for '{}'",
                resolved.display()
            )
        })?;

    let mut session =
        if let Some(session) = take_reusable_servo_session(canvas_width, canvas_height) {
            session
        } else {
            let session_create_started = Instant::now();
            match ServoSessionHandle::new_shared(SessionConfig {
                render_width: canvas_width,
                render_height: canvas_height,
                inject_engine_globals: true,
            }) {
                Ok(session) => {
                    record_servo_session_create(session_create_started.elapsed(), true);
                    session
                }
                Err(error) => {
                    record_servo_session_create(session_create_started.elapsed(), false);
                    cleanup_runtime_html_option(runtime_html_path.as_ref());
                    note_servo_session_error("Servo effect session creation failed", &error);
                    return Err(error);
                }
            }
        };

    let page_load_started = Instant::now();
    if let Err(error) = session.load_html_file(&runtime_source) {
        record_servo_page_load(page_load_started.elapsed(), false);
        close_servo_session_detached(session, "Servo effect session after page-load failure");
        cleanup_runtime_html_option(runtime_html_path.as_ref());
        note_servo_session_error("Servo effect page load failed", &error);
        return Err(error);
    }
    record_servo_page_load(page_load_started.elapsed(), true);

    Ok(LoadedServoSession {
        session,
        runtime_source,
        runtime_html_path,
    })
}

fn cleanup_runtime_html_option(path: Option<&PathBuf>) {
    if let Some(path) = path {
        cleanup_runtime_html_path(path);
    }
}

fn cleanup_runtime_html_path(path: &PathBuf) {
    if let Err(error) = std::fs::remove_file(path) {
        debug!(
            path = %path.display(),
            %error,
            "Failed to remove temporary runtime HTML source"
        );
    }
}

impl EffectRenderer for ServoRenderer {
    fn init(&mut self, metadata: &EffectMetadata) -> Result<()> {
        self.initialize_with_canvas_size(metadata, DEFAULT_CANVAS_WIDTH, DEFAULT_CANVAS_HEIGHT)
    }

    fn init_with_canvas_size(
        &mut self,
        metadata: &EffectMetadata,
        canvas_width: u32,
        canvas_height: u32,
    ) -> Result<()> {
        self.initialize_with_canvas_size(metadata, canvas_width, canvas_height)
    }

    fn render_into(&mut self, input: &FrameInput<'_>, target: &mut Canvas) -> Result<()> {
        if !self.initialized {
            bail!("ServoRenderer tick called before init");
        }

        self.poll_load_task();
        self.queue_frame(input);
        self.poll_in_flight_render();
        self.try_submit_queued_frame();

        if let Some(canvas) = self.last_canvas.as_ref()
            && canvas.width() == input.canvas_width
            && canvas.height() == input.canvas_height
        {
            copy_completed_canvas_into_target(canvas, target);
        } else {
            Self::render_placeholder_into(target, input);
        }
        Ok(())
    }

    #[cfg(feature = "servo-gpu-import")]
    fn render_output(&mut self, input: &FrameInput<'_>) -> Result<EffectRenderOutput> {
        if !self.initialized {
            bail!("ServoRenderer tick called before init");
        }

        self.poll_load_task();
        self.queue_frame(input);
        self.poll_in_flight_render_output();
        self.try_submit_queued_frame_with_gpu_preference(super::servo_gpu_import_should_attempt());

        if let Some(frame) = self.last_gpu_frame.as_ref()
            && frame.width == input.canvas_width
            && frame.height == input.canvas_height
        {
            return Ok(EffectRenderOutput::Gpu(frame.clone()));
        }

        if let Some(canvas) = self.last_canvas.as_ref()
            && canvas.width() == input.canvas_width
            && canvas.height() == input.canvas_height
        {
            return Ok(EffectRenderOutput::Cpu(canvas.clone()));
        }

        let mut placeholder = Canvas::new(input.canvas_width, input.canvas_height);
        Self::render_placeholder_into(&mut placeholder, input);
        Ok(EffectRenderOutput::Cpu(placeholder))
    }

    fn set_control(&mut self, name: &str, value: &ControlValue) {
        self.controls.insert(name.to_owned(), value.clone());
    }

    fn destroy(&mut self) {
        self.load_task = None;
        if let Some(session) = self.session.take() {
            recycle_servo_session(session, "Servo effect session");
        }
        self.pending_scripts.clear();
        self.queued_frame = None;
        self.last_canvas = None;
        #[cfg(feature = "servo-gpu-import")]
        {
            self.last_gpu_frame = None;
        }
        self.controls.clear();
        self.html_source = None;
        self.html_resolved_path = None;
        self.cleanup_runtime_html();
        self.initialized = false;
        self.load_failed = None;
        self.warned_fallback_frame = false;
        self.warned_stalled_frame = false;
        self.include_audio_updates = true;
        self.include_screen_updates = false;
        self.include_sensor_updates = false;
        self.last_animation_fps_cap = None;
        self.animation_cadence = AnimationCadence::MatchRenderLoop;
        self.last_submit_time_secs = None;
    }
}

impl Default for ServoRenderer {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone)]
struct QueuedFrameInput {
    time_secs: f32,
    delta_secs: f32,
    frame_number: u64,
    audio: hypercolor_types::audio::AudioData,
    interaction: crate::input::InteractionData,
    screen: Option<crate::input::ScreenData>,
    sensors: SystemSnapshot,
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
            screen: input.screen.cloned(),
            sensors: input.sensors.clone(),
            canvas_width: input.canvas_width,
            canvas_height: input.canvas_height,
        }
    }

    fn merge_from_input(&mut self, input: &FrameInput<'_>) {
        let prior_recent_keys = std::mem::take(&mut self.interaction.keyboard.recent_keys);
        self.time_secs = input.time_secs;
        self.delta_secs = input.delta_secs;
        self.frame_number = input.frame_number;
        self.audio.clone_from(input.audio);
        self.interaction.clone_from(input.interaction);
        match (&mut self.screen, input.screen) {
            (Some(current), Some(next)) => current.clone_from(next),
            (slot, Some(next)) => *slot = Some(next.clone()),
            (slot, None) => *slot = None,
        }
        self.sensors.clone_from(input.sensors);
        merge_unique_strings(
            &mut self.interaction.keyboard.recent_keys,
            prior_recent_keys,
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
            screen: self.screen.as_ref(),
            sensors: &self.sensors,
            canvas_width: self.canvas_width,
            canvas_height: self.canvas_height,
        }
    }
}

fn merge_unique_strings(destination: &mut Vec<String>, values: impl IntoIterator<Item = String>) {
    for value in values {
        if destination.iter().any(|existing| existing == &value) {
            continue;
        }
        destination.push(value);
    }
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
    FpsTier::from_fps((fps as u32).clamp(1, MAX_EFFECT_FPS_CAP)).fps()
}

fn animation_cadence(metadata: &EffectMetadata) -> AnimationCadence {
    if metadata.category == EffectCategory::Display {
        return AnimationCadence::Fixed(DEFAULT_DISPLAY_FPS_CAP);
    }

    AnimationCadence::MatchRenderLoop
}

fn effect_uses_sensor_data(metadata: &EffectMetadata) -> bool {
    metadata.category == EffectCategory::Display
        || metadata
            .tags
            .iter()
            .any(|tag| tag == "sensor" || tag == "sensors" || tag == "system-monitor")
        || metadata
            .controls
            .iter()
            .any(|control| matches!(control.kind, ControlKind::Sensor))
}

fn host_driven_animation(metadata: &EffectMetadata) -> bool {
    metadata.category != EffectCategory::Display
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effect::servo::worker::{
        install_running_shared_worker, reset_shared_servo_worker_state,
        shutdown_shared_servo_worker,
        test_support::{
            SHARED_WORKER_STATE_TEST_LOCK, spawn_blocking_load_test_worker, spawn_load_test_worker,
            spawn_render_test_worker, spawn_test_worker, worker_client_from,
        },
    };
    use hypercolor_types::audio::AudioData;
    use hypercolor_types::effect::{
        ControlDefinition, ControlType, EffectCategory, EffectId, EffectSource,
    };
    use hypercolor_types::sensor::SystemSnapshot;
    use std::sync::LazyLock;
    use std::sync::atomic::Ordering;
    use std::thread;
    use std::time::{Duration, Instant};
    use uuid::Uuid;

    static SILENCE: LazyLock<AudioData> = LazyLock::new(AudioData::silence);
    static DEFAULT_INTERACTION: LazyLock<crate::input::InteractionData> =
        LazyLock::new(crate::input::InteractionData::default);
    static EMPTY_SENSORS: LazyLock<SystemSnapshot> = LazyLock::new(SystemSnapshot::empty);
    static SOFT_STALL_TELEMETRY_TEST_LOCK: LazyLock<std::sync::Mutex<()>> =
        LazyLock::new(std::sync::Mutex::default);

    fn frame_input(delta_secs: f32) -> FrameInput<'static> {
        FrameInput {
            time_secs: 0.0,
            delta_secs,
            frame_number: 0,
            audio: &SILENCE,
            interaction: &DEFAULT_INTERACTION,
            screen: None,
            sensors: &EMPTY_SENSORS,
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
            screen: None,
            sensors: &EMPTY_SENSORS,
            canvas_width,
            canvas_height,
        }
    }

    fn solid_canvas(width: u32, height: u32, r: u8, g: u8, b: u8) -> Canvas {
        let mut canvas = Canvas::new(width, height);
        canvas.fill(Rgba::new(r, g, b, 255));
        canvas
    }

    fn wait_for_load_completion(renderer: &mut ServoRenderer) {
        for _ in 0..20 {
            renderer.poll_load_task();
            if renderer.load_task.is_none() {
                return;
            }
            thread::sleep(Duration::from_millis(10));
        }
        panic!("Servo load task should complete");
    }

    fn html_metadata(path: PathBuf) -> EffectMetadata {
        EffectMetadata {
            id: EffectId::from(Uuid::nil()),
            name: "HTML Test".to_owned(),
            author: "hypercolor".to_owned(),
            version: "0.1.0".to_owned(),
            description: "test".to_owned(),
            category: EffectCategory::Interactive,
            tags: Vec::new(),
            controls: Vec::new(),
            presets: Vec::new(),
            audio_reactive: false,
            screen_reactive: false,
            source: EffectSource::Html { path },
            license: None,
        }
    }

    fn display_html_metadata(path: PathBuf) -> EffectMetadata {
        let mut metadata = html_metadata(path);
        metadata.category = EffectCategory::Display;
        metadata
    }

    fn attach_renderer_session(
        renderer: &mut ServoRenderer,
        worker: &crate::effect::servo::worker::ServoWorker,
    ) {
        let mut session = ServoSessionHandle::new(
            worker_client_from(worker),
            SessionConfig {
                render_width: DEFAULT_CANVAS_WIDTH,
                render_height: DEFAULT_CANVAS_HEIGHT,
                inject_engine_globals: true,
            },
        )
        .expect("test session should initialize");
        session
            .load_html_file(std::path::Path::new("test.html"))
            .expect("test session should load");
        renderer.session = Some(session);
    }

    #[test]
    fn destroy_clears_renderer_state_without_shutting_down_shared_worker() {
        let (worker, stopped) = spawn_test_worker();

        let mut renderer = ServoRenderer::new();
        attach_renderer_session(&mut renderer, &worker);
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
        renderer
            .session
            .as_mut()
            .expect("attached test session")
            .request_render(Vec::new())
            .expect("test render should queue");
        renderer.last_canvas = Some(solid_canvas(
            DEFAULT_CANVAS_WIDTH,
            DEFAULT_CANVAS_HEIGHT,
            1,
            2,
            3,
        ));

        renderer.destroy();

        assert!(!stopped.load(Ordering::SeqCst));
        assert!(renderer.session.is_none());
        assert!(renderer.pending_scripts.is_empty());
        assert!(renderer.queued_frame.is_none());
        assert!(renderer.last_canvas.is_none());
        assert!(renderer.controls.is_empty());
        assert!(renderer.html_source.is_none());
        assert!(renderer.html_resolved_path.is_none());
        assert!(renderer.runtime_html_path.is_none());
        assert!(!renderer.initialized);
        assert!(!renderer.warned_fallback_frame);
        assert!(!renderer.warned_stalled_frame);
        assert!(renderer.include_audio_updates);
        assert!(!renderer.include_sensor_updates);

        drop(worker);
        assert!(stopped.load(Ordering::SeqCst));
    }

    #[test]
    fn bootstrap_scripts_track_default_animation_cap_without_js_throttle() {
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
                .all(|script| !script.contains("__hypercolorFpsCap"))
        );
    }

    #[test]
    fn display_animation_cadence_stays_fixed_at_30_fps() {
        let metadata = display_html_metadata(PathBuf::from("display.html"));

        assert_eq!(animation_cadence(&metadata), AnimationCadence::Fixed(30));
        assert_eq!(animation_cadence(&metadata).fps_cap(1.0 / 60.0), 30);
        assert_eq!(animation_cadence(&metadata).fps_cap(1.0 / 20.0), 30);
    }

    #[test]
    fn sensor_updates_are_limited_to_sensor_aware_metadata() {
        let plain = html_metadata(PathBuf::from("bubble.html"));
        assert!(!effect_uses_sensor_data(&plain));

        let display = display_html_metadata(PathBuf::from("face.html"));
        assert!(effect_uses_sensor_data(&display));

        let mut sensor_control = html_metadata(PathBuf::from("sensor.html"));
        sensor_control.controls.push(ControlDefinition {
            id: "targetSensor".to_owned(),
            name: "Sensor".to_owned(),
            kind: ControlKind::Sensor,
            control_type: ControlType::Dropdown,
            default_value: ControlValue::Enum("cpu_temp".to_owned()),
            min: None,
            max: None,
            step: None,
            labels: vec!["cpu_temp".to_owned()],
            group: None,
            tooltip: None,
            aspect_lock: None,
            preview_source: None,
            binding: None,
        });
        assert!(effect_uses_sensor_data(&sensor_control));
    }

    #[test]
    fn fixed_animation_cadence_waits_for_next_due_frame() {
        let cadence = AnimationCadence::Fixed(30);

        assert!(cadence.render_due(None, 0.0));
        assert!(!cadence.render_due(Some(0.0), 0.01));
        assert!(cadence.render_due(Some(0.0), 1.0 / 30.0));
        assert!(cadence.render_due(Some(0.0), 0.05));
    }

    #[test]
    fn take_pending_scripts_preserves_capacity() {
        let mut renderer = ServoRenderer::new();
        renderer.pending_scripts = Vec::with_capacity(8);
        renderer.pending_scripts.push("tick()".to_owned());

        let capacity = renderer.pending_scripts.capacity();
        let scripts = renderer.take_pending_scripts();

        assert_eq!(scripts, vec!["tick()"]);
        assert!(renderer.pending_scripts.is_empty());
        assert!(renderer.pending_scripts.capacity() >= capacity);
    }

    #[test]
    fn init_with_canvas_size_returns_before_servo_session_create_completes() {
        let _lock = SHARED_WORKER_STATE_TEST_LOCK
            .lock()
            .expect("shared worker test lock");
        reset_shared_servo_worker_state();

        let (worker, load_rx, release_tx, unload_rx, stopped) = spawn_blocking_load_test_worker();
        install_running_shared_worker(worker);

        let temp_dir = tempfile::tempdir().expect("temporary directory");
        let source_path = temp_dir.path().join("effect.html");
        std::fs::write(&source_path, "<!doctype html><html><body></body></html>")
            .expect("write source html");

        let metadata = html_metadata(source_path);
        let mut renderer = ServoRenderer::new();
        let started_at = Instant::now();
        renderer
            .init_with_canvas_size(&metadata, 640, 480)
            .expect("renderer should queue initialization");

        assert!(started_at.elapsed() < Duration::from_millis(50));
        assert!(renderer.load_task.is_some());
        assert!(renderer.session.is_none());

        let load = load_rx
            .recv_timeout(Duration::from_millis(100))
            .expect("create-session command should be queued asynchronously");
        assert_eq!(load.width, 640);
        assert_eq!(load.height, 480);

        release_tx.send(()).expect("release create-session");
        wait_for_load_completion(&mut renderer);
        assert!(renderer.session.is_some());

        renderer.destroy();
        unload_rx
            .recv_timeout(Duration::from_millis(100))
            .expect("destroy should unload test worker");

        shutdown_shared_servo_worker().expect("shared worker shutdown should succeed");
        assert!(stopped.load(Ordering::SeqCst));
    }

    #[test]
    fn render_into_uses_placeholder_while_servo_load_is_pending() {
        let _lock = SHARED_WORKER_STATE_TEST_LOCK
            .lock()
            .expect("shared worker test lock");
        reset_shared_servo_worker_state();

        let (worker, load_rx, release_tx, unload_rx, stopped) = spawn_blocking_load_test_worker();
        install_running_shared_worker(worker);

        let temp_dir = tempfile::tempdir().expect("temporary directory");
        let source_path = temp_dir.path().join("effect.html");
        std::fs::write(&source_path, "<!doctype html><html><body></body></html>")
            .expect("write source html");

        let metadata = html_metadata(source_path);
        let mut renderer = ServoRenderer::new();
        renderer
            .init_with_canvas_size(&metadata, 640, 480)
            .expect("renderer should queue initialization");

        load_rx
            .recv_timeout(Duration::from_millis(100))
            .expect("create-session command should be queued asynchronously");

        let audio = custom_audio(0.5);
        let interaction = custom_interaction(&[], &[]);
        let input = frame_input_with(1.0 / 30.0, 7, &audio, &interaction, 4, 3);
        let mut target = Canvas::new(1, 1);
        let started_at = Instant::now();

        renderer
            .render_into(&input, &mut target)
            .expect("placeholder render should succeed while Servo load is pending");

        assert!(started_at.elapsed() < Duration::from_millis(20));
        assert!(renderer.load_task.is_some());
        assert!(renderer.session.is_none());
        assert_eq!(target.width(), 4);
        assert_eq!(target.height(), 3);
        assert_eq!(target.get_pixel(0, 0), Rgba::new(7, 127, 39, 255));

        release_tx.send(()).expect("release create-session");
        wait_for_load_completion(&mut renderer);

        renderer.destroy();
        unload_rx
            .recv_timeout(Duration::from_millis(100))
            .expect("destroy should unload test worker");

        shutdown_shared_servo_worker().expect("shared worker shutdown should succeed");
        assert!(stopped.load(Ordering::SeqCst));
    }

    #[test]
    fn destroy_discards_completed_load_task_before_it_is_polled() {
        let (worker, load_rx, unload_rx, stopped) = spawn_load_test_worker();
        let mut session = ServoSessionHandle::new(
            worker_client_from(&worker),
            SessionConfig {
                render_width: 640,
                render_height: 480,
                inject_engine_globals: true,
            },
        )
        .expect("test session should initialize");

        load_rx
            .recv_timeout(Duration::from_millis(100))
            .expect("create-session command should be queued");
        session
            .load_html_file(std::path::Path::new("test.html"))
            .expect("test session should load");

        let (response_tx, response_rx) = std::sync::mpsc::sync_channel(1);
        response_tx
            .send(Ok(LoadedServoSession {
                session,
                runtime_source: PathBuf::from("runtime.html"),
                runtime_html_path: None,
            }))
            .expect("completed load should queue");

        let mut renderer = ServoRenderer::new();
        renderer.load_task = Some(ServoLoadTask {
            response_rx,
            shared: Arc::new(Mutex::new(ServoLoadTaskState { canceled: false })),
            started_at: Instant::now(),
        });

        renderer.destroy();
        unload_rx
            .recv_timeout(Duration::from_millis(100))
            .expect("completed background load should be detached during destroy");

        drop(worker);
        assert!(stopped.load(Ordering::SeqCst));
    }

    #[test]
    fn init_with_canvas_size_loads_servo_page_at_target_resolution() {
        let _lock = SHARED_WORKER_STATE_TEST_LOCK
            .lock()
            .expect("shared worker test lock");
        reset_shared_servo_worker_state();

        let (worker, load_rx, unload_rx, stopped) = spawn_load_test_worker();
        install_running_shared_worker(worker);

        let temp_dir = tempfile::tempdir().expect("temporary directory");
        let source_path = temp_dir.path().join("effect.html");
        std::fs::write(&source_path, "<!doctype html><html><body></body></html>")
            .expect("write source html");

        let metadata = html_metadata(source_path);
        let mut renderer = ServoRenderer::new();
        renderer
            .init_with_canvas_size(&metadata, 640, 480)
            .expect("renderer should initialize");

        let load = load_rx
            .recv_timeout(Duration::from_millis(100))
            .expect("load command should be recorded");
        assert_eq!(load.width, 640);
        assert_eq!(load.height, 480);
        wait_for_load_completion(&mut renderer);
        assert!(
            renderer
                .pending_scripts
                .iter()
                .any(|script| script.contains("window.engine.width = 640"))
        );
        assert!(
            renderer
                .pending_scripts
                .iter()
                .any(|script| script.contains("window.engine.height = 480"))
        );

        renderer.destroy();
        unload_rx
            .recv_timeout(Duration::from_millis(100))
            .expect("destroy should unload test worker");

        shutdown_shared_servo_worker().expect("shared worker shutdown should succeed");
        assert!(stopped.load(Ordering::SeqCst));
    }

    #[test]
    fn frame_scripts_track_animation_cap_without_js_throttle() {
        let mut renderer = ServoRenderer::new();
        renderer.enqueue_bootstrap_scripts();
        renderer.pending_scripts.clear();

        renderer.enqueue_frame_scripts(&frame_input(1.0 / 30.0));
        assert_eq!(renderer.last_animation_fps_cap, Some(30));
        assert!(
            renderer
                .pending_scripts
                .iter()
                .all(|script| !script.contains("__hypercolorFpsCap"))
        );

        renderer.pending_scripts.clear();
        renderer.enqueue_frame_scripts(&frame_input(1.0 / 15.0));
        assert_eq!(renderer.last_animation_fps_cap, Some(20));
        assert!(
            renderer
                .pending_scripts
                .iter()
                .all(|script| !script.contains("__hypercolorFpsCap"))
        );
    }

    #[test]
    fn frame_scripts_drive_sdk_render_with_static_host_script() {
        let mut renderer = ServoRenderer::new();
        let mut input = frame_input(1.0 / 30.0);
        input.time_secs = 2.5;

        renderer.enqueue_frame_scripts(&input);

        assert!(
            renderer
                .pending_scripts
                .iter()
                .any(|script| script.contains("window.__hypercolorRenderHostFrame"))
        );
        assert!(
            renderer
                .pending_scripts
                .iter()
                .all(|script| !script.contains("instance.render(2.5)"))
        );
    }

    #[test]
    fn display_frame_scripts_keep_fixed_animation_cap() {
        let mut renderer = ServoRenderer::new();
        renderer.animation_cadence = AnimationCadence::Fixed(30);
        renderer.host_driven_animation = false;
        renderer.enqueue_bootstrap_scripts();
        renderer.pending_scripts.clear();

        renderer.enqueue_frame_scripts(&frame_input(1.0 / 60.0));

        assert_eq!(renderer.last_animation_fps_cap, Some(30));
        assert!(
            renderer
                .pending_scripts
                .iter()
                .all(|script| !script.contains("__hypercolorFpsCap"))
        );
        assert!(
            renderer
                .pending_scripts
                .iter()
                .all(|script| !script.contains("instance.render"))
        );
    }

    #[test]
    fn soft_stall_timeout_tracks_active_animation_cap() {
        let mut renderer = ServoRenderer::new();

        assert_eq!(
            renderer.soft_stall_timeout(),
            FpsTier::Medium
                .frame_interval()
                .mul_f32(SOFT_STALL_FRAME_INTERVALS as f32)
        );

        renderer.last_animation_fps_cap = Some(60);
        assert_eq!(
            renderer.soft_stall_timeout(),
            FpsTier::Full
                .frame_interval()
                .mul_f32(SOFT_STALL_FRAME_INTERVALS as f32)
        );

        renderer.last_animation_fps_cap = Some(10);
        assert_eq!(
            renderer.soft_stall_timeout(),
            FpsTier::Minimal
                .frame_interval()
                .mul_f32(SOFT_STALL_FRAME_INTERVALS as f32)
        );
    }

    #[test]
    fn poll_in_flight_render_marks_soft_stall_before_hard_timeout() {
        let _soft_stall_guard = SOFT_STALL_TELEMETRY_TEST_LOCK.lock().expect("lock");
        let (worker, render_rx, result_tx, delivered_rx, _unload_rx, stopped) =
            spawn_render_test_worker();
        let baseline_stalls = crate::effect::servo::servo_telemetry_snapshot().soft_stalls_total;

        let mut renderer = ServoRenderer::new();
        attach_renderer_session(&mut renderer, &worker);
        renderer.initialized = true;
        renderer.last_animation_fps_cap = Some(60);
        renderer.last_canvas = Some(solid_canvas(
            DEFAULT_CANVAS_WIDTH,
            DEFAULT_CANVAS_HEIGHT,
            20,
            40,
            60,
        ));
        renderer
            .session
            .as_mut()
            .expect("attached test session")
            .request_render(Vec::new())
            .expect("test render should queue");
        let _ = render_rx
            .recv_timeout(Duration::from_millis(100))
            .expect("render command");

        thread::sleep(renderer.soft_stall_timeout() + Duration::from_millis(25));
        renderer.poll_in_flight_render();

        assert!(renderer.warned_stalled_frame);
        assert_eq!(
            crate::effect::servo::servo_telemetry_snapshot().soft_stalls_total,
            baseline_stalls + 1
        );

        renderer.poll_in_flight_render();
        assert_eq!(
            crate::effect::servo::servo_telemetry_snapshot().soft_stalls_total,
            baseline_stalls + 1
        );

        result_tx
            .send(Ok(solid_canvas(
                DEFAULT_CANVAS_WIDTH,
                DEFAULT_CANVAS_HEIGHT,
                1,
                1,
                1,
            )))
            .expect("cleanup render result");
        delivered_rx
            .recv_timeout(Duration::from_millis(100))
            .expect("cleanup result delivery ack");

        drop(worker);
        assert!(stopped.load(Ordering::SeqCst));
    }

    #[test]
    fn poll_in_flight_render_clears_stall_warning_after_completed_frame() {
        let _soft_stall_guard = SOFT_STALL_TELEMETRY_TEST_LOCK.lock().expect("lock");
        let (worker, render_rx, result_tx, delivered_rx, _unload_rx, stopped) =
            spawn_render_test_worker();

        let mut renderer = ServoRenderer::new();
        attach_renderer_session(&mut renderer, &worker);
        renderer.initialized = true;
        renderer.last_animation_fps_cap = Some(60);
        renderer.last_canvas = Some(solid_canvas(
            DEFAULT_CANVAS_WIDTH,
            DEFAULT_CANVAS_HEIGHT,
            20,
            40,
            60,
        ));
        renderer
            .session
            .as_mut()
            .expect("attached test session")
            .request_render(Vec::new())
            .expect("test render should queue");
        let _ = render_rx
            .recv_timeout(Duration::from_millis(100))
            .expect("render command");

        thread::sleep(renderer.soft_stall_timeout() + Duration::from_millis(25));
        renderer.poll_in_flight_render();
        assert!(renderer.warned_stalled_frame);

        result_tx
            .send(Ok(solid_canvas(
                DEFAULT_CANVAS_WIDTH,
                DEFAULT_CANVAS_HEIGHT,
                9,
                8,
                7,
            )))
            .expect("completed render result");
        delivered_rx
            .recv_timeout(Duration::from_millis(100))
            .expect("completed result delivery ack");

        renderer.poll_in_flight_render();

        assert!(!renderer.warned_stalled_frame);
        assert_eq!(
            renderer
                .last_canvas
                .as_ref()
                .expect("completed frame")
                .get_pixel(0, 0),
            Rgba::new(9, 8, 7, 255)
        );

        drop(worker);
        assert!(stopped.load(Ordering::SeqCst));
    }

    #[test]
    fn frame_scripts_skip_near_tier_jitter_updates() {
        let mut renderer = ServoRenderer::new();
        renderer.enqueue_bootstrap_scripts();
        renderer.pending_scripts.clear();

        renderer.enqueue_frame_scripts(&frame_input(1.0 / 60.0));
        assert_eq!(renderer.last_animation_fps_cap, Some(60));
        assert!(
            renderer
                .pending_scripts
                .iter()
                .all(|script| !script.contains("__hypercolorFpsCap"))
        );

        renderer.pending_scripts.clear();
        renderer.enqueue_frame_scripts(&frame_input(1.0 / 58.0));
        assert_eq!(renderer.last_animation_fps_cap, Some(60));
        assert!(
            renderer
                .pending_scripts
                .iter()
                .all(|script| !script.contains("__hypercolorFpsCap"))
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
    fn render_into_without_completed_frame_fills_placeholder_target() {
        let mut renderer = ServoRenderer::new();
        renderer.initialized = true;

        let audio = custom_audio(0.5);
        let interaction = custom_interaction(&[], &[]);
        let input = frame_input_with(1.0 / 30.0, 7, &audio, &interaction, 4, 3);
        let mut target = Canvas::new(1, 1);

        renderer
            .render_into(&input, &mut target)
            .expect("placeholder render should succeed");

        assert_eq!(target.width(), 4);
        assert_eq!(target.height(), 3);
        assert_eq!(target.get_pixel(0, 0), Rgba::new(7, 127, 39, 255));
        assert_eq!(target.get_pixel(3, 2), Rgba::new(7, 127, 39, 255));
    }

    #[test]
    fn render_into_ignores_completed_frame_with_stale_dimensions() {
        let mut renderer = ServoRenderer::new();
        renderer.initialized = true;
        renderer.last_canvas = Some(Canvas::new(2, 2));

        let audio = custom_audio(0.5);
        let interaction = custom_interaction(&[], &[]);
        let input = frame_input_with(1.0 / 30.0, 7, &audio, &interaction, 4, 3);
        let mut target = Canvas::new(1, 1);

        renderer
            .render_into(&input, &mut target)
            .expect("placeholder render should succeed");

        assert_eq!(target.width(), 4);
        assert_eq!(target.height(), 3);
        assert_eq!(target.get_pixel(0, 0), Rgba::new(7, 127, 39, 255));
    }

    #[test]
    fn render_into_copies_completed_frame_into_existing_target_storage() {
        let mut renderer = ServoRenderer::new();
        renderer.initialized = true;
        renderer.last_canvas = Some(solid_canvas(4, 3, 9, 8, 7));

        let audio = custom_audio(0.5);
        let interaction = custom_interaction(&[], &[]);
        let input = frame_input_with(1.0 / 30.0, 7, &audio, &interaction, 4, 3);
        let mut target = Canvas::new(4, 3);
        let target_ptr = target.as_rgba_bytes().as_ptr();

        renderer
            .render_into(&input, &mut target)
            .expect("completed frame render should succeed");

        assert_eq!(target.as_rgba_bytes().as_ptr(), target_ptr);
        assert_eq!(target.get_pixel(0, 0), Rgba::new(9, 8, 7, 255));
        assert_eq!(target.get_pixel(3, 2), Rgba::new(9, 8, 7, 255));
    }

    #[test]
    fn queued_frames_submit_latest_state_after_in_flight_render_finishes() {
        let (worker, render_rx, result_tx, delivered_rx, _unload_rx, stopped) =
            spawn_render_test_worker();

        let mut renderer = ServoRenderer::new();
        attach_renderer_session(&mut renderer, &worker);
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
            .send(Ok(solid_canvas(640, 360, 9, 8, 7)))
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
                .all(|script| !script.contains("__hypercolorFpsCap"))
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
            .send(Ok(solid_canvas(640, 360, 1, 1, 1)))
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
        attach_renderer_session(&mut renderer, &worker);
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
            .send(Ok(solid_canvas(320, 200, 20, 40, 60)))
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
            .send(Ok(solid_canvas(320, 200, 1, 1, 1)))
            .expect("cleanup render result");
        delivered_rx
            .recv_timeout(Duration::from_millis(100))
            .expect("cleanup result delivery ack");

        drop(worker);
        assert!(stopped.load(Ordering::SeqCst));
    }

    #[test]
    fn destroy_detaches_in_flight_render_before_unloading_worker_page() {
        let (worker, render_rx, result_tx, delivered_rx, unload_rx, stopped) =
            spawn_render_test_worker();

        let mut renderer = ServoRenderer::new();
        attach_renderer_session(&mut renderer, &worker);
        renderer.initialized = true;
        renderer.enqueue_bootstrap_scripts();

        let interaction = custom_interaction(&[], &[]);
        let audio = custom_audio(0.0);
        let frame = frame_input_with(1.0 / 30.0, 1, &audio, &interaction, 320, 200);

        renderer.tick(&frame).expect("initial tick");
        let _ = render_rx
            .recv_timeout(Duration::from_millis(100))
            .expect("first render command");

        let started_at = std::time::Instant::now();

        renderer.destroy();

        assert!(started_at.elapsed() < Duration::from_millis(20));
        assert!(unload_rx.recv_timeout(Duration::from_millis(20)).is_err());
        result_tx
            .send(Ok(solid_canvas(
                DEFAULT_CANVAS_WIDTH,
                DEFAULT_CANVAS_HEIGHT,
                7,
                8,
                9,
            )))
            .expect("cleanup render result");
        delivered_rx
            .recv_timeout(Duration::from_millis(100))
            .expect("cleanup result delivery ack");
        unload_rx
            .recv_timeout(Duration::from_millis(100))
            .expect("destroy should unload the active Servo page");

        drop(worker);
        assert!(stopped.load(Ordering::SeqCst));
    }
}
