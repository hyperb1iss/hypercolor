use anyhow::{Context, Result};
use hypercolor_types::config::DriverConfigEntry;
use serde::de::DeserializeOwned;

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
    pub fn parse_settings<T>(&self) -> Result<T>
    where
        T: DeserializeOwned,
    {
        let settings = serde_json::Value::Object(
            self.entry
                .settings
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
        );
        serde_json::from_value(settings)
            .with_context(|| format!("invalid config for driver '{}'", self.driver_id))
    }
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
    fn validate_config(&self, config: &DriverConfigEntry) -> Result<()>;
}
