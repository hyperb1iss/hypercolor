use std::collections::HashMap;

use hypercolor_types::effect::{ControlValue, EffectCategory, EffectMetadata};
use hypercolor_types::sensor::SystemSnapshot;
use tracing::warn;

use super::super::note_servo_session_error;
use super::super::session::ServoRenderSubmission;
use super::super::worker_client::ServoFramePayload;
use super::{DEFAULT_DISPLAY_FPS_CAP, DEFAULT_EFFECT_FPS_CAP, MAX_EFFECT_FPS_CAP, ServoRenderer};
use crate::effect::lightscript::LightScriptFrameUpdateOptions;
use crate::effect::traits::FrameInput;
use crate::engine::FpsTier;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AnimationCadence {
    MatchRenderLoop,
    Fixed(u32),
}

impl AnimationCadence {
    pub(super) fn fps_cap(self, delta_secs: f32) -> u32 {
        match self {
            Self::MatchRenderLoop => animation_fps_cap(delta_secs),
            Self::Fixed(fps_cap) => fps_cap,
        }
    }

    pub(super) fn render_due(
        self,
        last_submit_time_secs: Option<f32>,
        next_time_secs: f32,
    ) -> bool {
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

impl ServoRenderer {
    pub(super) fn enqueue_bootstrap_scripts(&mut self) {
        self.pending_scripts.push(self.runtime.bootstrap_script());
        if self.host_driven_animation {
            self.pending_scripts
                .push(host_driven_animation_flag_script());
        }
        self.last_animation_fps_cap = Some(DEFAULT_EFFECT_FPS_CAP);
    }

    pub(super) fn enqueue_frame_payloads(&mut self, input: &FrameInput<'_>) {
        let fps_cap = self.animation_cadence.fps_cap(input.delta_secs);
        self.last_animation_fps_cap = Some(fps_cap);
        if let Some(payload) = self.runtime.frame_payload_json(
            input,
            &self.controls,
            LightScriptFrameUpdateOptions {
                include_audio: self.include_audio_updates,
                include_screen: self.include_screen_updates,
                include_sensors: self.include_sensor_updates,
                include_interaction: self.include_interaction_updates,
                render_host_frame: self.host_driven_animation,
                selected_sensor_labels: selected_sensor_labels(
                    &self.scoped_sensor_control_ids,
                    &self.controls,
                )
                .as_deref(),
            },
        ) {
            self.pending_frame_payloads.push(
                ServoFramePayload::from_json(payload)
                    .expect("LightScript frame payload should serialize as a JSON object"),
            );
        }
    }

    pub(super) fn take_pending_scripts(&mut self) -> Vec<String> {
        let capacity = self.pending_scripts.capacity();
        std::mem::replace(&mut self.pending_scripts, Vec::with_capacity(capacity))
    }

    pub(super) fn take_pending_frame_payloads(&mut self) -> Vec<ServoFramePayload> {
        let capacity = self.pending_frame_payloads.capacity();
        std::mem::replace(
            &mut self.pending_frame_payloads,
            Vec::with_capacity(capacity),
        )
    }

    pub(super) fn restore_pending_updates(
        &mut self,
        mut scripts: Vec<String>,
        mut frame_payloads: Vec<ServoFramePayload>,
    ) {
        scripts.append(&mut self.pending_scripts);
        frame_payloads.append(&mut self.pending_frame_payloads);
        self.pending_scripts = scripts;
        self.pending_frame_payloads = frame_payloads;
    }

    pub(super) fn queue_frame(&mut self, input: &FrameInput<'_>) {
        if let Some(frame) = self.queued_frame.as_mut() {
            frame.merge_from_input(input);
            return;
        }

        self.queued_frame = Some(QueuedFrameInput::from_input(input));
    }

    pub(super) fn try_submit_queued_frame(&mut self) {
        self.try_submit_queued_frame_with_gpu_preference(false);
    }

    pub(super) fn try_submit_queued_frame_with_gpu_preference(&mut self, prefer_gpu: bool) {
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
        self.enqueue_frame_payloads(&frame_input);
        if let Some(session) = self.session.as_mut() {
            session.resize(frame.canvas_width, frame.canvas_height);
        }
        let scripts = self.take_pending_scripts();
        let frame_payloads = self.take_pending_frame_payloads();
        let request_result = {
            #[cfg(feature = "servo-gpu-import")]
            let reuse_cached_on_no_ready = self.reuse_cached_gpu_frame_on_no_ready;
            let session = self
                .session
                .as_mut()
                .expect("session presence should be stable while queuing one render");
            if prefer_gpu {
                #[cfg(feature = "servo-gpu-import")]
                {
                    session.request_render_gpu(scripts, frame_payloads, reuse_cached_on_no_ready)
                }
                #[cfg(not(feature = "servo-gpu-import"))]
                {
                    session.request_render_cpu_with_frame_payloads(scripts, frame_payloads)
                }
            } else {
                session.request_render_cpu_with_frame_payloads(scripts, frame_payloads)
            }
        };

        match request_result {
            Ok(ServoRenderSubmission::Submitted) => {
                self.warned_stalled_frame = false;
                self.last_submit_time_secs = Some(frame.time_secs);
            }
            Ok(ServoRenderSubmission::Pending {
                scripts,
                frame_payloads,
            }) => {
                self.restore_pending_updates(scripts, frame_payloads);
                self.queued_frame = Some(frame);
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
}

#[derive(Debug, Clone)]
pub(super) struct QueuedFrameInput {
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
    pub(super) fn from_input(input: &FrameInput<'_>) -> Self {
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

pub(super) fn animation_cadence(metadata: &EffectMetadata) -> AnimationCadence {
    if metadata.category == EffectCategory::Display {
        return AnimationCadence::Fixed(DEFAULT_DISPLAY_FPS_CAP);
    }

    AnimationCadence::MatchRenderLoop
}

fn selected_sensor_labels(
    sensor_control_ids: &[String],
    controls: &HashMap<String, ControlValue>,
) -> Option<Vec<String>> {
    let labels = sensor_control_ids
        .iter()
        .filter_map(|control_id| match controls.get(control_id) {
            Some(ControlValue::Enum(label) | ControlValue::Text(label)) if !label.is_empty() => {
                Some(label.clone())
            }
            _ => None,
        })
        .collect::<Vec<_>>();

    (!labels.is_empty()).then_some(labels)
}

fn host_driven_animation_flag_script() -> String {
    concat!(
        "(function(){\n",
        "  window.__hypercolorHostDrivenAnimation = true;\n",
        "  if (typeof globalThis === 'object' && globalThis !== null) { globalThis.__hypercolorHostDrivenAnimation = true; }\n",
        "})();",
    )
    .to_owned()
}
