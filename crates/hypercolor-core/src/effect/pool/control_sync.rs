use std::collections::HashMap;

use anyhow::{Context, Result, anyhow};
use hypercolor_types::control::{
    ControlDeltaBatch, ControlId, ControlSet, ControlValue, SetRevision,
};
use hypercolor_types::effect::{ControlBinding, ControlDefinition, ControlKind, EffectMetadata};
use hypercolor_types::sensor::SystemSnapshot;

use super::LayerEffectSource;
use crate::effect::traits::EffectRenderer;

pub(super) fn canonical_control_set(
    revision: SetRevision,
    metadata: &EffectMetadata,
    source: &LayerEffectSource,
) -> Result<ControlSet> {
    let mut controls = ControlSet::new(revision);
    for definition in &metadata.controls {
        let control_id = definition.control_id();
        let authored_value = source
            .controls
            .get(control_id)
            .unwrap_or(&definition.default_value);
        let value = definition.validate_value(authored_value).map_err(|error| {
            anyhow!(
                "effect '{}' control '{control_id}' is invalid: {error}",
                metadata.id
            )
        })?;
        value.try_to_effect_json().map_err(|error| {
            anyhow!(
                "effect '{}' control '{control_id}' cannot enter the runtime: {error}",
                metadata.id
            )
        })?;
        controls.insert(ControlId::from(control_id), value)?;
    }
    for (control_id, value) in &source.controls {
        if controls.get(control_id).is_none() {
            value.try_to_effect_json().map_err(|error| {
                anyhow!(
                    "effect '{}' control '{control_id}' cannot enter the runtime: {error}",
                    metadata.id
                )
            })?;
            controls.insert(ControlId::from(control_id.as_str()), value.clone())?;
        }
    }
    Ok(controls)
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct ActiveBindingState {
    pub(super) sensor_value: Option<f32>,
    pub(super) control_value: ControlValue,
}

#[expect(
    clippy::too_many_arguments,
    reason = "binding application updates renderer and authoritative resolution state"
)]
pub(super) fn apply_sensor_bindings(
    renderer: &mut dyn EffectRenderer,
    metadata: &EffectMetadata,
    bindings: &HashMap<String, ControlBinding>,
    controls: &ControlSet,
    binding_state: &mut HashMap<String, ActiveBindingState>,
    resolution_seq: &mut u64,
    controls_initialized: &mut bool,
    sensors: &SystemSnapshot,
) -> Result<()> {
    let mut next_binding_state = binding_state.clone();
    let mut changes = Vec::new();

    for control in &metadata.controls {
        let control_id = control.control_id();
        let Some(binding) = bindings.get(control_id) else {
            if next_binding_state.remove(control_id).is_some()
                && let Some(base_value) = controls.get(control_id)
            {
                changes.push((ControlId::from(control_id), base_value.clone()));
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
            changes.push((
                ControlId::from(control_id),
                next_state.control_value.clone(),
            ));
        }
        next_binding_state.insert(control_id.to_owned(), next_state);
    }

    if !*controls_initialized {
        let snapshot = resolved_control_snapshot(controls, &next_binding_state)?;
        renderer
            .initialize_controls(&snapshot)
            .context("renderer rejected authoritative control snapshot")?;
        *controls_initialized = true;
        *binding_state = next_binding_state;
        return Ok(());
    }

    if changes.is_empty() {
        *binding_state = next_binding_state;
        return Ok(());
    }

    let next_sequence = resolution_seq
        .checked_add(1)
        .ok_or_else(|| anyhow!("control resolution sequence overflowed"))?;
    let batch = ControlDeltaBatch::new(controls.set_revision(), next_sequence, &changes);
    if let Err(delta_error) = renderer.apply_controls(&batch) {
        *controls_initialized = false;
        let snapshot = resolved_control_snapshot(controls, &next_binding_state)?;
        renderer.initialize_controls(&snapshot).with_context(|| {
            format!("renderer rejected sensor delta ({delta_error}) and snapshot replay")
        })?;
        *controls_initialized = true;
    }

    *binding_state = next_binding_state;
    *resolution_seq = next_sequence;
    Ok(())
}

fn resolved_control_snapshot(
    controls: &ControlSet,
    binding_state: &HashMap<String, ActiveBindingState>,
) -> Result<ControlSet> {
    let mut snapshot = controls.clone();
    for (control_id, state) in binding_state {
        snapshot.insert(
            ControlId::from(control_id.as_str()),
            state.control_value.clone(),
        )?;
    }
    Ok(snapshot)
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
        .and_then(|state| state.control_value.as_effect_f32())
        .map_or(mapped, |previous_value| {
            let alpha = 1.0 - smoothing;
            previous_value + (mapped - previous_value) * alpha
        });

    match control.kind {
        ControlKind::Number | ControlKind::Hue | ControlKind::Area => control
            .validate_value(&ControlValue::Float(f64::from(smoothed)))
            .ok(),
        ControlKind::Boolean => {
            let midpoint = target_min + (target_max - target_min) * 0.5;
            control
                .validate_value(&ControlValue::Bool(smoothed >= midpoint))
                .ok()
        }
        _ => None,
    }
}
