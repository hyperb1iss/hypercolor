use hypercolor_types::config::DriverConfigEntry;
use hypercolor_types::control::ControlValue;
use serde::de::DeserializeOwned;

use crate::DriverError;

/// Read-only resolved config for one driver.
#[derive(Debug, Clone, Copy)]
pub struct DriverConfigView<'a> {
    pub driver_id: &'a str,
    pub entry: &'a DriverConfigEntry,
}

impl DriverConfigView<'_> {
    /// Whether the host should activate this driver.
    #[must_use]
    pub const fn enabled(&self) -> bool {
        self.entry.enabled
    }

    /// Deserialize this driver's settings into a typed private config.
    ///
    /// # Errors
    ///
    /// Returns an error when the settings payload does not match `T`.
    pub fn parse_settings<T>(&self) -> Result<T, DriverError>
    where
        T: DeserializeOwned,
    {
        let settings = self
            .entry
            .settings
            .iter()
            .map(|(key, value)| {
                let value = if ControlValue::has_canonical_wire_shape(value) {
                    let canonical = serde_json::from_value::<ControlValue>(value.clone()).map_err(
                        |error| DriverError::Configuration {
                            message: format!(
                                "invalid control value for driver '{}' setting '{key}': {error}",
                                self.driver_id
                            ),
                        },
                    )?;
                    control_value_to_settings_json(canonical)?
                } else {
                    value.clone()
                };
                Ok((key.clone(), value))
            })
            .collect::<Result<serde_json::Map<_, _>, DriverError>>()?;
        let settings = serde_json::Value::Object(settings);
        serde_json::from_value(settings).map_err(|error| DriverError::Configuration {
            message: format!("invalid config for driver '{}': {error}", self.driver_id),
        })
    }
}

fn control_value_to_settings_json(value: ControlValue) -> Result<serde_json::Value, DriverError> {
    let invalid = |kind: &str| DriverError::Configuration {
        message: format!("{kind} control values cannot configure a driver"),
    };
    Ok(match value {
        ControlValue::Null => serde_json::Value::Null,
        ControlValue::Bool(value) => serde_json::Value::Bool(value),
        ControlValue::Int(value) => serde_json::Value::from(value),
        ControlValue::Float(value) => serde_json::Value::from(value),
        ControlValue::Text(value) | ControlValue::Enum(value) => serde_json::Value::String(value),
        ControlValue::SecretRef(value) => serde_json::Value::String(value.as_str().to_owned()),
        ControlValue::Ip(value) => serde_json::Value::String(value.as_str().to_owned()),
        ControlValue::Mac(value) => serde_json::Value::String(value.as_str().to_owned()),
        ControlValue::Duration(value) => serde_json::Value::from(
            u64::try_from(value.as_millis()).map_err(|_| invalid("overflowing duration"))?,
        ),
        ControlValue::ColorRgb(value) => serde_json::json!([value.r, value.g, value.b]),
        ControlValue::ColorRgba(value) => {
            serde_json::json!([value.r, value.g, value.b, value.a])
        }
        ControlValue::Flags(values) => {
            serde_json::Value::Array(values.into_iter().map(serde_json::Value::String).collect())
        }
        ControlValue::List(values) => serde_json::Value::Array(
            values
                .into_iter()
                .map(control_value_to_settings_json)
                .collect::<Result<_, _>>()?,
        ),
        ControlValue::Map(values) => serde_json::Value::Object(
            values
                .into_iter()
                .map(|(key, value)| control_value_to_settings_json(value).map(|value| (key, value)))
                .collect::<Result<_, _>>()?,
        ),
        ControlValue::ColorLinear(_) => return Err(invalid("linear color")),
        ControlValue::Gradient(_) => return Err(invalid("gradient")),
        ControlValue::Rect(_) => return Err(invalid("rectangle")),
        ControlValue::Unknown => return Err(invalid("unknown")),
    })
}

/// Optional driver-owned configuration metadata and validation.
pub trait DriverConfigProvider: Send + Sync {
    /// Default config entry for this driver.
    fn default_config(&self) -> DriverConfigEntry;

    /// Validate a resolved config entry.
    ///
    /// # Errors
    ///
    /// Returns an error if the driver cannot accept the config payload.
    fn validate_config(&self, config: &DriverConfigEntry) -> Result<(), DriverError>;
}
