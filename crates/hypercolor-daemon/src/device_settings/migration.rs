use std::collections::{BTreeMap, HashMap};
use std::time::Duration;

use anyhow::Context;
use hypercolor_color::{Rgb, Rgba};
use hypercolor_types::control::{ControlValue, SecretRef};
use hypercolor_types::controls::ControlValueMap;

pub(super) fn migrate_v2_driver_controls(
    controls: HashMap<String, BTreeMap<String, serde_json::Value>>,
) -> anyhow::Result<HashMap<String, ControlValueMap>> {
    controls
        .into_iter()
        .map(|(device_key, values)| {
            values
                .into_iter()
                .map(|(control_id, value)| {
                    migrate_v2_control_value(value)
                        .with_context(|| {
                            format!(
                                "device settings control '{control_id}' for '{device_key}' is invalid"
                            )
                        })
                        .map(|value| (control_id, value))
                })
                .collect::<anyhow::Result<ControlValueMap>>()
                .map(|values| (device_key, values))
        })
        .collect()
}

fn migrate_v2_control_value(value: serde_json::Value) -> anyhow::Result<ControlValue> {
    let mut object = value
        .as_object()
        .cloned()
        .context("legacy control value must be an object")?;
    let kind = object
        .remove("kind")
        .and_then(|value| value.as_str().map(str::to_owned))
        .context("legacy control value must carry a kind")?;
    let payload = object.remove("value");

    fn parse<T: serde::de::DeserializeOwned>(
        kind: &str,
        payload: Option<serde_json::Value>,
    ) -> anyhow::Result<T> {
        serde_json::from_value(payload.with_context(|| format!("missing {kind} value"))?)
            .with_context(|| format!("invalid {kind} value"))
    }

    let value = match kind.as_str() {
        "null" => ControlValue::Null,
        "bool" => ControlValue::Bool(parse("bool", payload)?),
        "integer" => ControlValue::Int(parse("integer", payload)?),
        "float" => ControlValue::Float(parse("float", payload)?),
        "string" => ControlValue::Text(parse("string", payload)?),
        "secret_ref" => {
            ControlValue::SecretRef(SecretRef::new(parse::<String>("secret_ref", payload)?))
        }
        "color_rgb" => {
            let [r, g, b] = parse("color_rgb", payload)?;
            ControlValue::ColorRgb(Rgb::new(r, g, b))
        }
        "color_rgba" => {
            let [r, g, b, a] = parse("color_rgba", payload)?;
            ControlValue::ColorRgba(Rgba::new(r, g, b, a))
        }
        "ip_address" => ControlValue::ip(parse::<String>("ip_address", payload)?)?,
        "mac_address" => ControlValue::mac(parse::<String>("mac_address", payload)?)?,
        "duration_ms" => {
            ControlValue::Duration(Duration::from_millis(parse("duration_ms", payload)?))
        }
        "enum" => ControlValue::Enum(parse("enum", payload)?),
        "flags" => ControlValue::Flags(parse("flags", payload)?),
        "list" => ControlValue::List(
            parse::<Vec<serde_json::Value>>("list", payload)?
                .into_iter()
                .map(migrate_v2_control_value)
                .collect::<anyhow::Result<Vec<_>>>()?,
        ),
        "object" => ControlValue::Map(
            parse::<BTreeMap<String, serde_json::Value>>("object", payload)?
                .into_iter()
                .map(|(key, value)| migrate_v2_control_value(value).map(|value| (key, value)))
                .collect::<anyhow::Result<BTreeMap<_, _>>>()?,
        ),
        _ => ControlValue::Unknown,
    };
    value.validate()?;
    Ok(value)
}
