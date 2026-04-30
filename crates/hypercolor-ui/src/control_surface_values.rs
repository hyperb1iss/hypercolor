use std::collections::BTreeMap;

use hypercolor_types::controls::{
    ControlObjectField, ControlValue as DynamicControlValue, ControlValueType,
};
use serde_json::Value as JsonValue;

pub fn json_text(value: Option<&DynamicControlValue>) -> String {
    value
        .map(control_value_to_json)
        .and_then(|value| serde_json::to_string_pretty(&value).ok())
        .unwrap_or_default()
}

pub fn control_value_to_json(value: &DynamicControlValue) -> JsonValue {
    match value {
        DynamicControlValue::Null => JsonValue::Null,
        DynamicControlValue::Bool(value) => JsonValue::Bool(*value),
        DynamicControlValue::Integer(value) => JsonValue::from(*value),
        DynamicControlValue::Float(value) => JsonValue::from(*value),
        DynamicControlValue::String(value)
        | DynamicControlValue::SecretRef(value)
        | DynamicControlValue::IpAddress(value)
        | DynamicControlValue::MacAddress(value)
        | DynamicControlValue::Enum(value) => JsonValue::String(value.clone()),
        DynamicControlValue::ColorRgb(value) => JsonValue::Array(
            value
                .iter()
                .map(|channel| JsonValue::from(*channel))
                .collect(),
        ),
        DynamicControlValue::ColorRgba(value) => JsonValue::Array(
            value
                .iter()
                .map(|channel| JsonValue::from(*channel))
                .collect(),
        ),
        DynamicControlValue::DurationMs(value) => JsonValue::from(*value),
        DynamicControlValue::Flags(values) => JsonValue::Array(
            values
                .iter()
                .map(|value| JsonValue::String(value.clone()))
                .collect(),
        ),
        DynamicControlValue::List(values) => {
            JsonValue::Array(values.iter().map(control_value_to_json).collect())
        }
        DynamicControlValue::Object(values) => JsonValue::Object(
            values
                .iter()
                .map(|(key, value)| (key.clone(), control_value_to_json(value)))
                .collect(),
        ),
        DynamicControlValue::Unknown => JsonValue::Null,
    }
}

pub fn parse_json_control_value(
    value_type: &ControlValueType,
    raw: &str,
) -> Result<DynamicControlValue, String> {
    let json = serde_json::from_str::<JsonValue>(raw).map_err(|error| format!("JSON: {error}"))?;
    let value = json_to_control_value(value_type, json)?;
    value_type
        .validate_value(&value)
        .map_err(|error| format!("Invalid value: {error}"))?;
    Ok(value)
}

fn json_to_control_value(
    value_type: &ControlValueType,
    value: JsonValue,
) -> Result<DynamicControlValue, String> {
    match value_type {
        ControlValueType::Bool => value
            .as_bool()
            .map(DynamicControlValue::Bool)
            .ok_or_else(|| "Expected boolean JSON".to_string()),
        ControlValueType::Integer { .. } => value
            .as_i64()
            .map(DynamicControlValue::Integer)
            .ok_or_else(|| "Expected integer JSON".to_string()),
        ControlValueType::Float { .. } => value
            .as_f64()
            .map(DynamicControlValue::Float)
            .ok_or_else(|| "Expected number JSON".to_string()),
        ControlValueType::String { .. } => value
            .as_str()
            .map(|value| DynamicControlValue::String(value.to_string()))
            .ok_or_else(|| "Expected string JSON".to_string()),
        ControlValueType::Secret => value
            .as_str()
            .map(|value| DynamicControlValue::SecretRef(value.to_string()))
            .ok_or_else(|| "Expected secret reference string".to_string()),
        ControlValueType::IpAddress => value
            .as_str()
            .map(|value| DynamicControlValue::IpAddress(value.to_string()))
            .ok_or_else(|| "Expected IP address string".to_string()),
        ControlValueType::MacAddress => value
            .as_str()
            .map(|value| DynamicControlValue::MacAddress(value.to_string()))
            .ok_or_else(|| "Expected MAC address string".to_string()),
        ControlValueType::DurationMs { .. } => value
            .as_u64()
            .map(DynamicControlValue::DurationMs)
            .ok_or_else(|| "Expected duration integer JSON".to_string()),
        ControlValueType::Enum { .. } => value
            .as_str()
            .map(|value| DynamicControlValue::Enum(value.to_string()))
            .ok_or_else(|| "Expected enum string".to_string()),
        ControlValueType::Flags { .. } => value
            .as_array()
            .ok_or_else(|| "Expected flags array".to_string())?
            .iter()
            .map(|value| {
                value
                    .as_str()
                    .map(ToOwned::to_owned)
                    .ok_or_else(|| "Expected flag string".to_string())
            })
            .collect::<Result<Vec<_>, _>>()
            .map(DynamicControlValue::Flags),
        ControlValueType::ColorRgb => json_to_color::<3>(value).map(DynamicControlValue::ColorRgb),
        ControlValueType::ColorRgba => {
            json_to_color::<4>(value).map(DynamicControlValue::ColorRgba)
        }
        ControlValueType::List { item_type, .. } => value
            .as_array()
            .ok_or_else(|| "Expected list array".to_string())?
            .iter()
            .cloned()
            .map(|item| json_to_control_value(item_type, item))
            .collect::<Result<Vec<_>, _>>()
            .map(DynamicControlValue::List),
        ControlValueType::Object { fields } => json_to_object(fields, value),
        ControlValueType::Unknown => Err("Unsupported control value type".to_string()),
    }
}

fn json_to_object(
    fields: &[ControlObjectField],
    value: JsonValue,
) -> Result<DynamicControlValue, String> {
    let object = value
        .as_object()
        .ok_or_else(|| "Expected object JSON".to_string())?;
    for key in object.keys() {
        if !fields.iter().any(|field| field.id == *key) {
            return Err(format!("Unknown object field: {key}"));
        }
    }
    let mut values = BTreeMap::new();
    for field in fields {
        if let Some(value) = object.get(&field.id) {
            values.insert(
                field.id.clone(),
                json_to_control_value(&field.value_type, value.clone())?,
            );
        } else if let Some(default_value) = &field.default_value {
            values.insert(field.id.clone(), default_value.clone());
        } else if field.required {
            return Err(format!("Missing required object field: {}", field.id));
        }
    }
    Ok(DynamicControlValue::Object(values))
}

fn json_to_color<const N: usize>(value: JsonValue) -> Result<[u8; N], String> {
    if let Some(text) = value.as_str() {
        return parse_color_hex_channels(text);
    }
    let channels = value
        .as_array()
        .ok_or_else(|| "Expected color array or hex string".to_string())?;
    if channels.len() != N {
        return Err(format!("Expected {N} color channels"));
    }
    let mut out = [0_u8; N];
    for (index, channel) in channels.iter().enumerate() {
        let Some(channel) = channel.as_u64().and_then(|value| u8::try_from(value).ok()) else {
            return Err("Expected color channels from 0-255".to_string());
        };
        out[index] = channel;
    }
    Ok(out)
}

fn parse_color_hex_channels<const N: usize>(raw: &str) -> Result<[u8; N], String> {
    let hex = raw.trim().trim_start_matches('#');
    if hex.len() != N * 2 {
        return Err(format!("Expected {}-digit hex color", N * 2));
    }

    let mut out = [0_u8; N];
    for (index, channel) in out.iter_mut().enumerate() {
        let start = index * 2;
        let end = start + 2;
        *channel = u8::from_str_radix(&hex[start..end], 16)
            .map_err(|_| "Expected hex color channels".to_string())?;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use hypercolor_types::controls::{
        ControlObjectField, ControlValue as DynamicControlValue, ControlValueType,
    };

    use super::{json_text, parse_json_control_value};

    #[test]
    fn parses_secret_json_as_secret_reference() {
        let value = parse_json_control_value(&ControlValueType::Secret, r#""credential:hue:key""#)
            .expect("secret reference should parse");

        assert_eq!(
            value,
            DynamicControlValue::SecretRef("credential:hue:key".to_owned())
        );
    }

    #[test]
    fn parses_object_defaults_and_rejects_unknown_fields() {
        let value_type = ControlValueType::Object {
            fields: vec![
                ControlObjectField {
                    id: "enabled".to_owned(),
                    label: "Enabled".to_owned(),
                    value_type: ControlValueType::Bool,
                    required: true,
                    default_value: None,
                },
                ControlObjectField {
                    id: "mode".to_owned(),
                    label: "Mode".to_owned(),
                    value_type: ControlValueType::String {
                        min_len: None,
                        max_len: Some(16),
                        pattern: None,
                    },
                    required: false,
                    default_value: Some(DynamicControlValue::String("auto".to_owned())),
                },
            ],
        };

        let value = parse_json_control_value(&value_type, r#"{"enabled": true}"#)
            .expect("object should parse");
        let DynamicControlValue::Object(values) = value else {
            panic!("expected object control value");
        };
        assert_eq!(values["enabled"], DynamicControlValue::Bool(true));
        assert_eq!(
            values["mode"],
            DynamicControlValue::String("auto".to_owned())
        );

        let error = parse_json_control_value(&value_type, r#"{"enabled": true, "extra": 1}"#)
            .expect_err("unknown object fields should fail");
        assert!(error.contains("Unknown object field: extra"));
    }

    #[test]
    fn parses_color_hex_and_rejects_bad_channels() {
        let value = parse_json_control_value(&ControlValueType::ColorRgb, r##""#80ffea""##)
            .expect("hex color should parse");
        assert_eq!(value, DynamicControlValue::ColorRgb([128, 255, 234]));

        let error = parse_json_control_value(&ControlValueType::ColorRgb, "[0, 300, 1]")
            .expect_err("out of range channel should fail");
        assert!(error.contains("Expected color channels from 0-255"));
    }

    #[test]
    fn json_text_redacts_unknown_values_to_null() {
        assert_eq!(json_text(Some(&DynamicControlValue::Unknown)), "null");
    }
}
