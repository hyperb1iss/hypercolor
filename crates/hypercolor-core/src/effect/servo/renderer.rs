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
use hypercolor_types::effect::{ControlValue, EffectCategory, EffectMetadata, EffectSource};
use hypercolor_types::sensor::SystemSnapshot;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;
use tracing::{debug, info, warn};

use super::session::DrainPendingRenderError;
use super::telemetry::record_servo_soft_stall;
use super::worker::{
    RENDER_RESPONSE_TIMEOUT, effect_is_audio_reactive, poison_shared_servo_worker,
    prepare_runtime_html_source, servo_worker_is_fatal_error,
};
use super::{ServoSessionHandle, SessionConfig, note_servo_session_error};
use crate::effect::lightscript::LightscriptRuntime;
use crate::effect::paths::resolve_html_source_path;
use crate::effect::traits::{EffectRenderer, FrameInput};
use crate::engine::FpsTier;

const DEFAULT_EFFECT_FPS_CAP: u32 = 30;
const DEFAULT_DISPLAY_FPS_CAP: u32 = 30;
const MAX_EFFECT_FPS_CAP: u32 = 60;
const SOFT_STALL_FRAME_INTERVALS: u32 = 3;

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
    queued_frame: Option<QueuedFrameInput>,
    last_canvas: Option<Canvas>,
    warned_fallback_frame: bool,
    warned_stalled_frame: bool,
    include_audio_updates: bool,
    include_screen_updates: bool,
    last_animation_fps_cap: Option<u32>,
    animation_cadence: AnimationCadence,
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
            queued_frame: None,
            last_canvas: None,
            warned_fallback_frame: false,
            warned_stalled_frame: false,
            include_audio_updates: true,
            include_screen_updates: false,
            last_animation_fps_cap: None,
            animation_cadence: AnimationCadence::MatchRenderLoop,
            last_submit_time_secs: None,
        }
    }

    fn enqueue_bootstrap_scripts(&mut self) {
        self.pending_scripts.push(self.runtime.bootstrap_script());
        self.pending_scripts
            .push(animation_fps_cap_script(DEFAULT_EFFECT_FPS_CAP));
        self.last_animation_fps_cap = Some(DEFAULT_EFFECT_FPS_CAP);
    }

    fn enqueue_frame_scripts(&mut self, input: &FrameInput) {
        let fps_cap = self.animation_cadence.fps_cap(input.delta_secs);
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
        self.runtime.push_frame_scripts(
            &mut self.pending_scripts,
            &input.audio,
            input.screen,
            input.sensors,
            &self.controls,
            self.include_audio_updates,
            self.include_screen_updates,
        );
        if let Some(script) = self
            .runtime
            .input_update_script_if_changed(&input.interaction)
        {
            self.pending_scripts.push(script);
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
            if let Err(error) = std::fs::remove_file(&path) {
                debug!(
                    path = %path.display(),
                    %error,
                    "Failed to remove temporary runtime HTML source"
                );
            }
        }
    }

    fn initialize_with_canvas_size(
        &mut self,
        metadata: &EffectMetadata,
        canvas_width: u32,
        canvas_height: u32,
    ) -> Result<()> {
        use anyhow::Context;

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

        self.destroy();
        self.cleanup_runtime_html();
        self.session = None;
        self.controls.clear();
        self.runtime = LightscriptRuntime::new(canvas_width, canvas_height);
        self.pending_scripts.clear();
        self.warned_fallback_frame = false;
        self.warned_stalled_frame = false;
        self.include_audio_updates = effect_is_audio_reactive(metadata);
        self.include_screen_updates = metadata.screen_reactive;
        self.last_animation_fps_cap = None;
        self.animation_cadence = animation_cadence(metadata);
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

        let (runtime_source, runtime_html_path) =
            prepare_runtime_html_source(&resolved, &self.controls).with_context(|| {
                format!(
                    "failed to prepare runtime HTML source for '{}'",
                    resolved.display()
                )
            })?;

        let mut session = match ServoSessionHandle::new_shared(SessionConfig {
            render_width: canvas_width,
            render_height: canvas_height,
            inject_engine_globals: true,
        }) {
            Ok(session) => session,
            Err(error) => {
                note_servo_session_error("Servo effect session creation failed", &error);
                return Err(error);
            }
        };
        if let Err(error) = session.load_html_file(&runtime_source) {
            let _ = session.close();
            note_servo_session_error("Servo effect page load failed", &error);
            return Err(error);
        }
        self.session = Some(session);
        self.html_source = Some(path.clone());
        self.html_resolved_path = Some(runtime_source.clone());
        self.runtime_html_path = runtime_html_path;
        self.initialized = true;
        self.enqueue_bootstrap_scripts();

        info!(
            effect = %metadata.name,
            source = %path.display(),
            resolved = %runtime_source.display(),
            canvas_width,
            canvas_height,
            "Initialized ServoRenderer worker"
        );

        Ok(())
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

    fn try_submit_queued_frame(&mut self) {
        let Some(session) = self.session.as_ref() else {
            return;
        };
        if session.has_pending_render() {
            return;
        };
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
        match self
            .session
            .as_mut()
            .expect("session presence should be stable while queuing one render")
            .request_render(scripts)
        {
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

    fn drain_in_flight_render(&mut self) {
        let Some(session) = self.session.as_mut() else {
            return;
        };

        match session.drain_pending_render(RENDER_RESPONSE_TIMEOUT) {
            Ok(Some(canvas)) => {
                self.last_canvas = Some(canvas);
                self.warned_fallback_frame = false;
                self.warned_stalled_frame = false;
            }
            Ok(None) => {}
            Err(DrainPendingRenderError::Worker(error)) => {
                note_servo_session_error(
                    "Servo frame render failed while draining effect teardown",
                    &error,
                );
                if servo_worker_is_fatal_error(&error) {
                    self.session = None;
                }
                warn!(%error, "Servo frame render failed while draining effect teardown");
            }
            Err(DrainPendingRenderError::TimedOut) => {
                poison_shared_servo_worker(
                    "Timed out waiting for in-flight Servo frame during effect teardown",
                );
                warn!(
                    timeout_ms = RENDER_RESPONSE_TIMEOUT.as_millis(),
                    "Timed out waiting for in-flight Servo frame during effect teardown"
                );
            }
            Err(DrainPendingRenderError::Disconnected) => {
                poison_shared_servo_worker(
                    "Servo worker disconnected while draining effect teardown",
                );
                warn!("Servo worker disconnected while draining effect teardown");
                self.session = None;
            }
        }
    }

    fn active_fps_cap(&self) -> u32 {
        self.last_animation_fps_cap.unwrap_or(DEFAULT_EFFECT_FPS_CAP)
    }

    fn soft_stall_timeout(&self) -> Duration {
        let tier = FpsTier::from_fps(self.active_fps_cap());
        let soft_timeout = tier.frame_interval().mul_f32(SOFT_STALL_FRAME_INTERVALS as f32);
        soft_timeout.min(RENDER_RESPONSE_TIMEOUT)
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

        self.queue_frame(input);
        self.poll_in_flight_render();
        self.try_submit_queued_frame();

        if let Some(canvas) = self.last_canvas.as_ref()
            && canvas.width() == input.canvas_width
            && canvas.height() == input.canvas_height
        {
            target.clone_from(canvas);
        } else {
            Self::render_placeholder_into(target, input);
        }
        Ok(())
    }

    fn set_control(&mut self, name: &str, value: &ControlValue) {
        self.controls.insert(name.to_owned(), value.clone());
    }

    fn destroy(&mut self) {
        self.drain_in_flight_render();
        if let Some(session) = self.session.take() {
            if let Err(error) = session.close() {
                note_servo_session_error(
                    "Failed to destroy Servo effect session during destroy",
                    &error,
                );
                warn!(%error, "Failed to destroy Servo effect session during destroy");
            }
        }
        self.pending_scripts.clear();
        self.queued_frame = None;
        self.last_canvas = None;
        self.controls.clear();
        self.html_source = None;
        self.html_resolved_path = None;
        self.cleanup_runtime_html();
        self.initialized = false;
        self.warned_fallback_frame = false;
        self.warned_stalled_frame = false;
        self.include_audio_updates = true;
        self.include_screen_updates = false;
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

fn animation_fps_cap_script(fps_cap: u32) -> String {
    format!("window.__hypercolorFpsCap = {fps_cap};")
}

fn animation_cadence(metadata: &EffectMetadata) -> AnimationCadence {
    if metadata.category == EffectCategory::Display {
        return AnimationCadence::Fixed(DEFAULT_DISPLAY_FPS_CAP);
    }

    AnimationCadence::MatchRenderLoop
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effect::servo::worker::{
        install_running_shared_worker, reset_shared_servo_worker_state,
        shutdown_shared_servo_worker,
        test_support::{
            SHARED_WORKER_STATE_TEST_LOCK, spawn_load_test_worker, spawn_render_test_worker,
            spawn_test_worker, worker_client_from,
        },
    };
    use hypercolor_types::audio::AudioData;
    use hypercolor_types::effect::{EffectCategory, EffectId, EffectSource};
    use hypercolor_types::sensor::SystemSnapshot;
    use std::sync::LazyLock;
    use std::sync::atomic::Ordering;
    use std::thread;
    use std::time::Duration;
    use uuid::Uuid;

    static SILENCE: LazyLock<AudioData> = LazyLock::new(AudioData::silence);
    static DEFAULT_INTERACTION: LazyLock<crate::input::InteractionData> =
        LazyLock::new(crate::input::InteractionData::default);
    static EMPTY_SENSORS: LazyLock<SystemSnapshot> = LazyLock::new(SystemSnapshot::empty);

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

    fn solid_canvas(r: u8, g: u8, b: u8) -> Canvas {
        let mut canvas = Canvas::new(DEFAULT_CANVAS_WIDTH, DEFAULT_CANVAS_HEIGHT);
        canvas.fill(Rgba::new(r, g, b, 255));
        canvas
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
        renderer.last_canvas = Some(solid_canvas(1, 2, 3));

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
    fn display_animation_cadence_stays_fixed_at_30_fps() {
        let metadata = display_html_metadata(PathBuf::from("display.html"));

        assert_eq!(animation_cadence(&metadata), AnimationCadence::Fixed(30));
        assert_eq!(animation_cadence(&metadata).fps_cap(1.0 / 60.0), 30);
        assert_eq!(animation_cadence(&metadata).fps_cap(1.0 / 20.0), 30);
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
        assert_eq!(renderer.last_animation_fps_cap, Some(20));
        assert!(
            renderer
                .pending_scripts
                .iter()
                .any(|script| script == "window.__hypercolorFpsCap = 20;")
        );
    }

    #[test]
    fn display_frame_scripts_keep_fixed_animation_cap() {
        let mut renderer = ServoRenderer::new();
        renderer.animation_cadence = AnimationCadence::Fixed(30);
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
    }

    #[test]
    fn soft_stall_timeout_tracks_active_animation_cap() {
        let mut renderer = ServoRenderer::new();

        assert_eq!(
            renderer.soft_stall_timeout(),
            FpsTier::Medium.frame_interval().mul_f32(SOFT_STALL_FRAME_INTERVALS as f32)
        );

        renderer.last_animation_fps_cap = Some(60);
        assert_eq!(
            renderer.soft_stall_timeout(),
            FpsTier::Full.frame_interval().mul_f32(SOFT_STALL_FRAME_INTERVALS as f32)
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
        let (worker, render_rx, result_tx, delivered_rx, _unload_rx, stopped) =
            spawn_render_test_worker();
        let baseline_stalls = crate::effect::servo::servo_telemetry_snapshot().soft_stalls_total;

        let mut renderer = ServoRenderer::new();
        attach_renderer_session(&mut renderer, &worker);
        renderer.initialized = true;
        renderer.last_animation_fps_cap = Some(60);
        renderer.last_canvas = Some(solid_canvas(20, 40, 60));
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
            .send(Ok(solid_canvas(1, 1, 1)))
            .expect("cleanup render result");
        delivered_rx
            .recv_timeout(Duration::from_millis(100))
            .expect("cleanup result delivery ack");

        drop(worker);
        assert!(stopped.load(Ordering::SeqCst));
    }

    #[test]
    fn poll_in_flight_render_clears_stall_warning_after_completed_frame() {
        let (worker, render_rx, result_tx, delivered_rx, _unload_rx, stopped) =
            spawn_render_test_worker();

        let mut renderer = ServoRenderer::new();
        attach_renderer_session(&mut renderer, &worker);
        renderer.initialized = true;
        renderer.last_animation_fps_cap = Some(60);
        renderer.last_canvas = Some(solid_canvas(20, 40, 60));
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
            .send(Ok(solid_canvas(9, 8, 7)))
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
        assert!(
            renderer
                .pending_scripts
                .iter()
                .any(|script| script == "window.__hypercolorFpsCap = 60;")
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
                .any(|script| script == "window.__hypercolorFpsCap = 20;")
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
