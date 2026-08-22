use std::collections::BTreeMap;
use std::time::Duration;

use hypercolor_color::{LinearRgba, Rgb, Rgba};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Value, json};
#[cfg(feature = "schema")]
use utoipa::{PartialSchema, ToSchema};

use crate::effect::GradientStop;
use crate::spatial::NormalizedRect;

use super::{ControlValue, ControlValueInvalid, IpText, MacText, SecretRef};

const CONTROL_VALUE_KINDS: &[&str] = &[
    "null",
    "bool",
    "int",
    "float",
    "text",
    "secret_ref",
    "ip",
    "mac",
    "duration",
    "color_rgb",
    "color_rgba",
    "color_linear",
    "gradient",
    "rect",
    "enum",
    "flags",
    "list",
    "map",
    "unknown",
];

fn is_control_value_kind(kind: &str) -> bool {
    CONTROL_VALUE_KINDS.contains(&kind)
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "value")]
enum ControlValueRef<'a> {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Text(&'a str),
    SecretRef(&'a str),
    Ip(&'a str),
    Mac(&'a str),
    Duration(u64),
    ColorRgb(Rgb),
    ColorRgba(Rgba),
    ColorLinear(LinearRgba),
    Gradient(&'a [GradientStop]),
    Rect(NormalizedRect),
    Enum(&'a str),
    Flags(&'a [String]),
    List(&'a [ControlValue]),
    Map(&'a BTreeMap<String, ControlValue>),
    Unknown,
}

#[cfg(feature = "schema")]
#[derive(Deserialize, utoipa::ToSchema)]
#[allow(dead_code)]
#[cfg_attr(feature = "schema", schema(no_recursion))]
#[serde(rename_all = "snake_case", tag = "kind", content = "value")]
enum ControlValueWire {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Text(String),
    SecretRef(String),
    Ip(String),
    Mac(String),
    Duration(u64),
    ColorRgb(Rgb),
    ColorRgba(Rgba),
    ColorLinear(LinearRgba),
    Gradient(Vec<GradientStop>),
    Rect(NormalizedRect),
    Enum(String),
    Flags(Vec<String>),
    List(Vec<ControlValue>),
    Map(BTreeMap<String, ControlValue>),
    Unknown,
}

#[cfg(feature = "schema")]
impl utoipa::__dev::ComposeSchema for ControlValue {
    fn compose(
        _new_generics: Vec<utoipa::openapi::RefOr<utoipa::openapi::schema::Schema>>,
    ) -> utoipa::openapi::RefOr<utoipa::openapi::schema::Schema> {
        ControlValueWire::schema()
    }
}

#[cfg(feature = "schema")]
impl ToSchema for ControlValue {
    fn name() -> std::borrow::Cow<'static, str> {
        std::borrow::Cow::Borrowed("ControlValue")
    }

    fn schemas(
        schemas: &mut Vec<(
            String,
            utoipa::openapi::RefOr<utoipa::openapi::schema::Schema>,
        )>,
    ) {
        ControlValueWire::schemas(schemas);
        for (name, schema) in [
            (Rgb::name().into_owned(), Rgb::schema()),
            (Rgba::name().into_owned(), Rgba::schema()),
            (LinearRgba::name().into_owned(), LinearRgba::schema()),
            (GradientStop::name().into_owned(), GradientStop::schema()),
            (
                NormalizedRect::name().into_owned(),
                NormalizedRect::schema(),
            ),
        ] {
            schemas.push((name, schema));
        }
    }
}

impl Serialize for ControlValue {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.validate().map_err(serde::ser::Error::custom)?;
        let wire = match self {
            Self::Null => ControlValueRef::Null,
            Self::Bool(value) => ControlValueRef::Bool(*value),
            Self::Int(value) => ControlValueRef::Int(*value),
            Self::Float(value) => ControlValueRef::Float(*value),
            Self::Text(value) => ControlValueRef::Text(value),
            Self::SecretRef(value) => ControlValueRef::SecretRef(value.as_str()),
            Self::Ip(value) => ControlValueRef::Ip(value.as_str()),
            Self::Mac(value) => ControlValueRef::Mac(value.as_str()),
            Self::Duration(value) => {
                if value.subsec_nanos() % 1_000_000 != 0 {
                    return Err(serde::ser::Error::custom(
                        ControlValueInvalid::SubMillisecondDuration,
                    ));
                }
                let millis = u64::try_from(value.as_millis()).map_err(|_| {
                    serde::ser::Error::custom(ControlValueInvalid::DurationOverflow)
                })?;
                ControlValueRef::Duration(millis)
            }
            Self::ColorRgb(value) => ControlValueRef::ColorRgb(*value),
            Self::ColorRgba(value) => ControlValueRef::ColorRgba(*value),
            Self::ColorLinear(value) => ControlValueRef::ColorLinear(*value),
            Self::Gradient(value) => ControlValueRef::Gradient(value),
            Self::Rect(value) => ControlValueRef::Rect(*value),
            Self::Enum(value) => ControlValueRef::Enum(value),
            Self::Flags(value) => ControlValueRef::Flags(value),
            Self::List(value) => ControlValueRef::List(value),
            Self::Map(value) => ControlValueRef::Map(value),
            Self::Unknown => ControlValueRef::Unknown,
        };
        wire.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for ControlValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Default)]
        enum RawPayload {
            #[default]
            Missing,
            Present(serde_json::Value),
        }

        fn deserialize_payload<'de, D>(deserializer: D) -> Result<RawPayload, D::Error>
        where
            D: Deserializer<'de>,
        {
            serde_json::Value::deserialize(deserializer).map(RawPayload::Present)
        }

        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct RawControlValue {
            kind: String,
            #[serde(default, deserialize_with = "deserialize_payload")]
            value: RawPayload,
        }

        fn parse_value<T, E>(kind: &str, value: Option<serde_json::Value>) -> Result<T, E>
        where
            T: serde::de::DeserializeOwned,
            E: serde::de::Error,
        {
            let value = value.ok_or_else(|| E::custom(format!("missing value for {kind}")))?;
            serde_json::from_value(value)
                .map_err(|error| E::custom(format!("invalid {kind} value: {error}")))
        }

        fn parse_closed_object<T, E>(
            kind: &str,
            value: Option<serde_json::Value>,
            fields: &[&str],
        ) -> Result<T, E>
        where
            T: serde::de::DeserializeOwned,
            E: serde::de::Error,
        {
            let value = value.ok_or_else(|| E::custom(format!("missing value for {kind}")))?;
            if let Some(object) = value.as_object()
                && let Some(field) = object
                    .keys()
                    .find(|field| !fields.contains(&field.as_str()))
            {
                return Err(E::custom(format!(
                    "invalid {kind} value: unknown field `{field}`"
                )));
            }
            serde_json::from_value(value)
                .map_err(|error| E::custom(format!("invalid {kind} value: {error}")))
        }

        fn parse_gradient<E>(value: Option<serde_json::Value>) -> Result<Vec<GradientStop>, E>
        where
            E: serde::de::Error,
        {
            let value = value.ok_or_else(|| E::custom("missing value for gradient"))?;
            if let Some(stops) = value.as_array() {
                for (index, stop) in stops.iter().enumerate() {
                    if let Some(object) = stop.as_object()
                        && let Some(field) = object
                            .keys()
                            .find(|field| !matches!(field.as_str(), "position" | "color"))
                    {
                        return Err(E::custom(format!(
                            "invalid gradient value: stop {index} has unknown field `{field}`"
                        )));
                    }
                }
            }
            serde_json::from_value(value)
                .map_err(|error| E::custom(format!("invalid gradient value: {error}")))
        }

        fn parse_unit<E>(kind: &str, value: Option<serde_json::Value>) -> Result<(), E>
        where
            E: serde::de::Error,
        {
            if value.is_some() {
                return Err(E::custom(format!("{kind} must not contain a value")));
            }
            Ok(())
        }

        let raw = RawControlValue::deserialize(deserializer)?;
        let raw_value = match raw.value {
            RawPayload::Missing => None,
            RawPayload::Present(value) => Some(value),
        };
        let value = match raw.kind.as_str() {
            "null" => {
                parse_unit::<D::Error>("null", raw_value)?;
                Self::Null
            }
            "bool" => Self::Bool(parse_value("bool", raw_value)?),
            "int" => Self::Int(parse_value("int", raw_value)?),
            "float" => Self::Float(parse_value("float", raw_value)?),
            "text" => Self::Text(parse_value("text", raw_value)?),
            "secret_ref" => Self::SecretRef(SecretRef::new(parse_value::<String, D::Error>(
                "secret_ref",
                raw_value,
            )?)),
            "ip" => Self::Ip(
                IpText::new(parse_value::<String, D::Error>("ip", raw_value)?)
                    .map_err(serde::de::Error::custom)?,
            ),
            "mac" => Self::Mac(
                MacText::new(parse_value::<String, D::Error>("mac", raw_value)?)
                    .map_err(serde::de::Error::custom)?,
            ),
            "duration" => {
                Self::Duration(Duration::from_millis(parse_value("duration", raw_value)?))
            }
            "color_rgb" => Self::ColorRgb(parse_closed_object(
                "color_rgb",
                raw_value,
                &["r", "g", "b"],
            )?),
            "color_rgba" => Self::ColorRgba(parse_closed_object(
                "color_rgba",
                raw_value,
                &["r", "g", "b", "a"],
            )?),
            "color_linear" => Self::ColorLinear(parse_closed_object(
                "color_linear",
                raw_value,
                &["r", "g", "b", "a"],
            )?),
            "gradient" => Self::Gradient(parse_gradient(raw_value)?),
            "rect" => Self::Rect(parse_closed_object(
                "rect",
                raw_value,
                &["x", "y", "width", "height"],
            )?),
            "enum" => Self::Enum(parse_value("enum", raw_value)?),
            "flags" => Self::Flags(parse_value("flags", raw_value)?),
            "list" => Self::List(parse_value("list", raw_value)?),
            "map" => Self::Map(parse_value("map", raw_value)?),
            "unknown" => {
                parse_unit::<D::Error>("unknown", raw_value)?;
                Self::Unknown
            }
            other => {
                return Err(serde::de::Error::unknown_variant(
                    other,
                    CONTROL_VALUE_KINDS,
                ));
            }
        };
        value.validate().map_err(serde::de::Error::custom)?;
        Ok(value)
    }
}

impl ControlValue {
    /// Return whether a JSON value claims the canonical tagged-wire namespace.
    ///
    /// Exact `kind`/`value` envelopes and reserved canonical tags are parsed
    /// strictly so malformed values fail loudly. Arbitrary driver configuration
    /// objects with an unrelated `kind` field remain raw provider data.
    #[must_use]
    pub fn is_canonical_wire_candidate(value: &serde_json::Value) -> bool {
        let Some(object) = value.as_object() else {
            return false;
        };
        let Some(kind) = object.get("kind") else {
            return false;
        };
        object
            .keys()
            .all(|key| matches!(key.as_str(), "kind" | "value"))
            || kind.as_str().is_some_and(is_control_value_kind)
    }
}

/// Return the canonical recursive JSON Schema for the tagged control wire.
///
/// Structural constraints live here so REST documentation, MCP tools, and
/// generated clients cannot drift into independent value algebras. Gradient
/// item shapes and ranges are structural; [`ControlValue::validate`] enforces
/// the nondecreasing stop-order invariant after deserialization.
#[must_use]
pub fn control_value_json_schema() -> Value {
    fn unit(kind: &str) -> Value {
        json!({
            "type": "object",
            "properties": { "kind": { "const": kind } },
            "required": ["kind"],
            "additionalProperties": false
        })
    }

    fn tagged(kind: &str, value: Value) -> Value {
        json!({
            "type": "object",
            "properties": {
                "kind": { "const": kind },
                "value": value
            },
            "required": ["kind", "value"],
            "additionalProperties": false
        })
    }

    fn channels(names: &[&str], channel: Value) -> Value {
        let properties = names
            .iter()
            .map(|name| ((*name).to_owned(), channel.clone()))
            .collect::<serde_json::Map<_, _>>();
        json!({
            "type": "object",
            "properties": properties,
            "required": names,
            "additionalProperties": false
        })
    }

    let recursive = json!({ "$ref": "#/$defs/controlValue" });
    let byte = json!({ "type": "integer", "minimum": 0, "maximum": 255 });
    let number = json!({ "type": "number" });
    let f32_number = json!({
        "type": "number",
        "minimum": -f64::from(f32::MAX),
        "maximum": f64::from(f32::MAX)
    });
    let normalized_channel = json!({ "type": "number", "minimum": 0.0, "maximum": 1.0 });
    let variants = vec![
        unit("null"),
        tagged("bool", json!({ "type": "boolean" })),
        tagged(
            "int",
            json!({
                "type": "integer",
                "minimum": i64::MIN,
                "maximum": i64::MAX
            }),
        ),
        tagged("float", number.clone()),
        tagged("text", json!({ "type": "string" })),
        tagged("secret_ref", json!({ "type": "string" })),
        tagged(
            "ip",
            json!({
                "type": "string",
                "anyOf": [
                    { "format": "ipv4" },
                    { "format": "ipv6" }
                ]
            }),
        ),
        tagged(
            "mac",
            json!({
                "type": "string",
                "pattern": "^(?:[0-9A-Fa-f]{12}|(?:[0-9A-Fa-f]{2}:){5}[0-9A-Fa-f]{2}|(?:[0-9A-Fa-f]{2}-){5}[0-9A-Fa-f]{2}|(?:[0-9A-Fa-f]{4}\\.){2}[0-9A-Fa-f]{4})$"
            }),
        ),
        tagged(
            "duration",
            json!({ "type": "integer", "minimum": 0, "maximum": u64::MAX }),
        ),
        tagged("color_rgb", channels(&["r", "g", "b"], byte.clone())),
        tagged("color_rgba", channels(&["r", "g", "b", "a"], byte)),
        tagged(
            "color_linear",
            channels(&["r", "g", "b", "a"], f32_number.clone()),
        ),
        tagged(
            "gradient",
            json!({
                "type": "array",
                "description": "Two to eight stops in nondecreasing position order. JSON Schema validates each stop shape and range; canonical value admission enforces ordering.",
                "minItems": 2,
                "maxItems": 8,
                "items": {
                    "type": "object",
                    "properties": {
                        "position": { "type": "number", "minimum": 0.0, "maximum": 1.0 },
                        "color": {
                            "type": "array",
                            "items": normalized_channel,
                            "minItems": 4,
                            "maxItems": 4
                        }
                    },
                    "required": ["position", "color"],
                    "additionalProperties": false
                }
            }),
        ),
        tagged("rect", channels(&["x", "y", "width", "height"], f32_number)),
        tagged("enum", json!({ "type": "string" })),
        tagged(
            "flags",
            json!({ "type": "array", "items": { "type": "string" } }),
        ),
        tagged(
            "list",
            json!({ "type": "array", "items": recursive.clone() }),
        ),
        tagged(
            "map",
            json!({ "type": "object", "additionalProperties": recursive }),
        ),
        unit("unknown"),
    ];
    json!({ "oneOf": variants })
}
