use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Instant;

use anyhow::{Context, Result, bail};
use hypercolor_types::display::DisplayDescriptor;
use hypercolor_types::effect::{ControlValue, EffectMetadata, EffectSource};
use tracing::{debug, info, warn};

use super::super::telemetry::{
    record_servo_detached_destroy, record_servo_page_load, record_servo_renderer_load,
    record_servo_session_create,
};
use super::super::worker::{effect_is_audio_reactive, prepare_runtime_html_source};
use super::super::worker_client::ServoProducerRole;
use super::super::{ServoSessionHandle, SessionConfig, note_servo_session_error};
use super::{
    ServoRenderer, animation_cadence, effect_uses_interaction_data, effect_uses_lighting_data,
    effect_uses_media_data, effect_uses_net_data, effect_uses_sensor_data, host_driven_animation,
    scoped_sensor_control_ids,
};
use crate::effect::lightscript::LightscriptRuntime;
use crate::effect::paths::resolve_html_source_path;
use crate::effect::traits::EffectRenderer;

pub(super) struct ServoLoadTask {
    pub(super) response_rx: Receiver<Result<LoadedServoSession>>,
    pub(super) shared: Arc<Mutex<ServoLoadTaskState>>,
    pub(super) started_at: Instant,
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

pub(super) struct ServoLoadTaskState {
    pub(super) canceled: bool,
}

pub(super) struct LoadedServoSession {
    pub(super) session: ServoSessionHandle,
    pub(super) runtime_source: PathBuf,
    pub(super) runtime_html_path: Option<PathBuf>,
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

impl ServoRenderer {
    pub(super) fn cleanup_runtime_html(&mut self) {
        if let Some(path) = self.runtime_html_path.take() {
            cleanup_runtime_html_path(&path);
        }
    }

    pub(super) fn initialize_with_canvas_size(
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
        self.command_queue_saturated = false;
        self.include_audio_updates = effect_is_audio_reactive(metadata);
        self.include_screen_updates = metadata.screen_reactive;
        self.include_sensor_updates = effect_uses_sensor_data(metadata);
        self.scoped_sensor_control_ids = scoped_sensor_control_ids(metadata);
        self.include_interaction_updates = effect_uses_interaction_data(metadata);
        self.include_media_updates = effect_uses_media_data(metadata);
        self.include_net_updates = effect_uses_net_data(metadata);
        self.include_lighting_updates = effect_uses_lighting_data(metadata);
        #[cfg(feature = "servo-gpu-import")]
        {
            self.reuse_cached_gpu_frame_on_no_ready =
                super::should_reuse_cached_gpu_frame_on_no_ready(metadata);
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
        let display_descriptor = (self.producer_role == ServoProducerRole::DisplayFaceHtml)
            .then(|| self.display_descriptor.clone())
            .flatten();
        self.load_task = Some(start_servo_load_task(
            metadata.name.clone(),
            path.clone(),
            self.controls.clone(),
            canvas_width,
            canvas_height,
            self.producer_role,
            self.host_driven_animation,
            display_descriptor,
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

    pub(super) fn poll_load_task(&mut self) {
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

#[allow(clippy::too_many_arguments)]
fn start_servo_load_task(
    effect_name: String,
    html_source: PathBuf,
    controls: HashMap<String, ControlValue>,
    canvas_width: u32,
    canvas_height: u32,
    producer_role: ServoProducerRole,
    host_driven_animation: bool,
    display_descriptor: Option<DisplayDescriptor>,
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
                display_descriptor.as_ref(),
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

pub(super) fn recycle_servo_session(session: ServoSessionHandle, reason: &'static str) {
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

#[allow(clippy::too_many_arguments)]
fn load_servo_session(
    effect_name: &str,
    html_source: PathBuf,
    controls: &HashMap<String, ControlValue>,
    canvas_width: u32,
    canvas_height: u32,
    producer_role: ServoProducerRole,
    host_driven_animation: bool,
    display_descriptor: Option<&DisplayDescriptor>,
) -> Result<LoadedServoSession> {
    let resolved = resolve_html_source_path(&html_source).with_context(|| {
        format!(
            "failed to resolve HTML source for effect '{effect_name}' from '{}'",
            html_source.display()
        )
    })?;

    let (runtime_source, runtime_html_path) = prepare_runtime_html_source(
        &resolved,
        controls,
        host_driven_animation,
        display_descriptor,
    )
    .with_context(|| {
        format!(
            "failed to prepare runtime HTML source for '{}'",
            resolved.display()
        )
    })?;

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
