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
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Instant;
use tracing::{debug, info, warn};

#[cfg(test)]
use super::session::ServoRenderSubmission;
use super::telemetry::{
    record_servo_detached_destroy, record_servo_page_load, record_servo_renderer_load,
    record_servo_session_create,
};
use super::worker::{effect_is_audio_reactive, prepare_runtime_html_source};
use super::worker_client::{ServoFramePayload, ServoProducerRole};
use super::{ServoSessionHandle, SessionConfig, note_servo_session_error};
use crate::effect::lightscript::LightscriptRuntime;
use crate::effect::paths::resolve_html_source_path;
#[cfg(feature = "servo-gpu-import")]
use crate::effect::traits::{EffectRenderOutput, ImportedEffectFrame};
use crate::effect::traits::{EffectRenderer, FrameInput, prepare_target_canvas};
#[cfg(test)]
use crate::engine::FpsTier;
use frame_queue::{AnimationCadence, QueuedFrameInput, animation_cadence};

const DEFAULT_EFFECT_FPS_CAP: u32 = 30;
const DEFAULT_DISPLAY_FPS_CAP: u32 = 30;
const MAX_EFFECT_FPS_CAP: u32 = 60;
const SOFT_STALL_FRAME_INTERVALS: u32 = 5;

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
    pending_frame_payloads: Vec<ServoFramePayload>,
    session: Option<ServoSessionHandle>,
    load_task: Option<ServoLoadTask>,
    load_failed: Option<String>,
    queued_frame: Option<QueuedFrameInput>,
    last_canvas: Option<Canvas>,
    #[cfg(feature = "servo-gpu-import")]
    last_gpu_frame: Option<ImportedEffectFrame>,
    #[cfg(feature = "servo-gpu-import")]
    reuse_cached_gpu_frame_on_no_ready: bool,
    warned_fallback_frame: bool,
    warned_stalled_frame: bool,
    include_audio_updates: bool,
    include_screen_updates: bool,
    include_sensor_updates: bool,
    scoped_sensor_control_ids: Vec<String>,
    include_interaction_updates: bool,
    last_animation_fps_cap: Option<u32>,
    animation_cadence: AnimationCadence,
    host_driven_animation: bool,
    last_submit_time_secs: Option<f32>,
    producer_role: ServoProducerRole,
}

impl ServoRenderer {
    /// Create a new Servo renderer instance.
    #[must_use]
    pub fn new() -> Self {
        Self::new_with_producer_role(ServoProducerRole::SceneHtml)
    }

    /// Create a Servo renderer dedicated to display-face HTML producers.
    #[must_use]
    pub fn new_display_face() -> Self {
        Self::new_with_producer_role(ServoProducerRole::DisplayFaceHtml)
    }

    fn new_with_producer_role(producer_role: ServoProducerRole) -> Self {
        Self {
            html_source: None,
            html_resolved_path: None,
            runtime_html_path: None,
            controls: HashMap::new(),
            runtime: LightscriptRuntime::new(DEFAULT_CANVAS_WIDTH, DEFAULT_CANVAS_HEIGHT),
            initialized: false,
            pending_scripts: Vec::new(),
            pending_frame_payloads: Vec::new(),
            session: None,
            load_task: None,
            load_failed: None,
            queued_frame: None,
            last_canvas: None,
            #[cfg(feature = "servo-gpu-import")]
            last_gpu_frame: None,
            #[cfg(feature = "servo-gpu-import")]
            reuse_cached_gpu_frame_on_no_ready: false,
            warned_fallback_frame: false,
            warned_stalled_frame: false,
            include_audio_updates: true,
            include_screen_updates: false,
            include_sensor_updates: false,
            scoped_sensor_control_ids: Vec::new(),
            include_interaction_updates: false,
            last_animation_fps_cap: None,
            animation_cadence: AnimationCadence::MatchRenderLoop,
            host_driven_animation: false,
            last_submit_time_secs: None,
            producer_role,
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

        let previous_canvas = self
            .last_canvas
            .take()
            .filter(|canvas| canvas.width() == canvas_width && canvas.height() == canvas_height);
        #[cfg(feature = "servo-gpu-import")]
        let previous_gpu_frame = self
            .last_gpu_frame
            .take()
            .filter(|frame| frame.width == canvas_width && frame.height == canvas_height);

        self.destroy();
        self.cleanup_runtime_html();
        self.session = None;
        self.load_task = None;
        self.load_failed = None;
        self.controls.clear();
        self.runtime = LightscriptRuntime::new(canvas_width, canvas_height);
        self.pending_scripts.clear();
        self.pending_frame_payloads.clear();
        self.warned_fallback_frame = false;
        self.warned_stalled_frame = false;
        self.include_audio_updates = effect_is_audio_reactive(metadata);
        self.include_screen_updates = metadata.screen_reactive;
        self.include_sensor_updates = effect_uses_sensor_data(metadata);
        self.scoped_sensor_control_ids = scoped_sensor_control_ids(metadata);
        self.include_interaction_updates = effect_uses_interaction_data(metadata);
        #[cfg(feature = "servo-gpu-import")]
        {
            self.reuse_cached_gpu_frame_on_no_ready =
                should_reuse_cached_gpu_frame_on_no_ready(metadata);
        }
        self.last_animation_fps_cap = None;
        self.animation_cadence = animation_cadence(metadata);
        self.host_driven_animation = host_driven_animation(metadata);
        self.last_submit_time_secs = None;
        self.queued_frame = None;
        self.last_canvas = previous_canvas;
        #[cfg(feature = "servo-gpu-import")]
        {
            self.last_gpu_frame = previous_gpu_frame;
        }
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
            self.producer_role,
            self.host_driven_animation,
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
                record_servo_renderer_load(started_at.elapsed(), true);
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
                if let Some(task) = self.load_task.as_ref() {
                    record_servo_renderer_load(task.started_at.elapsed(), false);
                }
                self.load_task = None;
                let message = error.to_string();
                if self.load_failed.as_deref() != Some(message.as_str()) {
                    warn!(%error, "ServoRenderer load failed; rendering placeholder frames");
                }
                self.load_failed = Some(message);
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => {
                if let Some(task) = self.load_task.as_ref() {
                    record_servo_renderer_load(task.started_at.elapsed(), false);
                }
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
    producer_role: ServoProducerRole,
    host_driven_animation: bool,
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
                producer_role,
                host_driven_animation,
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
    producer_role: ServoProducerRole,
    host_driven_animation: bool,
) -> Result<LoadedServoSession> {
    use anyhow::Context;

    let resolved = resolve_html_source_path(&html_source).with_context(|| {
        format!(
            "failed to resolve HTML source for effect '{effect_name}' from '{}'",
            html_source.display()
        )
    })?;

    let (runtime_source, runtime_html_path) =
        prepare_runtime_html_source(&resolved, controls, host_driven_animation).with_context(
            || {
                format!(
                    "failed to prepare runtime HTML source for '{}'",
                    resolved.display()
                )
            },
        )?;

    let session_create_started = Instant::now();
    let mut session = match ServoSessionHandle::new_shared(SessionConfig {
        render_width: canvas_width,
        render_height: canvas_height,
        inject_engine_globals: true,
        producer_role,
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

        #[cfg(feature = "servo-gpu-import")]
        if self.reuse_cached_gpu_frame_on_no_ready {
            return Ok(EffectRenderOutput::Pending);
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
        self.pending_frame_payloads.clear();
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
        self.scoped_sensor_control_ids.clear();
        self.include_interaction_updates = false;
        #[cfg(feature = "servo-gpu-import")]
        {
            self.reuse_cached_gpu_frame_on_no_ready = false;
        }
        self.last_animation_fps_cap = None;
        self.animation_cadence = AnimationCadence::MatchRenderLoop;
        self.host_driven_animation = false;
        self.last_submit_time_secs = None;
    }
}

impl Default for ServoRenderer {
    fn default() -> Self {
        Self::new()
    }
}

fn effect_uses_sensor_data(metadata: &EffectMetadata) -> bool {
    metadata.tags.iter().any(|tag| {
        tag.eq_ignore_ascii_case("sensor")
            || tag.eq_ignore_ascii_case("sensors")
            || tag.eq_ignore_ascii_case("system-monitor")
    }) || metadata
        .controls
        .iter()
        .any(|control| matches!(control.kind, ControlKind::Sensor))
}

fn scoped_sensor_control_ids(metadata: &EffectMetadata) -> Vec<String> {
    let has_broad_sensor_tag = metadata.tags.iter().any(|tag| {
        tag.eq_ignore_ascii_case("sensor")
            || tag.eq_ignore_ascii_case("sensors")
            || tag.eq_ignore_ascii_case("system-monitor")
    });
    if has_broad_sensor_tag {
        return Vec::new();
    }

    metadata
        .controls
        .iter()
        .filter(|control| matches!(control.kind, ControlKind::Sensor))
        .map(|control| control.control_id().to_owned())
        .collect()
}

fn effect_uses_interaction_data(metadata: &EffectMetadata) -> bool {
    metadata.category == EffectCategory::Interactive
        || metadata.tags.iter().any(|tag| {
            tag.eq_ignore_ascii_case("interactive")
                || tag.eq_ignore_ascii_case("input")
                || tag.eq_ignore_ascii_case("mouse")
                || tag.eq_ignore_ascii_case("keyboard")
        })
}

fn host_driven_animation(metadata: &EffectMetadata) -> bool {
    metadata.category == EffectCategory::Display
}

#[cfg(feature = "servo-gpu-import")]
fn should_reuse_cached_gpu_frame_on_no_ready(metadata: &EffectMetadata) -> bool {
    metadata.category == EffectCategory::Display
}

mod frame_poll;
mod frame_queue;
#[cfg(test)]
mod tests;
