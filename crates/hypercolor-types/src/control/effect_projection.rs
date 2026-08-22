use crate::effect::GradientStop;
use crate::viewport::ViewportRect;

use super::{ControlValue, ControlValueInvalid, validate_gradient};

/// Why a raw effect-control JSON value cannot enter the canonical algebra.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EffectJsonValueError {
    /// The JSON value is not one of the shapes accepted by effect controls.
    #[error("unsupported effect control JSON shape")]
    UnsupportedShape,
    /// A number cannot be represented by the effect renderer's `f32` ABI.
    #[error("effect control number must be finite and within the f32 range")]
    FloatOutOfRange,
    /// A canonical integer cannot fit the effect renderer's `i32` ABI.
    #[error("effect control integer must be within the i32 range")]
    IntegerOutOfRange,
    /// A decoded gradient violates the canonical gradient contract.
    #[error("invalid effect gradient: {source}")]
    InvalidGradient {
        /// The canonical invariant that failed.
        source: Box<ControlValueInvalid>,
    },
    /// A nested projection failed; `path` locates the rejected value.
    #[error("{path}: {source}")]
    Nested {
        /// Where in the list or map the failure sits.
        path: String,
        /// The underlying projection failure.
        source: Box<EffectJsonValueError>,
    },
}

impl EffectJsonValueError {
    fn nested(path: impl Into<String>, source: Self) -> Self {
        Self::Nested {
            path: path.into(),
            source: Box::new(source),
        }
    }

    fn invalid_gradient(source: ControlValueInvalid) -> Self {
        Self::InvalidGradient {
            source: Box::new(source),
        }
    }
}

fn parse_effect_gradient_stop(
    index: usize,
    value: &serde_json::Value,
) -> Result<GradientStop, EffectJsonValueError> {
    let Some(object) = value.as_object() else {
        return Err(EffectJsonValueError::nested(
            format!("[{index}]"),
            EffectJsonValueError::UnsupportedShape,
        ));
    };
    if object.len() != 2 || !object.contains_key("pos") || !object.contains_key("color") {
        return Err(EffectJsonValueError::nested(
            format!("[{index}]"),
            EffectJsonValueError::UnsupportedShape,
        ));
    }

    let position = object
        .get("pos")
        .and_then(serde_json::Value::as_f64)
        .ok_or_else(|| {
            EffectJsonValueError::nested(
                format!("[{index}].pos"),
                EffectJsonValueError::UnsupportedShape,
            )
        })
        .and_then(|value| {
            narrow_effect_f32(value)
                .map_err(|error| EffectJsonValueError::nested(format!("[{index}].pos"), error))
        })?;
    let color = object
        .get("color")
        .and_then(serde_json::Value::as_array)
        .filter(|color| color.len() == 4)
        .ok_or_else(|| {
            EffectJsonValueError::nested(
                format!("[{index}].color"),
                EffectJsonValueError::UnsupportedShape,
            )
        })?;
    let mut channels = [0.0_f32; 4];
    for (channel, component) in color.iter().enumerate() {
        channels[channel] = component
            .as_f64()
            .ok_or_else(|| {
                EffectJsonValueError::nested(
                    format!("[{index}].color[{channel}]"),
                    EffectJsonValueError::UnsupportedShape,
                )
            })
            .and_then(|value| {
                narrow_effect_f32(value).map_err(|error| {
                    EffectJsonValueError::nested(format!("[{index}].color[{channel}]"), error)
                })
            })?;
    }

    Ok(GradientStop {
        position,
        color: channels,
    })
}

impl ControlValue {
    /// Admit a raw effect-control JSON value into the canonical algebra.
    ///
    /// The returned value still needs validation against its
    /// [`crate::effect::ControlDefinition`].
    pub fn try_from_effect_json(value: &serde_json::Value) -> Result<Self, EffectJsonValueError> {
        if let Some(value) = value.as_i64() {
            i32::try_from(value).map_err(|_| EffectJsonValueError::IntegerOutOfRange)?;
            return Ok(Self::Int(value));
        }
        if let Some(value) = value.as_f64() {
            narrow_effect_f32(value)?;
            return Ok(Self::Float(value));
        }
        if let Some(value) = value.as_bool() {
            return Ok(Self::Bool(value));
        }
        if let Some(value) = value.as_str() {
            return Ok(Self::Text(value.to_owned()));
        }
        if let Some(array) = value.as_array() {
            if array.iter().all(serde_json::Value::is_number) {
                return Err(EffectJsonValueError::UnsupportedShape);
            }
            let gradient = Self::Gradient(
                array
                    .iter()
                    .enumerate()
                    .map(|(index, stop)| parse_effect_gradient_stop(index, stop))
                    .collect::<Result<Vec<_>, _>>()?,
            );
            gradient
                .validate()
                .map_err(EffectJsonValueError::invalid_gradient)?;
            return Ok(gradient);
        }
        if let Some(object) = value.as_object() {
            if object.len() != 4
                || !["x", "y", "width", "height"]
                    .into_iter()
                    .all(|key| object.contains_key(key))
            {
                return Err(EffectJsonValueError::UnsupportedShape);
            }
            let component = |name| {
                object
                    .get(name)
                    .and_then(serde_json::Value::as_f64)
                    .ok_or(EffectJsonValueError::UnsupportedShape)
                    .and_then(narrow_effect_f32)
            };
            return Ok(Self::rect(ViewportRect::new(
                component("x")?,
                component("y")?,
                component("width")?,
                component("height")?,
            )));
        }
        Err(EffectJsonValueError::UnsupportedShape)
    }

    /// Admit the effect renderer's four-channel linear color shape.
    ///
    /// A bare four-number array is ambiguous without an effect control
    /// schema. Callers may use this explicit entry point only after the
    /// addressed control is known to be a color picker.
    pub fn try_from_effect_color_json(
        value: &serde_json::Value,
    ) -> Result<Self, EffectJsonValueError> {
        let components = value
            .as_array()
            .filter(|components| components.len() == 4)
            .ok_or(EffectJsonValueError::UnsupportedShape)?;
        let mut color = [0.0_f32; 4];
        for (index, component) in components.iter().enumerate() {
            color[index] = narrow_effect_f32(
                component
                    .as_f64()
                    .ok_or(EffectJsonValueError::UnsupportedShape)?,
            )?;
        }
        Ok(Self::linear_color(color))
    }

    /// Project a canonical value into the raw JSON value consumed by an
    /// effect runtime.
    ///
    /// Canonical tags remain authoritative until this final renderer
    /// boundary. Numeric widths are checked against the effect ABI, colors
    /// preserve their linear channels, and gradient failures retain their
    /// exact path. Variants outside the effect algebra are rejected.
    pub fn try_to_effect_json(&self) -> Result<serde_json::Value, EffectJsonValueError> {
        Ok(match self {
            Self::Null
            | Self::Unknown
            | Self::SecretRef(_)
            | Self::Ip(_)
            | Self::Mac(_)
            | Self::Duration(_)
            | Self::ColorRgb(_)
            | Self::ColorRgba(_)
            | Self::Flags(_)
            | Self::List(_)
            | Self::Map(_) => return Err(EffectJsonValueError::UnsupportedShape),
            Self::Bool(value) => serde_json::Value::Bool(*value),
            Self::Int(value) => serde_json::Value::from(
                i32::try_from(*value).map_err(|_| EffectJsonValueError::IntegerOutOfRange)?,
            ),
            Self::Float(value) => effect_f32_json(narrow_effect_f32(*value)?),
            Self::Text(value) | Self::Enum(value) => serde_json::Value::String(value.clone()),
            Self::ColorLinear(value) => serde_json::Value::Array(
                [value.r, value.g, value.b, value.a]
                    .into_iter()
                    .map(|component| narrow_effect_f32(f64::from(component)).map(effect_f32_json))
                    .collect::<Result<Vec<_>, _>>()?,
            ),
            Self::Gradient(stops) => {
                validate_gradient(stops).map_err(EffectJsonValueError::invalid_gradient)?;
                serde_json::Value::Array(
                    stops
                        .iter()
                        .enumerate()
                        .map(|(index, stop)| {
                            let position = narrow_effect_f32(f64::from(stop.position))
                                .map(effect_f32_json)
                                .map_err(|error| {
                                    EffectJsonValueError::nested(format!("[{index}].pos"), error)
                                })?;
                            let color = stop
                                .color
                                .iter()
                                .enumerate()
                                .map(|(channel, value)| {
                                    narrow_effect_f32(f64::from(*value))
                                        .map(effect_f32_json)
                                        .map_err(|error| {
                                            EffectJsonValueError::nested(
                                                format!("[{index}].color[{channel}]"),
                                                error,
                                            )
                                        })
                                })
                                .collect::<Result<Vec<_>, _>>()?;
                            Ok(serde_json::json!({
                                "pos": position,
                                "color": color,
                            }))
                        })
                        .collect::<Result<Vec<_>, EffectJsonValueError>>()?,
                )
            }
            Self::Rect(value) => serde_json::json!({
                "x": effect_f32_json(narrow_effect_f32(f64::from(value.x))?),
                "y": effect_f32_json(narrow_effect_f32(f64::from(value.y))?),
                "width": effect_f32_json(narrow_effect_f32(f64::from(value.width))?),
                "height": effect_f32_json(narrow_effect_f32(f64::from(value.height))?),
            }),
        })
    }
}

/// Narrow a JSON number to the effect renderer's scalar width.
pub fn narrow_effect_f32(value: f64) -> Result<f32, EffectJsonValueError> {
    if !value.is_finite() || value < f64::from(f32::MIN) || value > f64::from(f32::MAX) {
        return Err(EffectJsonValueError::FloatOutOfRange);
    }
    #[expect(clippy::cast_possible_truncation, clippy::as_conversions)]
    Ok(value as f32)
}

fn effect_f32_json(value: f32) -> serde_json::Value {
    let encoded = serde_json::to_string(&value).expect("finite f32 must encode as JSON");
    serde_json::from_str(&encoded).expect("encoded f32 must decode as JSON")
}
