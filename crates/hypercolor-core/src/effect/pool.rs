use std::collections::{HashMap, HashSet};

use anyhow::{Result, anyhow};

use hypercolor_types::audio::AudioData;
use hypercolor_types::canvas::Canvas;
use hypercolor_types::effect::{
    ControlDefinition, ControlKind, ControlValue, EffectId, EffectMetadata,
};
use hypercolor_types::scene::{RenderGroup, RenderGroupId};
use hypercolor_types::sensor::SystemSnapshot;

use super::factory::create_renderer_for_metadata;
use super::registry::EffectRegistry;
use super::traits::{EffectRenderer, FrameInput, prepare_target_canvas};
use crate::input::{InteractionData, ScreenData};

pub struct EffectPool {
    slots: HashMap<RenderGroupId, EffectSlot>,
}

impl EffectPool {
    #[must_use]
    pub fn new() -> Self {
        Self {
            slots: HashMap::new(),
        }
    }

    #[must_use]
    pub fn slot_count(&self) -> usize {
        self.slots.len()
    }

    pub fn reconcile(&mut self, groups: &[RenderGroup], registry: &EffectRegistry) -> Result<()> {
        let desired_ids = groups.iter().map(|group| group.id).collect::<HashSet<_>>();
        self.slots
            .retain(|group_id, _| desired_ids.contains(group_id));

        for group in groups {
            let Some(effect_id) = group.effect_id else {
                self.slots.remove(&group.id);
                continue;
            };

            let needs_rebuild = self
                .slots
                .get(&group.id)
                .is_none_or(|slot| slot.effect_id != effect_id);
            if needs_rebuild {
                let metadata = lookup_effect_metadata(registry, effect_id)?;
                let slot = EffectSlot::build(metadata, group)?;
                self.slots.insert(group.id, slot);
                continue;
            }

            if let Some(slot) = self.slots.get_mut(&group.id) {
                slot.sync_group_state(group);
            }
        }

        Ok(())
    }

    pub fn render_group_into(
        &mut self,
        group: &RenderGroup,
        delta_secs: f32,
        audio: &AudioData,
        interaction: &InteractionData,
        screen: Option<&ScreenData>,
        sensors: &SystemSnapshot,
        target: &mut Canvas,
    ) -> Result<()> {
        prepare_target_canvas(
            target,
            group.layout.canvas_width,
            group.layout.canvas_height,
        );

        if !group.enabled || group.effect_id.is_none() {
            target.clear();
            return Ok(());
        }

        let slot = self.slots.get_mut(&group.id).ok_or_else(|| {
            anyhow!(
                "render group '{}' is not reconciled before rendering",
                group.name
            )
        })?;
        slot.render_into(
            delta_secs,
            audio,
            interaction,
            screen,
            sensors,
            group.layout.canvas_width,
            group.layout.canvas_height,
            target,
        )
    }
}

impl Default for EffectPool {
    fn default() -> Self {
        Self::new()
    }
}

struct EffectSlot {
    effect_id: EffectId,
    metadata: EffectMetadata,
    renderer: Box<dyn EffectRenderer>,
    controls: HashMap<String, ControlValue>,
    binding_state: HashMap<String, ActiveBindingState>,
    elapsed_secs: f32,
    frame_number: u64,
}

impl EffectSlot {
    fn build(metadata: EffectMetadata, group: &RenderGroup) -> Result<Self> {
        let mut renderer = create_renderer_for_metadata(&metadata)?;
        renderer.init_with_canvas_size(
            &metadata,
            group.layout.canvas_width,
            group.layout.canvas_height,
        )?;

        let mut slot = Self {
            effect_id: metadata.id,
            metadata,
            renderer,
            controls: HashMap::new(),
            binding_state: HashMap::new(),
            elapsed_secs: 0.0,
            frame_number: 0,
        };
        slot.sync_group_state(group);
        Ok(slot)
    }

    fn sync_group_state(&mut self, group: &RenderGroup) {
        let mut desired = HashMap::new();

        for definition in &mut self.metadata.controls {
            let next_binding = group.control_bindings.get(definition.control_id()).cloned();
            if definition.binding != next_binding {
                definition.binding = next_binding;
                self.binding_state.remove(definition.control_id());
            }
            let value = group
                .controls
                .get(definition.control_id())
                .cloned()
                .unwrap_or_else(|| definition.default_value.clone());
            desired.insert(definition.control_id().to_owned(), value);
        }

        for (name, value) in &group.controls {
            desired.entry(name.clone()).or_insert_with(|| value.clone());
        }

        for (name, value) in &desired {
            if self.controls.get(name) != Some(value) {
                self.renderer.set_control(name, value);
            }
        }

        self.controls = desired;
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "rendering needs the full frame input plus a mutable target canvas"
    )]
    fn render_into(
        &mut self,
        delta_secs: f32,
        audio: &AudioData,
        interaction: &InteractionData,
        screen: Option<&ScreenData>,
        sensors: &SystemSnapshot,
        canvas_width: u32,
        canvas_height: u32,
        target: &mut Canvas,
    ) -> Result<()> {
        self.elapsed_secs += delta_secs;
        apply_sensor_bindings(
            self.renderer.as_mut(),
            &self.metadata,
            &self.controls,
            &mut self.binding_state,
            sensors,
        );
        let input = FrameInput {
            time_secs: self.elapsed_secs,
            delta_secs,
            frame_number: self.frame_number,
            audio,
            interaction,
            screen,
            sensors,
            canvas_width,
            canvas_height,
        };
        self.renderer.render_into(&input, target)?;
        self.frame_number = self.frame_number.wrapping_add(1);
        Ok(())
    }
}

fn lookup_effect_metadata(
    registry: &EffectRegistry,
    effect_id: EffectId,
) -> Result<EffectMetadata> {
    registry
        .get(&effect_id)
        .map(|entry| entry.metadata.clone())
        .ok_or_else(|| anyhow!("effect '{effect_id}' is not registered"))
}

#[derive(Debug, Clone, PartialEq)]
struct ActiveBindingState {
    sensor_value: Option<f32>,
    control_value: ControlValue,
}

fn apply_sensor_bindings(
    renderer: &mut dyn EffectRenderer,
    metadata: &EffectMetadata,
    controls: &HashMap<String, ControlValue>,
    binding_state: &mut HashMap<String, ActiveBindingState>,
    sensors: &SystemSnapshot,
) {
    for control in &metadata.controls {
        let control_id = control.control_id();
        let Some(binding) = control.binding.as_ref() else {
            if binding_state.remove(control_id).is_some()
                && let Some(base_value) = controls.get(control_id)
            {
                renderer.set_control(control_id, base_value);
            }
            continue;
        };

        let Some(base_value) = controls.get(control_id) else {
            continue;
        };

        let next_state = sensors
            .reading(&binding.sensor)
            .and_then(|reading| {
                evaluate_sensor_binding(
                    control,
                    reading.value,
                    binding.target_min,
                    binding.target_max,
                    binding.sensor_min,
                    binding.sensor_max,
                    binding.deadband,
                    binding.smoothing,
                    binding_state.get(control_id),
                )
                .map(|value| ActiveBindingState {
                    sensor_value: Some(reading.value),
                    control_value: value,
                })
            })
            .unwrap_or_else(|| ActiveBindingState {
                sensor_value: None,
                control_value: base_value.clone(),
            });

        if binding_state.get(control_id) != Some(&next_state) {
            renderer.set_control(control_id, &next_state.control_value);
        }
        binding_state.insert(control_id.to_owned(), next_state);
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "binding evaluation works on normalized scalar ranges plus previous state"
)]
fn evaluate_sensor_binding(
    control: &ControlDefinition,
    sensor_value: f32,
    target_min: f32,
    target_max: f32,
    sensor_min: f32,
    sensor_max: f32,
    deadband: f32,
    smoothing: f32,
    previous: Option<&ActiveBindingState>,
) -> Option<ControlValue> {
    let source_span = sensor_max - sensor_min;
    if !source_span.is_finite()
        || source_span.abs() < f32::EPSILON
        || !target_min.is_finite()
        || !target_max.is_finite()
    {
        return None;
    }

    if let Some(previous) = previous
        && let Some(previous_sensor) = previous.sensor_value
        && (sensor_value - previous_sensor).abs() <= deadband
    {
        return Some(previous.control_value.clone());
    }

    let normalized = ((sensor_value - sensor_min) / source_span).clamp(0.0, 1.0);
    let mapped = target_min + normalized * (target_max - target_min);
    let smoothed = previous
        .and_then(|state| state.control_value.as_f32())
        .map_or(mapped, |previous_value| {
            let alpha = 1.0 - smoothing;
            previous_value + (mapped - previous_value) * alpha
        });

    match control.kind {
        ControlKind::Number | ControlKind::Hue | ControlKind::Area => {
            control.validate_value(&ControlValue::Float(smoothed)).ok()
        }
        ControlKind::Boolean => {
            let midpoint = target_min + (target_max - target_min) * 0.5;
            control
                .validate_value(&ControlValue::Boolean(smoothed >= midpoint))
                .ok()
        }
        _ => None,
    }
}
