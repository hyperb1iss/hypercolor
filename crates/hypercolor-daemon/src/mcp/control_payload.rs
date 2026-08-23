//! Raw JSON control admission for MCP tool arguments.
//!
//! MCP callers send control values as loose JSON, so each value is
//! decoded through the addressed control's schema before it reaches
//! the typed domain path REST already uses.

use std::collections::HashMap;

use hypercolor_types::control::ControlValue;
use hypercolor_types::controls::{ControlApplyError, RejectedControlChange};
use hypercolor_types::effect::{EffectControlAdmissionError, EffectMetadata};

use crate::domain::effect::effect_json_rejection;

pub(crate) fn admit_raw_controls(
    metadata: &EffectMetadata,
    raw_controls: &serde_json::Map<String, serde_json::Value>,
) -> (HashMap<String, ControlValue>, Vec<RejectedControlChange>) {
    let mut normalized = HashMap::new();
    let mut rejected = Vec::new();

    for (name, value) in raw_controls {
        let result = metadata.control_by_id(name).map_or_else(
            || {
                let parsed =
                    ControlValue::try_from_effect_json(value).map_err(effect_json_rejection)?;
                parsed.try_to_effect_json().map_err(effect_json_rejection)?;
                Ok(parsed)
            },
            |control| {
                control
                    .admit_effect_json(value)
                    .map_err(|error| match error {
                        EffectControlAdmissionError::Json(error) => effect_json_rejection(error),
                        EffectControlAdmissionError::Validation(error) => {
                            ControlApplyError::InvalidValue {
                                message: error.to_string(),
                            }
                        }
                    })
            },
        );
        match result {
            Ok(control_value) => {
                normalized.insert(name.clone(), control_value);
            }
            Err(error) => rejected.push(RejectedControlChange {
                field_id: name.clone(),
                attempted_value: ControlValue::try_from_effect_json(value)
                    .unwrap_or(ControlValue::Unknown),
                error,
            }),
        }
    }

    (normalized, rejected)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use hypercolor_types::control::ControlValue;
    use hypercolor_types::effect::{
        ControlDefinition, ControlKind, ControlType, EffectCategory, EffectId, EffectMetadata,
        EffectSource,
    };

    fn metadata() -> EffectMetadata {
        EffectMetadata {
            id: EffectId::new(uuid::Uuid::now_v7()),
            name: "admission fixture".to_owned(),
            author: "test".to_owned(),
            version: "1".to_owned(),
            description: String::new(),
            category: EffectCategory::Ambient,
            tags: Vec::new(),
            controls: vec![ControlDefinition {
                id: "accent".to_owned(),
                name: "Accent".to_owned(),
                kind: ControlKind::Color,
                control_type: ControlType::ColorPicker,
                default_value: ControlValue::linear_color([1.0, 1.0, 1.0, 1.0]),
                min: None,
                max: None,
                step: None,
                labels: Vec::new(),
                group: None,
                tooltip: None,
                aspect_lock: None,
                preview_source: None,
                binding: None,
            }],
            presets: Vec::new(),
            audio_reactive: false,
            screen_reactive: false,
            input_reactive: false,
            source: EffectSource::Native {
                path: "fixture".into(),
            },
            license: None,
        }
    }

    #[test]
    fn raw_admission_uses_color_schema_but_keeps_unknown_arrays_strict() {
        let raw = serde_json::json!({
            "accent": [0.125, 0.25, 0.5, 1.0],
            "unknown": [0.125, 0.25, 0.5, 1.0]
        });
        let (admitted, rejected) = super::admit_raw_controls(
            &metadata(),
            raw.as_object().expect("fixture should be an object"),
        );

        assert_eq!(
            admitted.get("accent"),
            Some(&ControlValue::linear_color([0.125, 0.25, 0.5, 1.0]))
        );
        assert_eq!(rejected.len(), 1);
        assert_eq!(rejected[0].field_id, "unknown");
        assert!(matches!(
            rejected[0].error,
            hypercolor_types::controls::ControlApplyError::InvalidValue { .. }
        ));
    }

    #[test]
    fn typed_admission_rejects_every_non_effect_variant_on_unknown_keys() {
        let rejected_values = [
            serde_json::json!({"kind": "null"}),
            serde_json::json!({"kind": "secret_ref", "value": "token"}),
            serde_json::json!({"kind": "ip", "value": "127.0.0.1"}),
            serde_json::json!({"kind": "mac", "value": "01:23:45:67:89:ab"}),
            serde_json::json!({"kind": "duration", "value": 250}),
            serde_json::json!({"kind": "color_rgb", "value": {"r": 1, "g": 2, "b": 3}}),
            serde_json::json!({"kind": "color_rgba", "value": {"r": 1, "g": 2, "b": 3, "a": 4}}),
            serde_json::json!({"kind": "flags", "value": ["one"]}),
            serde_json::json!({"kind": "list", "value": [{"kind": "bool", "value": true}]}),
            serde_json::json!({"kind": "map", "value": {"one": {"kind": "bool", "value": true}}}),
            serde_json::json!({"kind": "unknown"}),
        ]
        .into_iter()
        .enumerate()
        .map(|(index, value)| {
            (
                format!("unknown_{index}"),
                serde_json::from_value::<ControlValue>(value)
                    .expect("fixture should decode canonically"),
            )
        })
        .collect::<HashMap<_, _>>();

        let (admitted, rejected) =
            crate::domain::effect::normalize_control_values(&metadata(), &rejected_values);

        assert!(admitted.is_empty());
        assert_eq!(rejected.len(), rejected_values.len());
    }
}
