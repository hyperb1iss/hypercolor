//! Authorization for devices that exist only because config names them.
//!
//! A manual driver never probes for hardware, so the config is the only thing
//! that says a device may be opened. Discovery metadata and anything else a
//! host retained can describe such a device, but only [`ConfiguredDevices`]
//! authorizes one: it resolves the device from the live config at the moment
//! the driver acts, and [`ConfiguredDevices::confirm`] holds an open that
//! spanned async setup to the config it started from.

use std::sync::Arc;

use hypercolor_driver_api::{DriverConfigSource, DriverConfigView, DriverError};
use hypercolor_types::config::DriverConfigEntry;
use hypercolor_types::device::{DeviceError, DeviceId};

/// Validates a driver's config and lists its devices with their plans.
pub type ResolveDevices<P> = fn(DriverConfigView<'_>) -> Result<Vec<(DeviceId, P)>, DriverError>;

/// A manual driver's devices, as the live config names them.
pub struct ConfiguredDevices<P> {
    driver_id: &'static str,
    config: Arc<dyn DriverConfigSource>,
    resolve: ResolveDevices<P>,
}

impl<P> Clone for ConfiguredDevices<P> {
    fn clone(&self) -> Self {
        Self {
            driver_id: self.driver_id,
            config: Arc::clone(&self.config),
            resolve: self.resolve,
        }
    }
}

/// One device resolved from one reading of the live config.
pub struct ConfiguredDevice<P> {
    plan: Arc<P>,
    /// The config generation this plan came from. The entry itself is the
    /// generation: it is scoped to this driver, so edits elsewhere never
    /// disturb an open, and a value that compares equal is still authorized.
    generation: DriverConfigEntry,
}

impl<P> ConfiguredDevice<P> {
    /// The resolved plan.
    #[must_use]
    pub const fn plan(&self) -> &Arc<P> {
        &self.plan
    }
}

impl<P> ConfiguredDevices<P> {
    /// Authorize `driver_id`'s devices against `config`.
    #[must_use]
    pub fn new(
        driver_id: &'static str,
        config: Arc<dyn DriverConfigSource>,
        resolve: ResolveDevices<P>,
    ) -> Self {
        Self {
            driver_id,
            config,
            resolve,
        }
    }

    /// Resolve `id` from the config as it is right now.
    ///
    /// # Errors
    ///
    /// Returns [`DeviceError::NotFound`] when the driver is disabled, its
    /// config is invalid, or no configured device has this id.
    pub fn current(&self, id: &DeviceId) -> Result<ConfiguredDevice<P>, DeviceError> {
        let entry = self.config.current();
        let driver_id = self.driver_id;
        if !entry.enabled {
            return Err(not_configured(
                id,
                &format!("the {driver_id} driver is disabled"),
            ));
        }
        let view = DriverConfigView {
            driver_id,
            entry: &entry,
        };
        let devices = (self.resolve)(view).map_err(|error| {
            not_configured(id, &format!("the {driver_id} config is invalid: {error}"))
        })?;
        let plan = devices
            .into_iter()
            .find_map(|(device_id, plan)| (device_id == *id).then_some(plan))
            .ok_or_else(|| not_configured(id, "no device in the current config has this id"))?;
        Ok(ConfiguredDevice {
            plan: Arc::new(plan),
            generation: entry,
        })
    }

    /// Check that the config `device` came from is still current.
    ///
    /// Call this after async setup and immediately before publishing, with
    /// no await in between, so setup never outlives the authorization that
    /// started it.
    ///
    /// # Errors
    ///
    /// Returns what [`Self::current`] returns when the device is no longer
    /// configured. When it is still configured but the config moved, returns
    /// [`DeviceError::ConnectionFailed`], so the lifecycle retries against
    /// the new config.
    pub fn confirm(&self, id: &DeviceId, device: &ConfiguredDevice<P>) -> Result<(), DeviceError> {
        if self.config.current() == device.generation {
            return Ok(());
        }
        self.current(id)?;
        Err(DeviceError::connection(
            id,
            "the driver config changed while this device was opening",
        ))
    }
}

fn not_configured(id: &DeviceId, reason: &str) -> DeviceError {
    DeviceError::NotFound {
        device: format!("{id} ({reason})"),
    }
}
