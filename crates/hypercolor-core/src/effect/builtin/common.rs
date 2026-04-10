//! Shared helpers for built-in effect metadata tables.
//!
//! Each per-effect module calls into these constructors to build its
//! [`ControlDefinition`] list and [`PresetTemplate`] catalog, and the
//! module-level registrar uses [`builtin_effect_id`] to derive stable
//! UUIDs from the effect's stem name.

use hypercolor_types::effect::{
    ControlDefinition, ControlKind, ControlType, ControlValue, EffectId, PresetTemplate,
};
use uuid::Uuid;

pub(super) fn color_control(
    id: &str,
    name: &str,
    default_value: [f32; 4],
    group: &str,
    tooltip: &str,
) -> ControlDefinition {
    ControlDefinition {
        id: id.to_owned(),
        name: name.to_owned(),
        kind: ControlKind::Color,
        control_type: ControlType::ColorPicker,
        default_value: ControlValue::Color(default_value),
        min: None,
        max: None,
        step: None,
        labels: Vec::new(),
        group: Some(group.to_owned()),
        tooltip: Some(tooltip.to_owned()),
    }
}

#[allow(
    clippy::too_many_arguments,
    reason = "control definitions are constructed from explicit schema fields"
)]
pub(super) fn slider_control(
    id: &str,
    name: &str,
    default_value: f32,
    min: f32,
    max: f32,
    step: f32,
    group: &str,
    tooltip: &str,
) -> ControlDefinition {
    ControlDefinition {
        id: id.to_owned(),
        name: name.to_owned(),
        kind: ControlKind::Number,
        control_type: ControlType::Slider,
        default_value: ControlValue::Float(default_value),
        min: Some(min),
        max: Some(max),
        step: Some(step),
        labels: Vec::new(),
        group: Some(group.to_owned()),
        tooltip: Some(tooltip.to_owned()),
    }
}

pub(super) fn toggle_control(
    id: &str,
    name: &str,
    default_value: bool,
    group: &str,
    tooltip: &str,
) -> ControlDefinition {
    ControlDefinition {
        id: id.to_owned(),
        name: name.to_owned(),
        kind: ControlKind::Boolean,
        control_type: ControlType::Toggle,
        default_value: ControlValue::Boolean(default_value),
        min: None,
        max: None,
        step: None,
        labels: Vec::new(),
        group: Some(group.to_owned()),
        tooltip: Some(tooltip.to_owned()),
    }
}

pub(super) fn dropdown_control(
    id: &str,
    name: &str,
    default_value: &str,
    labels: &[&str],
    group: &str,
    tooltip: &str,
) -> ControlDefinition {
    ControlDefinition {
        id: id.to_owned(),
        name: name.to_owned(),
        kind: ControlKind::Combobox,
        control_type: ControlType::Dropdown,
        default_value: ControlValue::Enum(default_value.to_owned()),
        min: None,
        max: None,
        step: None,
        labels: labels.iter().map(|label| (*label).to_owned()).collect(),
        group: Some(group.to_owned()),
        tooltip: Some(tooltip.to_owned()),
    }
}

pub(super) fn preset(name: &str, controls: &[(&str, ControlValue)]) -> PresetTemplate {
    PresetTemplate {
        name: name.to_owned(),
        description: None,
        controls: controls
            .iter()
            .map(|(k, v)| ((*k).to_owned(), v.clone()))
            .collect(),
    }
}

pub(super) fn preset_with_desc(
    name: &str,
    description: &str,
    controls: &[(&str, ControlValue)],
) -> PresetTemplate {
    PresetTemplate {
        name: name.to_owned(),
        description: Some(description.to_owned()),
        controls: controls
            .iter()
            .map(|(k, v)| ((*k).to_owned(), v.clone()))
            .collect(),
    }
}

/// Generate a deterministic ID for a built-in effect.
///
/// IDs must remain stable across daemon restarts so saved references
/// (profiles/scenes/API clients) continue to resolve.
pub(super) fn builtin_effect_id(name: &str) -> EffectId {
    let key = format!("hypercolor:builtin:{name}");
    let mut hash: u128 = 0x6c62_69f0_7bb0_14d9_8d4f_1283_7ec6_3b8a;
    for byte in key.bytes() {
        hash ^= u128::from(byte);
        hash = hash.wrapping_mul(0x1000_0000_01b3);
    }

    let mut bytes = hash.to_be_bytes();
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;

    EffectId::new(Uuid::from_bytes(bytes))
}
