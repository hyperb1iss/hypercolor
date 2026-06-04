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
use hypercolor_types::effect::{ControlKind, ControlValue, EffectCategory, EffectMetadata};
use std::collections::HashMap;
use std::path::PathBuf;

use super::ServoSessionHandle;
#[cfg(test)]
use super::SessionConfig;
#[cfg(test)]
use super::session::ServoRenderSubmission;
use super::worker_client::{ServoFramePayload, ServoProducerRole};
use crate::effect::lightscript::LightscriptRuntime;
#[cfg(feature = "servo-gpu-import")]
use crate::effect::traits::{EffectRenderOutput, ImportedEffectFrame};
use crate::effect::traits::{EffectRenderer, FrameInput, prepare_target_canvas};
#[cfg(test)]
use crate::engine::FpsTier;
use frame_queue::{AnimationCadence, QueuedFrameInput, animation_cadence};
#[cfg(test)]
use load::{LoadedServoSession, ServoLoadTaskState};
use load::{ServoLoadTask, recycle_servo_session};

const DEFAULT_EFFECT_FPS_CAP: u32 = 30;
const DEFAULT_DISPLAY_FPS_CAP: u32 = 30;
const MAX_EFFECT_FPS_CAP: u32 = 60;
const SOFT_STALL_FRAME_INTERVALS: u32 = 5;

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
}

fn copy_completed_canvas_into_target(source: &Canvas, target: &mut Canvas) {
    prepare_target_canvas(target, source.width(), source.height());
    target
        .as_rgba_bytes_mut()
        .copy_from_slice(source.as_rgba_bytes());
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
mod load;
#[cfg(test)]
mod tests;
