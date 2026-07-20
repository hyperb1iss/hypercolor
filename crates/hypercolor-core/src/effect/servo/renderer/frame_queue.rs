use std::collections::HashMap;
use std::sync::{Arc, LazyLock};

use hypercolor_types::effect::{ControlValue, EffectCategory, EffectMetadata};
use hypercolor_types::sensor::SystemSnapshot;
use tracing::warn;

use super::super::note_servo_session_error;
use super::super::session::ServoRenderAdmission;
use super::super::worker_client::ServoFramePayload;
use super::{DEFAULT_DISPLAY_FPS_CAP, DEFAULT_EFFECT_FPS_CAP, MAX_EFFECT_FPS_CAP, ServoRenderer};
use crate::effect::lightscript::{LightScriptFrameUpdate, LightScriptFrameUpdateOptions};
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
        last_submit_time_secs: Option<f64>,
        next_time_secs: f64,
    ) -> bool {
        match self {
            Self::MatchRenderLoop => true,
            Self::Fixed(fps_cap) => {
                let min_frame_interval_secs = 1.0 / f64::from(fps_cap.max(1));
                last_submit_time_secs.is_none_or(|last_submit_time_secs| {
                    next_time_secs + f64::EPSILON >= last_submit_time_secs + min_frame_interval_secs
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
        if let Some(update) = self.runtime.frame_update(
            input,
            &self.controls,
            LightScriptFrameUpdateOptions {
                include_audio: self.include_audio_updates,
                include_screen: self.include_screen_updates,
                include_sensors: self.include_sensor_updates,
                include_interaction: self.include_interaction_updates,
                include_media: self.include_media_updates,
                include_net: self.include_net_updates,
                include_lighting: self.include_lighting_updates,
                render_host_frame: self.host_driven_animation,
                selected_sensor_labels: selected_sensor_labels(
                    &self.scoped_sensor_control_ids,
                    &self.controls,
                )
                .as_deref(),
            },
        ) {
            match update {
                LightScriptFrameUpdate::PayloadJson(payload) => {
                    self.pending_frame_payloads.push(
                        ServoFramePayload::from_json(payload)
                            .expect("LightScript frame payload should serialize as a JSON object"),
                    );
                }
                LightScriptFrameUpdate::HostFrameScript(script) => {
                    self.pending_scripts.push(script);
                }
            }
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

    pub(super) fn queue_frame(&mut self, input: &FrameInput<'_>) {
        let demand = QueuedFrameDemand {
            audio: self.include_audio_updates,
            interaction: self.include_interaction_updates,
            screen: self.include_screen_updates,
            sensors: self.include_sensor_updates,
            media: self.include_media_updates,
            net: self.include_net_updates,
            lighting: self.include_lighting_updates,
        };
        if let Some(frame) = self.queued_frame.as_mut() {
            frame.merge_from_input(input, demand);
            return;
        }

        self.queued_frame = Some(QueuedFrameInput::from_input(input, demand));
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

        let reservation = match self
            .session
            .as_ref()
            .expect("session presence should be stable while admitting one render")
            .reserve_render()
        {
            Ok(ServoRenderAdmission::Reserved(reservation)) => reservation,
            Ok(ServoRenderAdmission::Pending) => {
                self.queued_frame = Some(frame);
                return;
            }
            Ok(ServoRenderAdmission::Saturated) => {
                self.retain_saturated_frame(frame);
                return;
            }
            Err(error) => {
                self.command_queue_saturated = false;
                note_servo_session_error("Failed to admit Servo frame render", &error);
                self.session = None;
                warn!(%error, "Failed to admit Servo frame render");
                if !self.warned_fallback_frame {
                    warn!("Falling back to the previous completed frame for this effect");
                    self.warned_fallback_frame = true;
                }
                return;
            }
        };

        let frame_input = frame.as_frame_input();
        let mut scripts = self.take_pending_scripts();
        self.enqueue_frame_payloads(&frame_input);
        scripts.append(&mut self.take_pending_scripts());
        if let Some(session) = self.session.as_mut() {
            session.resize(frame.canvas_width, frame.canvas_height);
        }
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
                    session.request_reserved_render_gpu(
                        reservation,
                        scripts,
                        frame_payloads,
                        reuse_cached_on_no_ready,
                    )
                }
                #[cfg(not(feature = "servo-gpu-import"))]
                {
                    session.request_reserved_render_cpu_with_frame_payloads(
                        reservation,
                        scripts,
                        frame_payloads,
                    )
                }
            } else {
                session.request_reserved_render_cpu_with_frame_payloads(
                    reservation,
                    scripts,
                    frame_payloads,
                )
            }
        };

        match request_result {
            Ok(()) => {
                self.warned_stalled_frame = false;
                self.command_queue_saturated = false;
                self.last_submit_time_secs = Some(frame.time_secs);
            }
            Err(error) => {
                self.command_queue_saturated = false;
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

    pub(super) fn retain_saturated_frame(&mut self, frame: QueuedFrameInput) {
        self.queued_frame = Some(frame);
        if !self.command_queue_saturated {
            warn!("Servo worker render queue is saturated; retaining latest frame");
        }
        self.command_queue_saturated = true;
    }
}

static SILENT_AUDIO: LazyLock<hypercolor_types::audio::AudioData> =
    LazyLock::new(hypercolor_types::audio::AudioData::silence);
static EMPTY_INTERACTION: LazyLock<crate::input::InteractionData> =
    LazyLock::new(crate::input::InteractionData::default);
static EMPTY_SENSORS: LazyLock<SystemSnapshot> = LazyLock::new(SystemSnapshot::empty);

#[derive(Debug, Clone, Copy)]
pub(super) struct QueuedFrameDemand {
    audio: bool,
    interaction: bool,
    screen: bool,
    sensors: bool,
    media: bool,
    net: bool,
    lighting: bool,
}

#[derive(Debug, Clone)]
pub(super) struct QueuedFrameInput {
    time_secs: f64,
    delta_secs: f32,
    frame_number: u64,
    audio: Option<Arc<hypercolor_types::audio::AudioData>>,
    interaction: Option<Arc<crate::input::InteractionData>>,
    screen: Option<Arc<crate::input::ScreenData>>,
    sensors: Option<Arc<SystemSnapshot>>,
    media: Option<Arc<hypercolor_types::media::MediaState>>,
    net: Option<Arc<hypercolor_types::net::NetStats>>,
    lighting: Option<Arc<hypercolor_types::lighting::LightingState>>,
    canvas_width: u32,
    canvas_height: u32,
}

impl QueuedFrameInput {
    pub(super) fn from_input(input: &FrameInput<'_>, demand: QueuedFrameDemand) -> Self {
        Self {
            time_secs: input.time_secs,
            delta_secs: input.delta_secs,
            frame_number: input.frame_number,
            audio: demand.audio.then(|| Arc::new(input.audio.clone())),
            interaction: demand
                .interaction
                .then(|| Arc::new(input.interaction.clone())),
            screen: if demand.screen {
                input.screen.map(|screen| Arc::new(screen.clone()))
            } else {
                None
            },
            sensors: demand.sensors.then(|| Arc::new(input.sensors.clone())),
            media: if demand.media {
                input.sources.media.map(|media| Arc::new(media.clone()))
            } else {
                None
            },
            net: if demand.net {
                input.sources.net.map(|net| Arc::new(net.clone()))
            } else {
                None
            },
            lighting: if demand.lighting {
                input
                    .sources
                    .lighting
                    .map(|lighting| Arc::new(lighting.clone()))
            } else {
                None
            },
            canvas_width: input.canvas_width,
            canvas_height: input.canvas_height,
        }
    }

    fn merge_from_input(&mut self, input: &FrameInput<'_>, demand: QueuedFrameDemand) {
        let (prior_recent_keys, prior_batch) = self
            .interaction
            .as_mut()
            .map(|interaction| {
                let interaction = Arc::make_mut(interaction);
                (
                    std::mem::take(&mut interaction.keyboard.recent_keys),
                    std::mem::take(&mut interaction.batch),
                )
            })
            .unwrap_or_default();
        self.time_secs = input.time_secs;
        self.delta_secs = input.delta_secs;
        self.frame_number = input.frame_number;
        clone_demanded_from(&mut self.audio, input.audio, demand.audio);
        clone_demanded_from(&mut self.interaction, input.interaction, demand.interaction);
        clone_optional_demanded_from(&mut self.screen, input.screen, demand.screen);
        clone_demanded_from(&mut self.sensors, input.sensors, demand.sensors);
        clone_optional_demanded_from(&mut self.media, input.sources.media, demand.media);
        clone_optional_demanded_from(&mut self.net, input.sources.net, demand.net);
        clone_optional_demanded_from(&mut self.lighting, input.sources.lighting, demand.lighting);
        if let Some(interaction) = self.interaction.as_mut() {
            let interaction = Arc::make_mut(interaction);
            merge_unique_strings(&mut interaction.keyboard.recent_keys, prior_recent_keys);
            // Superseded frames must not lose their input edges: fold the
            // replaced frame's batch in ahead of the new one.
            interaction.batch.absorb_prior(prior_batch);
        }
        self.canvas_width = input.canvas_width;
        self.canvas_height = input.canvas_height;
    }

    fn as_frame_input(&self) -> FrameInput<'_> {
        FrameInput {
            time_secs: self.time_secs,
            delta_secs: self.delta_secs,
            frame_number: self.frame_number,
            audio: self.audio.as_deref().unwrap_or(&SILENT_AUDIO),
            interaction: self.interaction.as_deref().unwrap_or(&EMPTY_INTERACTION),
            screen: self.screen.as_deref(),
            sensors: self.sensors.as_deref().unwrap_or(&EMPTY_SENSORS),
            sources: crate::effect::traits::FrameDataSources {
                media: self.media.as_deref(),
                net: self.net.as_deref(),
                lighting: self.lighting.as_deref(),
            },
            canvas_width: self.canvas_width,
            canvas_height: self.canvas_height,
        }
    }

    #[cfg(test)]
    pub(super) fn retained_input_domains(&self) -> [bool; 7] {
        [
            self.audio.is_some(),
            self.interaction.is_some(),
            self.screen.is_some(),
            self.sensors.is_some(),
            self.media.is_some(),
            self.net.is_some(),
            self.lighting.is_some(),
        ]
    }

    #[cfg(test)]
    pub(super) fn queued_interaction(&self) -> Option<&crate::input::InteractionData> {
        self.interaction.as_deref()
    }

    #[cfg(test)]
    pub(super) const fn queued_frame_number(&self) -> u64 {
        self.frame_number
    }
}

fn clone_demanded_from<T: Clone>(slot: &mut Option<Arc<T>>, next: &T, demanded: bool) {
    clone_optional_demanded_from(slot, Some(next), demanded);
}

fn clone_optional_demanded_from<T: Clone>(
    slot: &mut Option<Arc<T>>,
    next: Option<&T>,
    demanded: bool,
) {
    if !demanded {
        *slot = None;
        return;
    }
    match (slot.as_mut(), next) {
        (Some(current), Some(next)) => Arc::make_mut(current).clone_from(next),
        (None, Some(next)) => *slot = Some(Arc::new(next.clone())),
        (_, None) => *slot = None,
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
